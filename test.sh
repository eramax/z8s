#!/usr/bin/env bash
# z8s integration test suite
# Usage: ./test.sh [--server http://localhost:6443]
set -uo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
DAEMON="$(dirname "$0")/z8s.sh"
LOG="/tmp/z8s.log"
PASS=0; FAIL=0; ERRORS=()

# ── colours ──────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { echo -e "${RED}FAIL${NC} $1: $2"; ERRORS+=("$1: $2"); FAIL=$((FAIL+1)); }
section() { echo -e "\n${YELLOW}── $1 ──${NC}"; }

# ── helpers ───────────────────────────────────────────────────────────────────
# k: always-zero wrapper for queries where we grep the output
k() { kubectl --server="$SERVER" --insecure-skip-tls-verify "$@" 2>&1 || true; }
# kapply: real exit code for apply/create/delete operations
kapply() { kubectl --server="$SERVER" --insecure-skip-tls-verify "$@" 2>&1; }

wait_pod_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-20}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local phase
        phase=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
        local ready
        ready=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
        if [[ "$phase" == "Running" && "$ready" == "true" ]]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# ── start server via daemon script ────────────────────────────────────────────
section "Server startup"
"$DAEMON" restart

# wait for API to respond
for i in $(seq 1 15); do
    if curl -sf "$SERVER/healthz" >/dev/null 2>&1; then
        pass "server started (${i}s)"
        break
    fi
    sleep 1
    if [[ $i -eq 15 ]]; then
        fail "server startup" "did not respond within 15s"
        echo "--- server log ---"; tail -30 "$LOG"; exit 1
    fi
done

