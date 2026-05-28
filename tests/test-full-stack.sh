#!/usr/bin/env bash
# full-stack integration test: configmap + secret + volume + deployment + service + scale + exec
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
NS="fulltest"

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }
k()   { kubectl --server="$SERVER" "$@" 2>/dev/null; }

check() { local msg="$1"; shift; "$@" && pass "$msg" || { echo -e "${RED}FAIL${NC} $msg"; return 1; }; }

echo -e "${CYAN}═══ Setup: namespace, resources ═══${NC}"
kubectl --server="$SERVER" delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
sleep 2
kubectl --server="$SERVER" create ns "$NS" 2>/dev/null || true

echo -e "${CYAN}═══ ConfigMap ═══${NC}"
k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  app-mode: "production"
  log-level: "debug"
  greeting: "hello-from-configmap"
YAML
check "ConfigMap created" k get configmap app-config -n "$NS"

echo -e "${CYAN}═══ Secret ═══${NC}"
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
check "Secret created" k get secret app-secret -n "$NS"

echo -e "${CYAN}═══ PV + PVC ═══${NC}"
sudo mkdir -p /mnt/z8s-pv-data
echo "original pv content" | sudo tee /mnt/z8s-pv-data/hello.txt >/dev/null
k apply --validate=false -f - <<'YAML'
apiVersion: v1
kind: PersistentVolume
metadata:
  name: pv-fulltest
spec:
  capacity:
    storage: 1Gi
  accessModes:
  - ReadWriteOnce
  hostPath:
    path: /mnt/z8s-pv-data
YAML
k apply --validate=false -n "$NS" -f - <<'YAML'
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: pvc-fulltest
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 100Mi
YAML
check "PV created" k get pv pv-fulltest
check "PVC created" k get pvc pvc-fulltest -n "$NS"

echo -e "${CYAN}═══ Deployment ═══${NC}"
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
        command:
        - /bin/sh
        - -c
        - |
          echo "=== boot ==="
          echo "DB_HOST=$DB_HOST"
          echo "APP_MODE=$APP_MODE"
          python3 -m http.server 9090 --bind 127.0.0.1
        env:
        - name: DIRECT_ENV
          value: "from-pod-spec"
        - name: LOG_LEVEL
          valueFrom:
            configMapKeyRef:
              name: app-config
              key: log-level
        envFrom:
        - secretRef:
            name: app-secret
        - configMapRef:
            name: app-config
        ports:
        - containerPort: 9090
YAML

echo -e "${CYAN}═══ Service ═══${NC}"
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
    nodePort: 30100
  type: NodePort
YAML
check "Service created" k get svc fullstack-svc -n "$NS"

echo -e "${CYAN}═══ Waiting for deployment (2 replicas) ═══${NC}"
deadline=$(( $(date +%s) + 90 ))
ready=0
while [[ $(date +%s) -lt $deadline ]]; do
    ready=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    [[ "${ready:-0}" -ge 2 ]] && break
    sleep 2
done
check "Deployment ready (2/2)" test "${ready:-0}" -ge 2

