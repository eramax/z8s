#!/usr/bin/env bash
# Fast regression tests for recent z8s networking + deployment fixes.
# Targets: deployment/alpine-pod lifecycle, per-pod netns + port publish, ClusterIP proxy.
#
# Usage:
#   cargo build && ./z8s.sh stop && ./z8s.sh start   # restart after code changes
#   ./tests/run-network-fixes.sh
#   Z8S_SERVER=http://127.0.0.1:6443 ./tests/run-network-fixes.sh
#
# Expect ~1–3 minutes (not the full 10-minute suite).
set -eo pipefail

SERVER="${Z8S_SERVER:-https://localhost:6443}"
NS="z8s-fixtest"
PASS=0
FAIL=0
ERRORS=()

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS + 1)); }
fail() {
  local m="$1" d="${2:-}"
  echo -e "${RED}FAIL${NC} $m${d:+: $d}"
  ERRORS+=("$m${d:+: $d}")
  FAIL=$((FAIL + 1))
}
section() { echo -e "\n${YELLOW}══ $1 ══${NC}"; }
sub() { echo -e "${CYAN}  ▸ $1${NC}"; }

k() { kubectl --kubeconfig ~/.kube/config "$@" 2>&1; }
kapply() { kubectl --kubeconfig ~/.kube/config "$@" 2>&1; }  # caller checks exit status when needed

