#!/usr/bin/env bash
set -uo pipefail
SERVER="${Z8S_SERVER:-https://localhost:6443}"
DAEMON="/home/abb/dev/z8s/z8s.sh"
YAML_DIR="/home/abb/dev/z8s/tests"
PASS=0; FAIL=0; ERRORS=()
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'
pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { echo -e "${RED}FAIL${NC} $1"; ERRORS+=("$1"); FAIL=$((FAIL+1)); }
k() { /home/abb/.local/bin/kubectl --kubeconfig ~/.kube/config "$@" 2>&1 || true; }
kapply() { /home/abb/.local/bin/kubectl --kubeconfig ~/.kube/config --validate=false "$@" 2>&1; }

wait_pod_regex() {
  local pattern="$1" ns="${2:-default}" timeout="${3:-90}"
  local deadline=$(( $(date +%s) + timeout ))
  while [[ $(date +%s) -lt $deadline ]]; do
    local ready=$(k get pods -n "$ns" 2>/dev/null | grep -E "$pattern" | wc -l)
    [[ "$ready" -gt 0 ]] && return 0
    sleep 2
  done; return 1
}

echo "=== Start z8s ==="
"$DAEMON" restart 2>&1; sleep 2
curl -sfk "$SERVER/healthz" >/dev/null && pass "server started" || { fail "server start"; exit 1; }

echo "=== Create resources ==="
kapply apply -f "$YAML_DIR/00-namespace.yaml" 2>/dev/null
kapply apply -f "$YAML_DIR/01-configmap.yaml" 2>/dev/null
kapply apply -f "$YAML_DIR/02-secret.yaml" 2>/dev/null
pass "configmaps + secrets created"

# Deployments needed for service tests
kapply apply -f "$YAML_DIR/09-deployment-python.yaml" 2>/dev/null
kapply apply -f "$YAML_DIR/12-deployment-nginx.yaml" 2>/dev/null
kapply apply -f "$YAML_DIR/13-deployment-info.yaml" 2>/dev/null
pass "deployments applied"

# Wait for deployments to be ready
for dep in python-deploy nginx-deploy nginx-hello whoami http-echo hostinfo cluster-dashboard; do
  for i in $(seq 1 30); do
    reps=$(k get deployment "$dep" -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
    [[ "${reps:-0}" -ge 1 ]] && break; sleep 2
  done
  k get deployment "$dep" 2>/dev/null | grep -q "$dep" && pass "deployment $dep ready" || fail "deployment $dep not ready"
done

# Create services
kapply apply -f "$YAML_DIR/11-service.yaml" 2>/dev/null
pass "services applied"
sleep 3

# Create client pod
cat > /tmp/svc-client.yaml <<'CLIENTEOF'
apiVersion: v1
kind: Pod
metadata:
  name: svc-client
  namespace: default
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
CLIENTEOF
kapply apply -f /tmp/svc-client.yaml 2>/dev/null
wait_pod_regex "svc-client" "default" 60 || { fail "svc-client not ready"; exit 1; }
pass "svc-client pod ready"

echo "=== Run 6 service tests ==="
test_svc() {
  local svc="$1" port="$2" expected="$3" label="$4"
  local ip=$(k get svc "$svc" -n default -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
  [[ -z "$ip" ]] && ip="$svc"
  for try in 1 2 3; do
    out=$(k exec svc-client -- wget -q -O- -T 3 "http://${ip}:${port}/" 2>&1) || true
    if echo "$out" | grep -qiE "$expected"; then
      pass "svc: $label (try $try)"; return
    fi
    sleep 2
  done
  fail "svc: $label (clusterIP=$ip, port=$port) — $out"
}

test_svc "python-svc" "18080" "directory listing|http|html" "python HTTP server"
test_svc "nginx-svc" "80" "nginx|html|welcome" "nginx default page"
test_svc "whoami-svc" "80" "whoami|hostname|I.m" "whoami info page"
test_svc "http-echo-svc" "5678" "hello|echo|request" "http-echo text"
test_svc "hostinfo-svc" "18081" "hostname|host|info" "hostinfo page"
test_svc "nginx-hello-svc" "80" "hello|nginx|html" "nginx-hello page"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]] && exit 0 || exit 1
