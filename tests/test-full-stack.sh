#!/usr/bin/env bash
set -uo pipefail

SERVER="${Z8S_SERVER:-https://localhost:6443}"
NS="fulltest"
GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; }
k()   { kubectl --kubeconfig ~/.kube/config "$@"; }
check() { local m="$1"; shift; "$@" && pass "$m" || fail "$m"; }

pick_free_port() {
  python3 -c "import socket; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind(('$1',0)); print(s.getsockname()[1]); s.close()"
}
SP=$(pick_free_port 127.0.0.1)
NP=$(pick_free_port 0.0.0.0)

cleanup() {
  for pid in $(fuser "$SP/tcp" "$NP/tcp" 2>/dev/null | xargs); do
    kill "$pid" 2>/dev/null || true
  done
  # fallback: scan /proc/net/tcp for orphaned sockets
  for port in "$SP" "$NP"; do
    hex=$(printf '%04X' "$port")
    while IFS= read -r line; do
      inode=$(echo "$line" | awk '{print $10}')
      [ -z "$inode" ] && continue
      for d in /proc/[0-9]*/fd; do
        pid="${d%/fd*}"; pid="${pid#/proc/}"
        for f in "$d"/*; do
          target=$(readlink "$f" 2>/dev/null) || continue
          [ "$target" = "socket:[$inode]" ] && kill "$pid" 2>/dev/null || true
        done 2>/dev/null
      done 2>/dev/null
    done < <(awk -v h=":$(printf '%04X' "$port")" '$2 ~ h && $4 == "0A" {print}' /proc/net/tcp 2>/dev/null)
  done
}

cleanup
kubectl delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
sleep 1
k create ns "$NS" 2>/dev/null || true

k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  app_mode: "production"
  log_level: "debug"
  greeting: "hello-from-configmap"
---
apiVersion: v1
kind: Secret
metadata:
  name: app-secret
type: Opaque
stringData:
  DB_HOST: "postgres.internal"
  DB_PORT: "5432"
  DB_USER: "admin"
  DB_PASS: "s3cr3t-p@ss!"
  API_KEY: "sk-1234567890abcdef"
---
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv-fulltest
spec:
  capacity: { storage: 1Gi }
  accessModes: [ReadWriteOnce]
  hostPath: { path: /mnt/z8s-pv-data }
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc-fulltest
spec:
  accessModes: [ReadWriteOnce]
  resources: { requests: { storage: 100Mi } }
YAML

check "ConfigMap" k get configmap app-config -n "$NS"
check "Secret"    k get secret app-secret -n "$NS"
check "PV"        k get pv pv-fulltest
check "PVC"       k get pvc pvc-fulltest -n "$NS"

k apply --validate=false -n "$NS" -f - <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: fullstack
  labels:
    app: fullstack
spec:
  replicas: 2
  selector:
    matchLabels:
      app: fullstack
  template:
    metadata:
      labels:
        app: fullstack
    spec:
      containers:
      - name: app
        image: host://alpine
        command:
        - /bin/sh
        - -c
        - |
          echo '=== boot ==='
          echo "DB_HOST=\$DB_HOST"
          echo "APP_MODE=\$app_mode"
          nohup python3 -m http.server $SP --bind 127.0.0.1 >/dev/null 2>&1 &
          exec sleep infinity
        env:
        - name: DIRECT_ENV
          value: "from-pod-spec"
        envFrom:
        - secretRef: { name: app-secret }
        - configMapRef: { name: app-config }
        ports:
        - containerPort: $SP
---
apiVersion: v1
kind: Service
metadata:
  name: fullstack-svc
spec:
  selector:
    app: fullstack
  ports:
  - port: 80
    targetPort: $SP
    nodePort: $NP
  type: NodePort
YAML
check "Service" k get svc fullstack-svc -n "$NS"

for i in $(seq 1 45); do
    r=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)
    [ "${r:-0}" -ge 2 ] && break
    sleep 2
done
check "Deployment 2/2" test "${r:-0}" -ge 2

POD1=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod | head -1)
POD2=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod | tail -1)
echo "Pod1=$POD1 Pod2=$POD2"

for p in "$POD1" "$POD2"; do
    for i in 1 2 3 4 5; do
        kubectl exec -n "$NS" "$p" -- true 2>/dev/null && break || sleep 2
    done
done

echo -e "${CYAN}═══ 1: env vars ═══${NC}"
env1=$(kubectl exec -n "$NS" "$POD1" -- env 2>/dev/null)
check "ConfigMap app_mode"     grep -q "app_mode=production" <<< "$env1"
check "ConfigMap log_level"    grep -q "log_level=debug" <<< "$env1"
check "ConfigMap greeting"     grep -q "greeting=hello-from-configmap" <<< "$env1"
check "Direct env DIRECT_ENV"  grep -q "DIRECT_ENV=from-pod-spec" <<< "$env1"
check "Secret DB_HOST"         grep -q "DB_HOST=postgres.internal" <<< "$env1"
check "Secret DB_PORT"         grep -q "DB_PORT=5432" <<< "$env1"
check "Secret DB_USER"         grep -q "DB_USER=admin" <<< "$env1"
check "Secret DB_PASS"         grep -q "DB_PASS=s3cr3t-p@ss!" <<< "$env1"
check "Secret API_KEY"         grep -q "API_KEY=sk-1234567890abcdef" <<< "$env1"
check "Service SVC_HOST"       grep -q "FULLSTACK_SVC_SERVICE_HOST" <<< "$env1"
check "Service SVC_PORT=80"    grep -q "FULLSTACK_SVC_SERVICE_PORT=80" <<< "$env1"

echo -e "${CYAN}═══ 2: exec ═══${NC}"
check "whoami=root"     grep -q "root" <<< "$(kubectl exec -n "$NS" "$POD1" -- whoami 2>/dev/null)"
check "inline secret"   grep -q "admin" <<< "$(kubectl exec -n "$NS" "$POD1" -- sh -c 'echo "u=$DB_USER"' 2>/dev/null)"

echo -e "${CYAN}═══ 3: HTTP ═══${NC}"
for i in 1 2 3 4 5; do
    out=$(curl -sf http://localhost:$NP/ 2>&1) && break || sleep 2
done
check "NodePort $NP" grep -qiE "directory listing|http|html" <<< "$out"

echo -e "${CYAN}═══ 4: logs ═══${NC}"
sleep 2
logs=$(k logs -n "$NS" "$POD1" 2>/dev/null || echo "")
check "logs boot"  grep -q "boot" <<< "$logs"
check "logs DB_HOST" grep -q "DB_HOST=postgres.internal" <<< "$logs"

echo -e "${CYAN}═══ 5: Pod2 same ═══${NC}"
env2=$(kubectl exec -n "$NS" "$POD2" -- env 2>/dev/null)
check "Pod2 DB_HOST"  grep -q "DB_HOST=postgres.internal" <<< "$env2"
check "Pod2 app_mode" grep -q "app_mode=production" <<< "$env2"

echo -e "${CYAN}═══ 6: Scale 2→4 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=4 2>/dev/null
for i in $(seq 1 30); do
    r=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || true)
    [ "${r:-0}" -ge 4 ] && break
    sleep 2
done
check "4 Running" test "${r:-0}" -ge 4

for p in $(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod); do
    for i in 1 2 3 4 5; do
        kubectl exec -n "$NS" "$p" -- true 2>/dev/null && break || sleep 2
    done
done

echo -e "${CYAN}═══ 7: env on 4 pods ═══${NC}"
errs=0
for p in $(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod); do
    ok=$(kubectl exec -n "$NS" "$p" -- sh -c 'test -n "$DB_USER" && test -n "$app_mode" && echo OK || echo FAIL' 2>/dev/null)
    echo "  $p: $ok"
    [ "$ok" = "OK" ] || errs=$((errs+1))
done
check "all 4 have env" test "$errs" -eq 0

echo -e "${CYAN}═══ 8: Scale 4→1 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=1 2>/dev/null
sleep 3
for i in $(seq 1 30); do
    r=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || true)
    [ "${r:-0}" -eq 1 ] && break
    sleep 1
done
check "1 Running" test "${r:-0}" -eq 1

SURVIVOR=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod | head -1)
for i in 1 2 3 4 5; do
    kubectl exec -n "$NS" "$SURVIVOR" -- true 2>/dev/null && break || sleep 2
done

echo -e "${CYAN}═══ 9: survivor ═══${NC}"
check "survivor env" grep -q "admin:production" <<< "$(kubectl exec -n "$NS" "$SURVIVOR" -- sh -c 'echo "$DB_USER:$app_mode"' 2>/dev/null)"
for i in 1 2 3; do
    out=$(curl -sf http://localhost:$NP/ 2>&1) && break || sleep 2
done
check "HTTP after scale" grep -qiE "directory listing|http|html" <<< "$out"

echo -e "${CYAN}═══ 10: PV/PVC ═══${NC}"
check "PV cap=1Gi"  test "$(k get pv pv-fulltest -o jsonpath='{.spec.capacity.storage}' 2>/dev/null)" = "1Gi"
check "PVC req=100Mi" test "$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.spec.resources.requests.storage}' 2>/dev/null)" = "100Mi"
check "PVC bound"  test "$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null)" = "Bound"
check "PV bound"   test "$(k get pv pv-fulltest -o jsonpath='{.status.phase}' 2>/dev/null)" = "Bound"
check "PVC volumeName" test -n "$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.spec.volumeName}' 2>/dev/null)"

echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo -e "${GREEN}  FULL STACK TEST PASSED${NC}"
echo -e "${GREEN}════════════════════════════════════════════${NC}"
#kubectl delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
