#!/usr/bin/env bash
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
NS="fulltest"

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; }
k()   { kubectl --server="$SERVER" "$@"; }

check() { local msg="$1"; shift; "$@" && pass "$msg" || fail "$msg"; }

wait_exec_ready() {
    local ns="$1" pod="$2"
    for i in 1 2 3 4 5; do
        kubectl --server="$SERVER" exec -n "$ns" "$pod" -- true 2>/dev/null && return 0
        sleep 2
    done
    return 1
}

kexec() {
    local ns="$1" pod="$2"; shift 2
    kubectl --server="$SERVER" exec -n "$ns" "$pod" -- "$@" 2>/dev/null
}

echo -e "${CYAN}═══ Setup ═══${NC}"
kubectl --server="$SERVER" delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
sleep 2
kubectl --server="$SERVER" create ns "$NS" 2>/dev/null || true

echo -e "${CYAN}═══ ConfigMap + Secret + PV/PVC ═══${NC}"
k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  app_mode: "production"
  log_level: "debug"
  greeting: "hello-from-configmap"
YAML
k apply --validate=false -n "$NS" -f - <<'YAML'
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
YAML
k apply --validate=false -f - <<'YAML'
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv-fulltest
spec:
  capacity: { storage: 1Gi }
  accessModes: [ReadWriteOnce]
  hostPath: { path: /mnt/z8s-pv-data }
YAML
k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc-fulltest
spec:
  accessModes: [ReadWriteOnce]
  resources: { requests: { storage: 100Mi } }
YAML

check "ConfigMap stored" k get configmap app-config -n "$NS"
check "Secret stored"    k get secret app-secret -n "$NS"
check "PV stored"        k get pv pv-fulltest
check "PVC stored"       k get pvc pvc-fulltest -n "$NS"

echo -e "${CYAN}═══ Deployment + Service ═══${NC}"
k apply --validate=false -n "$NS" -f - <<'YAML'
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
        command: ["/bin/sh", "-c", "echo '=== boot ==='; echo \"DB_HOST=$DB_HOST\"; python3 -m http.server 9090 --bind 127.0.0.1"]
        env:
        - name: DIRECT_ENV
          value: "from-pod-spec"
        envFrom:
        - secretRef: { name: app-secret }
        - configMapRef: { name: app-config }
        ports:
        - containerPort: 9090
YAML
k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: fullstack-svc
spec:
  selector:
    app: fullstack
  ports:
  - port: 80
    targetPort: 9090
    nodePort: 30091
  type: NodePort
YAML
check "Service stored" k get svc fullstack-svc -n "$NS"

echo -e "${CYAN}═══ Wait for 2/2 ═══${NC}"
deadline=$(( $(date +%s) + 90 ))
while [[ $(date +%s) -lt $deadline ]]; do
    r=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    [[ "${r:-0}" -ge 2 ]] && break
    sleep 2
done
check "Deployment 2/2 ready" test "${r:-0}" -ge 2

POD1=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/fullstack-pod-||p' | head -1) && POD1="fullstack-pod-$POD1"
POD2=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/fullstack-pod-||p' | tail -1) && POD2="fullstack-pod-$POD2"
echo "  Pod1=$POD1 Pod2=$POD2"

# Wait for exec to be ready
wait_exec_ready "$NS" "$POD1" || fail "Pod1 never accepted exec"
wait_exec_ready "$NS" "$POD2" || fail "Pod2 never accepted exec"

echo -e "${CYAN}═══ Test: env vars ═══${NC}"
env1=$(kexec "$NS" "$POD1" env)
check "ConfigMap app_mode=production"         grep -q "app_mode=production" <<< "$env1"
check "ConfigMap log_level=debug"             grep -q "log_level=debug" <<< "$env1"
check "ConfigMap greeting"                    grep -q "greeting=hello-from-configmap" <<< "$env1"
check "Direct env DIRECT_ENV"                 grep -q "DIRECT_ENV=from-pod-spec" <<< "$env1"
check "Secret DB_HOST"                       grep -q "DB_HOST=postgres.internal" <<< "$env1"
check "Secret DB_PORT"                       grep -q "DB_PORT=5432" <<< "$env1"
check "Secret DB_USER"                       grep -q "DB_USER=admin" <<< "$env1"
check "Secret DB_PASS"                       grep -q "DB_PASS=s3cr3t-p@ss!" <<< "$env1"
check "Secret API_KEY"                       grep -q "API_KEY=sk-1234567890abcdef" <<< "$env1"
check "Service FULLSTACK_SVC_SERVICE_HOST"   grep -q "FULLSTACK_SVC_SERVICE_HOST" <<< "$env1"
check "Service FULLSTACK_SVC_SERVICE_PORT=80" grep -q "FULLSTACK_SVC_SERVICE_PORT=80" <<< "$env1"