wait_pod_ready() {
  local name="$1" timeout="${2:-45}"
  local deadline=$(( $(date +%s) + timeout ))
  while [[ $(date +%s) -lt $deadline ]]; do
    local phase ready
    phase=$(k get pod "$name" -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null) || true
    ready=$(k get pod "$name" -n "$NS" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null) || true
    if [[ "$phase" == "Running" && "$ready" == "true" ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_deploy_ready() {
  local name="$1" replicas="${2:-1}" timeout="${3:-60}"
  local deadline=$(( $(date +%s) + timeout ))
  while [[ $(date +%s) -lt $deadline ]]; do
    local ready
    ready=$(k get deployment "$name" -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null) || ready=""
    ready="${ready:-0}"
    [[ "$ready" =~ ^[0-9]+$ ]] || ready=0
    if (( ready >= replicas )); then
      return 0
    fi
    sleep 1
  done
  return 1
}

http_via_clusterip() {
  [[ "$HAVE_CLIENT" -eq 1 ]] || return 1
  local svc="$1" port="$2" pattern="$3"
  local ip out try
  ip=$(k get svc "$svc" -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null) || true
  [[ -z "$ip" || "$ip" == "None" ]] && return 1
  for try in 1 2 3 4 5; do
    out=$(k exec svc-client -n "$NS" -- wget -q -O- -T 5 "http://${ip}:${port}/" 2>&1) || true
    if echo "$out" | grep -qiE "$pattern"; then
      return 0
    fi
    sleep 2
  done
  return 1
}

cleanup() {
  k delete namespace "$NS" --wait=false --ignore-not-found >/dev/null 2>&1 || true
}

trap cleanup EXIT

# If svc-client is not ready, later service checks are skipped but other tests still run.
HAVE_CLIENT=0

section "Setup"
if ! k get --raw=/healthz 2>/dev/null | grep -q ok; then
  fail "z8s API" "not reachable at $SERVER (start with ./z8s.sh start)"
  echo -e "\n Results: ${PASS} passed, ${FAIL} failed"
  exit 1
fi
pass "z8s API healthy"

cleanup
sleep 2
if ! kapply apply --validate=false -f - <<EOF >/dev/null
apiVersion: v1
kind: Namespace
metadata:
  name: ${NS}
EOF
then
  fail "namespace ${NS}" "kubectl apply failed"
  exit 1
fi
sleep 1
pass "namespace ${NS} created"

# Shared wget client (host network — no containerPort, no isolated netns)
kapply apply --validate=false -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: svc-client
  namespace: ${NS}
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
EOF
if wait_pod_ready svc-client 90; then
  pass "svc-client ready"
  HAVE_CLIENT=1
else
  fail "svc-client" "not ready (service HTTP tests will be skipped)"
fi

# ── 1. Deployment must not adopt/delete standalone pods ─────────────────────
section "1. Deployment vs standalone pod (alpine-pod lifecycle)"

kapply apply --validate=false -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: alpine-standalone
  namespace: ${NS}
  labels:
    app: alpine-fix
    type: fixtest
spec:
  containers:
  - name: alpine
    image: alpine:latest
    command: ["sleep", "infinity"]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: alpine-fix-deploy
  namespace: ${NS}
spec:
  replicas: 2
  selector:
    matchLabels:
      app: alpine-fix
  template:
    metadata:
      labels:
        app: alpine-fix
    spec:
      containers:
      - name: alpine
        image: alpine:latest
        command: ["sleep", "infinity"]
EOF

if wait_deploy_ready alpine-fix-deploy 2 45; then
  pass "alpine-fix-deploy 2/2 ready"
else
  fail "alpine-fix-deploy" "not ready in time"
fi

if k get pod alpine-standalone -n "$NS" -o name >/dev/null 2>&1; then
  pass "standalone alpine-standalone still exists (not deleted as excess)"
else
  fail "alpine-standalone" "missing — deployment likely adopted/deleted it"
fi

managed_count=$(k get pods -n "$NS" -o name 2>/dev/null | grep -c 'alpine-fix-deploy-pod-' || true)
ready=$(k get deployment alpine-fix-deploy -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null) || true
ready="${ready:-0}"
[[ "$ready" =~ ^[0-9]+$ ]] || ready=0
if [[ "$managed_count" -eq 2 && "$ready" -le 2 ]]; then
  pass "deployment has 2 managed pods; readyReplicas=${ready} (standalone not inflated)"
elif [[ "$ready" -gt "$managed_count" && "$managed_count" -eq 2 ]]; then
  fail "readyReplicas" "got ${ready} but only ${managed_count} managed pods — status counts standalone/extra"
else
  fail "readyReplicas" "managed=${managed_count} readyReplicas=${ready}"
fi

out=$(k get pod alpine-standalone -n "$NS" -o jsonpath='{.metadata.name}' 2>/dev/null) || true
if [[ "$out" == "alpine-standalone" ]]; then
  pass "get pod -o jsonpath on standalone"
else
  fail "get pod jsonpath" "$out"
fi

# ── 2. Two workloads both on containerPort 80 ───────────────────────────────
section "2. Multiple pods on port 80 (isolated netns + published backends)"

kapply apply --validate=false -f - >/dev/null <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-a
  namespace: ${NS}
spec:
  replicas: 1
  selector:
    matchLabels:
      app: web-a
  template:
    metadata:
      labels:
        app: web-a
    spec:
      containers:
      - name: http
        image: python:3-alpine
        command: ["python3", "-m", "http.server", "80", "--bind", "0.0.0.0"]
        ports:
        - containerPort: 80
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-b
  namespace: ${NS}
spec:
  replicas: 1
  selector:
    matchLabels:
      app: web-b
  template:
    metadata:
      labels:
        app: web-b
    spec:
      containers:
      - name: http
        image: python:3-alpine
        command: ["python3", "-m", "http.server", "80", "--bind", "0.0.0.0"]
        ports:
        - containerPort: 80
---
apiVersion: v1
kind: Service
metadata:
  name: web-a-svc
  namespace: ${NS}
spec:
  selector:
    app: web-a
  ports:
  - port: 80
    targetPort: 80
---
apiVersion: v1
kind: Service
metadata:
  name: web-b-svc
  namespace: ${NS}
spec:
  selector:
    app: web-b
  ports:
  - port: 80
    targetPort: 80
EOF

wait_deploy_ready web-a 1 90 || true
wait_deploy_ready web-b 1 90 || true
sleep 3

sub "ClusterIP → nginx"
if http_via_clusterip web-a-svc 80 'html|Directory|http'; then
  pass "web-a-svc returns HTTP (python http.server on :80)"
else
  fail "web-a-svc" "no HTTP response via ClusterIP"
fi

sub "ClusterIP → whoami"
if http_via_clusterip web-b-svc 80 'html|Directory|http'; then
  pass "web-b-svc returns HTTP (second workload on :80)"
else
  fail "web-b-svc" "no HTTP response via ClusterIP"
fi

# ── 3. Python on 18080 without containerPort (host network) ─────────────────
section "3. Python pod without containerPort (host listener 18080)"

kapply apply --validate=false -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: python-fix
  namespace: ${NS}
  labels:
    app: python-fix
spec:
  containers:
  - name: python
    image: python:3-slim
    command: ["/bin/sh", "-c"]
    args:
    - |
      exec python3 -c "
      from http.server import HTTPServer, SimpleHTTPRequestHandler
      HTTPServer(('127.0.0.1', 18080), SimpleHTTPRequestHandler).serve_forever()
      "
---
apiVersion: v1
kind: Service
metadata:
  name: python-fix-svc
  namespace: ${NS}
spec:
  selector:
    app: python-fix
  ports:
  - port: 18080
    targetPort: 18080
EOF

if wait_pod_ready python-fix 60; then
  pass "python-fix pod ready"
else
  fail "python-fix" "pod not ready"
fi

sleep 2
if http_via_clusterip python-fix-svc 18080 'directory|html|Index|http'; then
  pass "python-fix-svc reachable on 18080"
else
  fail "python-fix-svc" "no HTTP response on ClusterIP:18080"
fi

# ── 4. Exec smoke (non-isolated alpine) ─────────────────────────────────────
section "4. Exec smoke"

if k get pod alpine-standalone -n "$NS" >/dev/null 2>&1; then
  out=$(k exec alpine-standalone -n "$NS" -- /bin/sh -c 'echo exec-ok' 2>&1) || true
  if echo "$out" | grep -q exec-ok; then
    pass "exec into non-isolated alpine"
  else
    fail "exec alpine-standalone" "$out"
  fi
fi

# ── Summary ─────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════"
echo " Results: ${PASS} passed, ${FAIL} failed"
if [[ $FAIL -gt 0 ]]; then
  echo ""
  echo " Failures:"
  for e in "${ERRORS[@]}"; do
    echo "  ✗ $e"
  done
  echo "════════════════════════════════════════════"
  echo " Log: /tmp/z8s.log"
  exit 1
fi
echo "════════════════════════════════════════════"
exit 0
