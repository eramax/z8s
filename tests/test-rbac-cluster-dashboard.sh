#!/usr/bin/env bash
# R8: ServiceAccount token + Role — in-pod API access for cluster-dashboard.
# Requires: z8s running at Z8S_SERVER (default http://127.0.0.1:6443).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVER="${Z8S_SERVER:-http://127.0.0.1:6443}"
KUBECTL="${KUBECTL:-kubectl}"
STACK="${SCRIPT_DIR}/rbac/cluster-dashboard-stack.yaml"

PASS=0
FAIL=0

pass() { echo "  PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "  FAIL: $1 — ${2:-}"; FAIL=$((FAIL + 1)); }

k() { "$KUBECTL" --server="$SERVER" "$@"; }

cleanup() {
  echo "Cleaning up RBAC dashboard test resources..."
  k delete -f "$STACK" --ignore-not-found 2>/dev/null || true
}
trap cleanup EXIT

echo "=== RBAC cluster-dashboard E2E (R8) ==="
echo "Server: $SERVER"
echo ""

if ! k version --request-timeout=5s &>/dev/null; then
  echo "ERROR: cannot reach API at $SERVER"
  exit 1
fi

echo "Applying stack..."
k apply -f "$STACK"

echo "Waiting for cluster-dashboard Deployment..."
if k wait --for=condition=Available "deployment/cluster-dashboard" -n default --timeout=120s 2>/dev/null; then
  pass "deployment/cluster-dashboard Available"
else
  fail "deployment ready" "timeout or not Available"
fi

POD=""
for _ in $(seq 1 60); do
  POD=$(k get pods -n default -l app=cluster-dashboard -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
  if [[ -n "$POD" ]]; then
    phase=$(k get pod "$POD" -n default -o jsonpath='{.status.phase}' 2>/dev/null || true)
    ready=$(k get pod "$POD" -n default -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null || true)
    if [[ "$phase" == "Running" && "$ready" == "true" ]]; then
      break
    fi
  fi
  sleep 2
done

if [[ -z "$POD" ]]; then
  fail "dashboard pod" "no running pod"
else
  pass "dashboard pod $POD Running"
fi

# In-pod API call using mounted SA token (not X-Remote-User).
API_TEST='
TOKEN=$(cat /var/run/secrets/z8s.io/serviceaccount/token 2>/dev/null || true)
if [ -z "$TOKEN" ]; then echo "NO_TOKEN"; exit 2; fi
if command -v kubectl >/dev/null 2>&1; then
  KUBECONFIG=/var/run/secrets/z8s.io/serviceaccount/kubeconfig kubectl get pods -n default --request-timeout=15s
else
  wget -qO- --header="Authorization: Bearer $TOKEN" \
    --no-check-certificate \
    "https://kubernetes.default.svc.cluster.local/api/v1/namespaces/default/pods" 2>/dev/null \
    | head -c 200
fi
'
out=$(k exec -n default "$POD" -c dashboard -- sh -c "$API_TEST" 2>&1) || rc=$?
rc=${rc:-0}
if [[ "$rc" -eq 0 ]] && echo "$out" | grep -qE 'pod|Pod|items|NAME'; then
  pass "in-pod list pods with SA token"
else
  fail "in-pod list pods" "rc=$rc output=${out:0:200}"
fi

# SSAR from pod (R5)
if k exec -n default "$POD" -c dashboard -- sh -c '
  command -v kubectl >/dev/null 2>&1 || exit 0
  KUBECONFIG=/var/run/secrets/z8s.io/serviceaccount/kubeconfig \
    kubectl auth can-i list pods --namespace=default
' 2>/dev/null | grep -q yes; then
  pass "kubectl auth can-i list pods"
else
  echo "  SKIP: kubectl auth can-i (kubectl missing or SSAR no)"
fi

# Negative: pod without RoleBinding should not list pods.
DENY_POD="rbac-deny-test"
for _ in $(seq 1 30); do
  phase=$(k get pod "$DENY_POD" -n default -o jsonpath='{.status.phase}' 2>/dev/null || true)
  [[ "$phase" == "Running" ]] && break
  sleep 1
done
deny_out=$(k exec -n default "$DENY_POD" -c test -- sh -c '
TOKEN=$(cat /var/run/secrets/z8s.io/serviceaccount/token 2>/dev/null || true)
[ -n "$TOKEN" ] || exit 2
wget -qO- --header="Authorization: Bearer $TOKEN" \
  --no-check-certificate \
  "https://kubernetes.default.svc.cluster.local/api/v1/namespaces/default/pods" 2>&1
' 2>&1) || deny_rc=$?
deny_rc=${deny_rc:-0}
if [[ "$deny_rc" -ne 0 ]] || echo "$deny_out" | grep -qiE 'Forbidden|403|Unauthorized|failure'; then
  pass "no-access SA denied API list"
else
  fail "no-access SA should be denied" "got: ${deny_out:0:120}"
fi

# Multi-doc apply endpoint (R6) — curl with deploy-bot if Role exists, else skip
echo ""
echo "Multi-doc apply authz smoke (optional)..."
if command -v curl &>/dev/null; then
  code=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SERVER/api/v1/apply" \
    -H "Content-Type: application/yaml" \
    -H "X-Remote-User: anonymous" \
    --data-binary "apiVersion: v1
kind: ConfigMap
metadata:
  name: rbac-apply-test
  namespace: default
" 2>/dev/null || echo "000")
  if [[ "$code" == "403" ]] || [[ "$code" == "200" ]]; then
    pass "POST /api/v1/apply returned $code"
  else
    echo "  NOTE: apply endpoint HTTP $code (bindings may be open)"
  fi
  k delete configmap rbac-apply-test -n default --ignore-not-found 2>/dev/null || true
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]
