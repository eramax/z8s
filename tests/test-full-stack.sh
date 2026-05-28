#!/usr/bin/env bash
# full-stack integration test: configmap + secret + volume + deployment + service + scale + exec
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
NS="fulltest"

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; }
k()   { kubectl --server="$SERVER" "$@" 2>/dev/null; }

check() { local msg="$1"; shift; "$@" && pass "$msg" || fail "$msg"; }

# retry exec up to 5 times — pod may not accept exec immediately
kexec() {
    local ns="$1" pod="$2"; shift 2
    for i in 1 2 3 4 5; do
        out=$(k exec -n "$ns" "$pod" -- "$@" 2>/dev/null) && echo "$out" && return 0
        sleep 1
    done
    echo ""
    return 1
}

echo -e "${CYAN}═══ Setup ═══${NC}"
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
  app_mode: "production"
  log_level: "debug"
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
check "PV stored"  k get pv pv-fulltest
check "PVC stored" k get pvc pvc-fulltest -n "$NS"

echo -e "${CYAN}═══ Deployment (2 replicas) ═══${NC}"
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
          echo "DB_HOST=$DB_HOST" ; echo "APP_MODE=$APP_MODE"
          python3 -m http.server 9090 --bind 127.0.0.1
        env:
        - name: DIRECT_ENV
          value: "from-pod-spec"
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
    nodePort: 30091
  type: NodePort
YAML
check "Service stored" k get svc fullstack-svc -n "$NS"

echo -e "${CYAN}═══ Wait for 2/2 ready ═══${NC}"
deadline=$(( $(date +%s) + 90 ))
ready=0
while [[ $(date +%s) -lt $deadline ]]; do
    ready=$(k get deployment fullstack -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo "0")
    [[ "${ready:-0}" -ge 2 ]] && break
    sleep 2
done
[[ "${ready:-0}" -ge 2 ]] && pass "Deployment 2/2 ready" || fail "Deployment ready=$ready"
sleep 3