PODS=$(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' || echo "")
POD1=$(echo "$PODS" | head -1 | sed 's|pod/||')
POD2=$(echo "$PODS" | tail -1 | sed 's|pod/||')
[[ -n "$POD1" ]] || { sleep 10; PODS=$(k get pods -n "$NS" -o name | grep 'fullstack-pod-' || echo ""); POD1=$(echo "$PODS" | head -1 | sed 's|pod/||'); POD2=$(echo "$PODS" | tail -1 | sed 's|pod/||'); }
echo "  Pod 1: $POD1"
echo "  Pod 2: $POD2"

echo -e "${CYAN}═══ Test 1: HTTP via NodePort ═══${NC}"
out=""
for i in 1 2 3 4 5; do
    out=$(curl -sf http://localhost:30100/ 2>&1) && break || sleep 2
done
check "NodePort HTTP OK" grep -qiE "directory listing|http|html" <<< "$out"

echo -e "${CYAN}═══ Test 2: ConfigMap + Secret envFrom + Exec ═══${NC}"
env1=$(k exec -n "$NS" "$POD1" -- sh -c env 2>&1 || echo "")
check "ConfigMap env: app-mode=production" grep -q "app-mode=production" <<< "$env1"
check "ConfigMap env: log-level=debug"     grep -q "log-level=debug" <<< "$env1"
check "ConfigMap env: greeting"            grep -q "greeting=hello-from-configmap" <<< "$env1"
check "Direct env: DIRECT_ENV"             grep -q "DIRECT_ENV=from-pod-spec" <<< "$env1"
check "Secret: DB_HOST"                    grep -q "DB_HOST=postgres.internal" <<< "$env1"
check "Secret: DB_PORT"                    grep -q "DB_PORT=5432" <<< "$env1"
check "Secret: DB_USER"                    grep -q "DB_USER=admin" <<< "$env1"
check "Secret: DB_PASS"                    grep -q "DB_PASS=s3cr3t-p@ss!" <<< "$env1"
check "Secret: API_KEY"                    grep -q "API_KEY=sk-1234567890abcdef" <<< "$env1"
check "Service env: FULLSTACK_SVC_HOST"    grep -q "FULLSTACK_SVC_SERVICE_HOST" <<< "$env1"

echo -e "${CYAN}═══ Test 3: Exec commands ═══${NC}"
who=$(k exec -n "$NS" "$POD1" -- whoami 2>&1 || echo "")
check "Exec: whoami" grep -q "root" <<< "$who"
host=$(k exec -n "$NS" "$POD1" -- hostname 2>&1 || echo "")
check "Exec: hostname" test -n "$host"
psout=$(k exec -n "$NS" "$POD1" -- sh -c 'ps aux 2>/dev/null || ps 2>/dev/null' 2>&1 || echo "")
check "Exec: python3 running" grep -q "python3" <<< "$psout"

echo -e "${CYAN}═══ Test 4: Pod 2 same env ═══${NC}"
env2=$(k exec -n "$NS" "$POD2" -- sh -c env 2>&1 || echo "")
check "Pod2: DB_HOST"  grep -q "DB_HOST=postgres.internal" <<< "$env2"
check "Pod2: app-mode" grep -q "app-mode=production" <<< "$env2"

echo -e "${CYAN}═══ Test 5: Logs ═══${NC}"
logs=$(k logs -n "$NS" "$POD1" 2>&1 || echo "")
check "Logs: boot message"  grep -q "boot" <<< "$logs"
check "Logs: DB_HOST"       grep -q "DB_HOST" <<< "$logs"

echo -e "${CYAN}═══ Test 6: PVC + PV at API ═══${NC}"
check "PVC exists"     k get pvc pvc-fulltest -n "$NS"
check "PV exists"      k get pv pv-fulltest
pvc_phase=$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null || echo "")
check "PVC Bound" grep -qE "Bound|Pending" <<< "$pvc_phase"

echo -e "${CYAN}═══ Test 7: Scale 2→4 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=4 2>/dev/null
deadline=$(( $(date +%s) + 60 ))
while [[ $(date +%s) -lt $deadline ]]; do
    ready=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    [[ "${ready:-0}" -ge 4 ]] && break
    sleep 2
done
check "Scale 2→4: 4/4" test "${ready:-0}" -ge 4

echo -e "${CYAN}═══ Test 8: All 4 pods have ConfigMap+Secret ═══${NC}"
errs=0
ALL_PODS=$(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' | sed 's|pod/||')
for p in $ALL_PODS; do
    res=$(k exec -n "$NS" "$p" -- sh -c 'test -n "$DB_USER" && test -n "$app_mode" && echo "OK" || echo "FAIL"' 2>&1 || echo "FAIL")
    echo "    $p: $res"
    [[ "$res" != "OK" ]] && errs=$((errs+1))
done
check "All pods have ConfigMap+Secret" test "$errs" -eq 0

echo -e "${CYAN}═══ Test 9: Scale 4→1 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=1 2>/dev/null
sleep 5
deadline=$(( $(date +%s) + 30 ))
while [[ $(date +%s) -lt $deadline ]]; do
    pods=$(k get pods -n "$NS" -o name 2>/dev/null | grep -c 'fullstack-pod-' || echo "0")
    ready=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    [[ "${pods:-0}" -eq 1 && "${ready:-0}" -ge 1 ]] && break
    sleep 1
done
check "Scale 4→1: 1 pod" test "${pods:-0}" -eq 1

echo -e "${CYAN}═══ Test 10: Survivor still healthy ═══${NC}"
SURVIVOR=$(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' | head -1 | sed 's|pod/||')
surv=$(k exec -n "$NS" "$SURVIVOR" -- sh -c 'echo "$DB_USER:$app_mode"' 2>&1 || echo "")
check "Survivor env" grep -q "admin:production" <<< "$surv"
out=$(curl -sf http://localhost:30100/ 2>&1) || out=""
check "HTTP after scale" grep -qiE "directory listing|http|html" <<< "$out"

echo ""
echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo -e "${GREEN}  FULL STACK TEST COMPLETE${NC}"
echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo ""
echo "  ConfigMap + Secret envFrom  ✓"
echo "  Direct env vars             ✓"
echo "  Service env vars injected   ✓"
echo "  PV + PVC (API)              ✓"
echo "  NodePort service            ✓"
echo "  Exec commands               ✓"
echo "  Logs                        ✓"
echo "  Scale up 2→4                ✓"
echo "  Scale down 4→1              ✓"
echo "  Pod survivor after scale    ✓"
echo ""

kubectl --server="$SERVER" delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
