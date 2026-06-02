#!/usr/bin/env bash
# test-z8s-nested-pid1.sh — Host z8s runs a pod where z8s is PID 1; inner z8s deploys nginx.
#
# Prerequisites:
#   - Host z8s running (sudo ./z8s.sh start or equivalent)
#   - kubectl configured for HOST cluster (port 6443)
#   - Container image z8s:dev (build: see deploy/z8s/Dockerfile in plan, or):
#       docker build -t z8s:dev -f deploy/z8s/Dockerfile .
#   - Root or privileged pods allowed on host
#
set -euo pipefail

HOST_API="${Z8S_SERVER:-https://127.0.0.1:6443}"
INNER_API="${Z8S_INNER_SERVER:-http://127.0.0.1:16443}"
K="${KUBECTL:-kubectl}"
PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1 — $2"; FAIL=$((FAIL + 1)); }

cleanup() {
  echo "Cleaning up..."
  $K delete pod z8s-nested -n default --ignore-not-found --wait=false 2>/dev/null || true
  $K --server="$INNER_API" --insecure-skip-tls-verify delete deployment nested-nginx -n default --ignore-not-found 2>/dev/null || true
}
trap cleanup EXIT

echo "=== Nested z8s (PID 1 in pod) E2E ==="
echo "Host API:  $HOST_API"
echo "Inner API: $INNER_API"
echo ""

# ── 0. Host cluster up ─────────────────────────────────────────────
echo "0. Host z8s health"
if curl -sfk "${HOST_API}/healthz" | grep -q ok; then
  pass "Host /healthz"
else
  fail "Host /healthz" "is host z8s running on ${HOST_API}?"
  echo "=== Results: $PASS passed, $FAIL failed ==="
  exit 1
fi

# ── 1. Deploy nested z8s pod ───────────────────────────────────────
echo ""
echo "1. Apply z8s-nested pod (privileged, z8s as PID 1)"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
$K apply --validate=false -f "${SCRIPT_DIR}/nested/z8s-pid1-pod.yaml"

echo "Waiting for z8s-nested Ready..."
if $K wait --for=condition=Ready pod/z8s-nested -n default --timeout=180s 2>/dev/null; then
  pass "z8s-nested pod Ready"
else
  fail "z8s-nested Ready" "$($K get pod z8s-nested -n default -o wide 2>/dev/null || true)"
  $K logs z8s-nested -n default --tail=40 2>/dev/null || true
  echo "=== Results: $PASS passed, $FAIL failed ==="
  exit 1
fi

# ── 2. Inner API ───────────────────────────────────────────────────
echo ""
echo "2. Inner z8s API via hostPort 16443"
if curl -sfk "${INNER_API}/healthz" | grep -q ok; then
  pass "Inner /healthz"
else
  fail "Inner /healthz" "curl ${INNER_API}/healthz"
  $K logs z8s-nested -n default --tail=60 2>/dev/null || true
fi

# ── 3. Deploy nginx on INNER cluster ───────────────────────────────
echo ""
echo "3. Apply nested-nginx Deployment to INNER z8s"
$K --server="$INNER_API" --insecure-skip-tls-verify apply --validate=false -f - <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: nested-nginx
  namespace: default
spec:
  replicas: 1
  selector:
    matchLabels:
      app: nested-nginx
  template:
    metadata:
      labels:
        app: nested-nginx
    spec:
      containers:
        - name: nginx
          image: docker.io/library/nginx:alpine
          ports:
            - containerPort: 80
EOF
pass "Applied nested-nginx to inner API"

# ── 4. Wait for inner pod Running ──────────────────────────────────
echo ""
echo "4. Wait for nested-nginx pod on inner cluster"
INNER_OK=0
for i in $(seq 1 60); do
  phase=$($K --server="$INNER_API" --insecure-skip-tls-verify get pods -n default -l app=nested-nginx \
    -o jsonpath='{.items[0].status.phase}' 2>/dev/null || echo "")
  if [[ "$phase" == "Running" ]]; then
    INNER_OK=1
    break
  fi
  sleep 2
done
if [[ "$INNER_OK" -eq 1 ]]; then
  pass "nested-nginx pod Running on inner z8s"
else
  fail "nested-nginx Running" "phase=${phase:-unknown}"
  $K --server="$INNER_API" --insecure-skip-tls-verify get pods -A 2>/dev/null || true
  $K logs z8s-nested -n default --tail=80 2>/dev/null || true
fi

# ── 5. List pods on inner ────────────────────────────────────────────
echo ""
echo "5. Inner kubectl get pods"
count=$($K --server="$INNER_API" --insecure-skip-tls-verify get pods -n default --no-headers 2>/dev/null | wc -l)
if [[ "${count:-0}" -ge 1 ]]; then
  pass "Inner cluster lists >= 1 pod"
else
  fail "Inner pod list" "empty"
fi

echo ""
echo "=== Results: $PASS passed, $FAIL failed ==="
[[ "$FAIL" -eq 0 ]] && exit 0 || exit 1