PODS=$(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' || echo "")
POD1=$(echo "$PODS" | head -1 | sed 's|pod/||')
POD2=$(echo "$PODS" | tail -1 | sed 's|pod/||')
echo "  Pod1=$POD1 Pod2=$POD2"

# ─── TEST 1: ConfigMap envFrom + Secret envFrom + Direct env + Exec ──────
echo -e "${CYAN}═══ Test: ConfigMap + Secret env ═══${NC}"
env1=$(kexec "$NS" "$POD1" sh -c env)
check "ConfigMap: APP_MODE=production"     grep -q "app_mode=production" <<< "$env1"
check "ConfigMap: LOG_LEVEL=debug"         grep -q "log_level=debug" <<< "$env1"
check "ConfigMap: GREETING"                grep -q "greeting=hello-from-configmap" <<< "$env1"
check "Direct env: DIRECT_ENV"             grep -q "DIRECT_ENV=from-pod-spec" <<< "$env1"
check "Secret: DB_HOST"                    grep -q "DB_HOST=postgres.internal" <<< "$env1"
check "Secret: DB_PORT=5432"               grep -q "DB_PORT=5432" <<< "$env1"
check "Secret: DB_USER=admin"              grep -q "DB_USER=admin" <<< "$env1"
check "Secret: DB_PASS"                    grep -q "DB_PASS=s3cr3t-p@ss!" <<< "$env1"
check "Secret: API_KEY"                    grep -q "API_KEY=sk-1234567890abcdef" <<< "$env1"
check "Service host injected"              grep -q "FULLSTACK_SVC_SERVICE_HOST" <<< "$env1"
check "Service port=80"                    grep -q "FULLSTACK_SVC_SERVICE_PORT=80" <<< "$env1"

# ─── TEST 2: HTTP via NodePort ────────────────────────────────────────────
echo -e "${CYAN}═══ Test: HTTP ═══${NC}"
out=$(curl -sf http://localhost:30091/ 2>&1) || out=""
check "NodePort HTTP responds" grep -qiE "directory listing|http|html" <<< "$out"

# ─── TEST 3: Exec commands ────────────────────────────────────────────────
echo -e "${CYAN}═══ Test: Exec ═══${NC}"
who=$(kexec "$NS" "$POD1" whoami)
check "whoami=root" grep -q "root" <<< "$who"
inline=$(kexec "$NS" "$POD1" sh -c 'echo "db=$DB_USER"')
check "inline secret ref" grep -q "db=admin" <<< "$inline"

# ─── TEST 4: Pod 2 same env ───────────────────────────────────────────────
echo -e "${CYAN}═══ Test: Pod 2 ═══${NC}"
env2=$(kexec "$NS" "$POD2" sh -c env)
check "Pod2: DB_HOST"  grep -q "DB_HOST=postgres.internal" <<< "$env2"
check "Pod2: APP_MODE" grep -q "app_mode=production" <<< "$env2"

# ─── TEST 5: Logs ─────────────────────────────────────────────────────────
echo -e "${CYAN}═══ Test: Logs ═══${NC}"
logs=$(kexec "$NS" "$POD1" sh -c 'cat /proc/1/fd/1 2>/dev/null; cat /proc/1/fd/2 2>/dev/null' 2>/dev/null || k logs -n "$NS" "$POD1" 2>/dev/null || echo "")
check "Logs show boot"    grep -q "boot" <<< "$logs"
check "Logs show DB_HOST" grep -q "DB_HOST=postgres.internal" <<< "$logs"

# ─── TEST 6: Scale 2→4 ───────────────────────────────────────────────────
echo -e "${CYAN}═══ Test: Scale 2→4 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=4 2>/dev/null
deadline=$(( $(date +%s) + 60 ))
while [[ $(date +%s) -lt $deadline ]]; do
    running=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || echo "0")
    [[ "${running:-0}" -ge 4 ]] && break
    sleep 2
done
check "Scale 2→4: 4 Running" test "${running:-0}" -ge 4

# ─── TEST 7: All 4 pods have Secret+ConfigMap ─────────────────────────────
echo -e "${CYAN}═══ Test: All pods inherit env ═══${NC}"
errs=0
for p in $(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' | sed 's|pod/||'); do
    res=$(kexec "$NS" "$p" sh -c 'test -n "$DB_USER" && test -n "$app_mode" && echo "OK" || echo "FAIL"')
    echo "    $p: $res"
    [[ "$res" != "OK" ]] && errs=$((errs+1))
done
check "All pods have Secret+ConfigMap" test "$errs" -eq 0

# ─── TEST 8: Scale 4→1 ──────────────────────────────────────────────────
echo -e "${CYAN}═══ Test: Scale 4→1 ═══${NC}"
k scale deployment fullstack -n "$NS" --replicas=1 2>/dev/null
deadline=$(( $(date +%s) + 30 ))
while [[ $(date +%s) -lt $deadline ]]; do
    running=$(k get pods -n "$NS" 2>/dev/null | grep 'fullstack-pod-' | grep -c 'Running' || echo "0")
    [[ "${running:-0}" -eq 1 ]] && break
    sleep 1
done
check "Scale 4→1: 1 Running" test "${running:-0}" -eq 1

# ─── TEST 9: Survivor after scale ────────────────────────────────────────
echo -e "${CYAN}═══ Test: Survivor ═══${NC}"
SURVIVOR=$(k get pods -n "$NS" -o name 2>/dev/null | grep 'fullstack-pod-' | head -1 | sed 's|pod/||')
surv=$(kexec "$NS" "$SURVIVOR" sh -c 'echo "$DB_USER:$app_mode"')
check "Survivor has env" grep -q "admin:production" <<< "$surv"
out=$(curl -sf http://localhost:30091/ 2>&1) || out=""
check "HTTP after scale" grep -qiE "directory listing|http|html" <<< "$out"

# ─── TEST 10: PV + PVC ════════════════════════════════════════════════════
echo -e "${CYAN}═══ Test: PV/PVC ═══${NC}"
pv_size=$(k get pv pv-fulltest -o jsonpath='{.spec.capacity.storage}' 2>/dev/null || echo "")
check "PV capacity=1Gi" test "$pv_size" = "1Gi"
pvc_size=$(k get pvc pvc-fulltest -n "$NS" -o jsonpath='{.spec.resources.requests.storage}' 2>/dev/null || echo "")
check "PVC request=100Mi" test "$pvc_size" = "100Mi"

echo ""
echo -e "${GREEN}════════════════════════════════════════════${NC}"
echo -e "${GREEN}  FULL STACK TEST COMPLETE${NC}"
echo -e "${GREEN}════════════════════════════════════════════${NC}"

kubectl --server="$SERVER" delete ns "$NS" --ignore-not-found --wait=false 2>/dev/null || true