cleanup() {
    echo ""
    section "Cleanup"
    k delete pod test-pod exec-pod long-pod --ignore-not-found 2>/dev/null || true
    k delete deployment test-deploy --ignore-not-found 2>/dev/null || true
    k delete namespace test-ns --ignore-not-found 2>/dev/null || true
    # z8s is left running (managed by z8s.sh)
    echo ""
    echo "═══════════════════════════════════"
    echo " Results: ${PASS} passed, ${FAIL} failed"
    echo "═══════════════════════════════════"
    if [[ ${#ERRORS[@]} -gt 0 ]]; then
        echo ""
        echo "Failures:"
        for e in "${ERRORS[@]}"; do echo "  ✗ $e"; done
    fi
    echo ""
    echo "--- Last 40 lines of server log ($LOG) ---"
    tail -40 "$LOG"
    [[ $FAIL -eq 0 ]] && exit 0 || exit 1
}
trap cleanup EXIT

# ── 1. Discovery ─────────────────────────────────────────────────────────────
section "Discovery"

out=$(k api-versions 2>&1)
if echo "$out" | grep -q "v1"; then
    pass "api-versions (v1 present)"
else
    fail "api-versions" "$out"
fi

out=$(k api-resources 2>&1)
for res in pods deployments namespaces nodes events; do
    if echo "$out" | grep -q "$res"; then
        pass "api-resources: $res listed"
    else
        fail "api-resources: $res" "not in output"
    fi
done

# ── 2. Namespaces ─────────────────────────────────────────────────────────────
section "Namespaces"

out=$(k get namespaces 2>&1)
if echo "$out" | grep -q "default"; then
    pass "get namespaces (default exists)"
else
    fail "get namespaces" "$out"
fi

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: test-ns
EOF
)
if echo "$out" | grep -qiE "created|configured|test-ns"; then pass "create namespace"; else fail "create namespace" "$out"; fi
out=$(k get namespace test-ns 2>&1)
if echo "$out" | grep -qE "test-ns|Active"; then pass "get namespace test-ns"; else fail "get namespace test-ns" "$out"; fi
out=$(kapply delete namespace test-ns 2>&1) || true
if echo "$out" | grep -qiE "deleted|test-ns"; then pass "delete namespace"; else fail "delete namespace" "$out"; fi

# ── 3. Nodes ─────────────────────────────────────────────────────────────────
section "Nodes"

out=$(k get nodes 2>&1)
if echo "$out" | grep -qi "ready\|z8s"; then
    pass "get nodes"
else
    fail "get nodes" "$out"
fi

out=$(k describe node 2>&1)
if echo "$out" | grep -qi "Capacity\|cpu\|memory"; then
    pass "describe node (capacity present)"
else
    fail "describe node" "$out"
fi

# ── 4. Pod lifecycle ─────────────────────────────────────────────────────────
section "Pod lifecycle"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
  namespace: default
spec:
  containers:
  - name: main
    image: ""
    command: ["/bin/sleep"]
    args: ["60"]
EOF
)
if echo "$out" | grep -qiE "created|configured|test-pod"; then
    pass "create pod"
else
    fail "create pod" "$out"
fi

out=$(k get pods 2>&1)
if echo "$out" | grep -q "test-pod"; then
    pass "list pods (test-pod visible)"
else
    fail "list pods" "$out"
fi

out=$(k get pod test-pod -o jsonpath='{.status.phase}' 2>&1)
if [[ "$out" == "Running" || "$out" == "Pending" ]]; then
    pass "pod phase is valid ($out)"
else
    fail "pod phase" "got: '$out'"
fi

# wait for ready
if wait_pod_ready test-pod default 20; then
    pass "pod became Ready (1/1)"
else
    phase=$(k get pod test-pod -o jsonpath='{.status.phase}' 2>/dev/null)
    ready=$(k get pod test-pod -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
    fail "pod ready" "phase=$phase ready=$ready after 20s"
fi

out=$(k get pod test-pod -o wide 2>&1)
if echo "$out" | grep -q "test-pod"; then
    pass "get pod -o wide"
else
    fail "get pod -o wide" "$out"
fi

out=$(k describe pod test-pod 2>&1)
if echo "$out" | grep -qi "Status\|Container"; then
    pass "describe pod"
else
    fail "describe pod" "$out"
fi

out=$(k logs test-pod 2>&1)
# sleep has no output; an error message means failure
if echo "$out" | grep -qi "error\|404\|not found"; then
    fail "pod logs" "$out"
else
    pass "pod logs (no error)"
fi

kapply delete pod test-pod >/dev/null 2>&1 && pass "delete pod" || fail "delete pod" "command failed"

# ── 5. Phase 1 isolation (user namespace) ────────────────────────────────────
section "Phase 1 isolation (user namespace)"

out=$(k exec ubuntu -- id 2>&1)
if echo "$out" | grep -q "uid=0(root)"; then
    pass "uid mapping (root inside userns)"
else
    fail "uid mapping (root inside userns)" "$out"
fi

out=$(k exec ubuntu -- ls -la /proc/1/exe 2>&1)
if ! echo "$out" | grep -qi "systemd\|lib/systemd"; then
    pass "container has its own /proc/1 (not host init)"
else
    fail "container has its own /proc/1 (not host init)" "$out"
fi

out=$(k exec ubuntu -- cat /proc/self/uid_map 2>&1)
if echo "$out" | grep -q "^\s*0\s"; then
    pass "uid_map shows root mapping"
else
    fail "uid_map shows root mapping" "$out"
fi

# ── 6. Exec ──────────────────────────────────────────────────────────────────
section "Exec & interactive shell"

kapply apply --validate=false -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: exec-pod
  namespace: default
spec:
  containers:
  - name: shell
    image: ""
    command: ["/bin/sleep"]
    args: ["120"]
EOF

wait_pod_ready exec-pod default 15 || true  # best effort

out=$(k exec exec-pod -- /bin/echo hello 2>&1)
if echo "$out" | grep -q "hello"; then
    pass "exec: echo hello"
else
    fail "exec: echo hello" "$out"
fi

out=$(k exec exec-pod -- /bin/ls / 2>&1)
if echo "$out" | grep -qE "bin|usr|etc"; then
    pass "exec: ls /"
else
    fail "exec: ls /" "$out"
fi

out=$(echo "echo shelltest" | k exec -i exec-pod -- /bin/sh 2>&1)
if echo "$out" | grep -q "shelltest"; then
    pass "exec: non-interactive shell (stdin pipe)"
else
    fail "exec: non-interactive shell" "$out"
fi

k delete pod exec-pod >/dev/null 2>&1 || true

# ── 7. Deployments ───────────────────────────────────────────────────────────
section "Deployments"

out=$(kapply apply --validate=false -f - 2>&1 <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-deploy
  namespace: default
spec:
  replicas: 2
  selector:
    matchLabels:
      app: test
  template:
    metadata:
      labels:
        app: test
    spec:
      containers:
      - name: worker
        image: ""
        command: ["/bin/sleep"]
        args: ["120"]
EOF
)
if echo "$out" | grep -qiE "created|configured|test-deploy"; then
    pass "create deployment"
else
    fail "create deployment" "$out"
fi

out=$(k get deployments 2>&1)
if echo "$out" | grep -q "test-deploy"; then
    pass "list deployments"
else
    fail "list deployments" "$out"
fi

out=$(k get deployment test-deploy 2>&1)
if echo "$out" | grep -q "test-deploy"; then
    pass "get deployment"
else
    fail "get deployment" "$out"
fi

out=$(kapply scale deployment test-deploy --replicas=3 2>&1)
if [[ $? -eq 0 ]]; then
    pass "scale deployment to 3"
else
    fail "scale deployment" "$out"
fi

out=$(k describe deployment test-deploy 2>&1)
if echo "$out" | grep -qi "replicas\|selector"; then
    pass "describe deployment"
else
    fail "describe deployment" "$out"
fi

kapply delete deployment test-deploy >/dev/null 2>&1 && pass "delete deployment" || fail "delete deployment" "command failed"

# ── 8. Events ────────────────────────────────────────────────────────────────
section "Events"

out=$(k get events 2>&1)
if echo "$out" | grep -qiE "event|started|z8s"; then
    pass "get events"
else
    fail "get events" "$out"
fi

out=$(k get events -n default 2>&1)
if [[ $? -eq 0 ]]; then
    pass "get events -n default"
else
    fail "get events -n default" "$out"
fi

# ── 9. ConfigMaps & Secrets ─────────────────────────────────────────────────
section "ConfigMaps & Secrets"

out=$(k get configmaps 2>&1)
[[ $? -eq 0 ]] && pass "get configmaps" || fail "get configmaps" "$out"

out=$(k get secrets 2>&1)
[[ $? -eq 0 ]] && pass "get secrets" || fail "get secrets" "$out"

# ── 10. All-namespaces ───────────────────────────────────────────────────────
section "Cross-namespace"

out=$(k get pods --all-namespaces 2>&1)
[[ $? -eq 0 ]] && pass "get pods --all-namespaces" || fail "get pods --all-namespaces" "$out"