echo -e "${CYAN}═══ Test: exec ═══${NC}"
check "whoami=root" grep -q "root" <<< "$(kexec "$NS" "$POD1" whoami)"
check "inline secret ref" grep -q "admin" <<< "$(kexec "$NS" "$POD1" sh -c 'echo "db_user=$DB_USER"')"

echo -e "${CYAN}═══ Test: HTTP ═══${NC}"
for i in 1 2 3 4 5; do
    out=$(curl -sf http://localhost:30091/ 2>&1) && break || sleep 2
done
check "NodePort HTTP responds" grep -qiE "directory listing|http|html" <<< "$out"

echo -e "${CYAN}═══ Test: logs ═══${NC}"
logs=$(k logs -n "$NS" "$POD1" 2>/dev/null || echo "")
check "Logs show boot"    grep -q "boot" <<< "$logs"
check "Logs show DB_HOST" grep -q "DB_HOST=postgres.internal" <<< "$logs"

echo -e "${CYAN}═══ Test: Pod 2 same env ═══${NC}"
env2=$(kexec "$NS" "$POD2" env)
check "Pod2 DB_HOST"  grep -q "DB_HOST=postgres.internal" <<< "$env2"
check "Pod2 app_mode" grep -q "app_mode=production" <<< "$env2"

echo -e "${CYAN}═══ Test: Scale 2→4 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=4 2>/dev/null
deadline=$(( $(date +%s) + 60 ))
while [[ $(date +%s) -lt $deadline ]]; do
    running=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || true)
    [[ "$running" -ge 4 ]] 2>/dev/null && break
    sleep 2
done
check "4 pods Running" test "${running:-0}" -ge 4 2>/dev/null

echo -e "${CYAN}═══ Test: env on all 4 pods ═══${NC}"
errs=0
for p in $(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod); do
    ok=$(kexec "$NS" "$p" sh -c 'test -n "$DB_USER" && test -n "$app_mode" && echo OK || echo FAIL')
    echo "    $p: $ok"
    [[ "$ok" == "OK" ]] || errs=$((errs+1))
done
check "All pods have Secret+ConfigMap" test "$errs" -eq 0

echo -e "${CYAN}═══ Test: Scale 4→1 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=1 2>/dev/null
sleep 3
deadline=$(( $(date +%s) + 30 ))
while [[ $(date +%s) -lt $deadline ]]; do
    running=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || true)
    [[ "$running" -eq 1 ]] 2>/dev/null && break
    sleep 1
done
check "1 pod Running" test "${running:-0}" -eq 1 2>/dev/null

SURVIVOR=$(k get pods -n "$NS" -o name 2>/dev/null | sed -n 's|pod/||p' | grep fullstack-pod | head -1)
echo -e "${CYAN}═══ Test: survivor post-scale ═══${NC}"
check "survivor has env" grep -q "admin:production" <<< "$(kexec "$NS" "$SURVIVOR" sh -c 'echo "$DB_USER:$app_mode"')"
for i in 1 2 3; do
    out=$(curl -sf http://localhost:30091/ 2>&1) && break || sleep 2
done
check "HTTP after scale" grep -qiE "directory listing|http|html" <<< "$out"

echo -e "${CYAN}═══ Test: PV/PVC API ═══${NC}"
check "PV capacity 1Gi"   test "$(k get pv pv-fulltest -o jsonpath='{.spec.capacity.storage}' 2>/dev/null)" = "1Gi"
check "PVC request 100Mi" test "$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.spec.resources.requests.storage}' 2>/dev/null)" = "100Mi"

echo ""
echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo -e "${GREEN}  FULL STACK TEST COMPLETE${NC}"
echo -e "${GREEN}════════════════════════════════════════════${NC}"

kubectl --server="$SERVER" delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
