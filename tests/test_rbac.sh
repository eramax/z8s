#!/usr/bin/env bash
# test_rbac.sh — Test RBAC for pod-to-cluster API access
# Demonstrates: a pod's service account can only do what its Role allows.
# Requires: z8s running, kubectl configured
set -euo pipefail

API="http://127.0.0.1:6443"
PASS=0; FAIL=0; SKIP=0

pass() { echo "  PASS: $1"; ((PASS++)); }
fail() { echo "  FAIL: $1 — $2"; ((FAIL++)); }
skip() { echo "  SKIP: $1 — $2"; ((SKIP++)); }

cleanup() {
    echo "Cleaning up..."
    kubectl delete rolebinding viewer-binding deploy-bot-binding -n default --ignore-not-found 2>/dev/null || true
    kubectl delete role pod-viewer pod-manager -n default --ignore-not-found 2>/dev/null || true
    kubectl delete pod rbac-test-pod rbac-test-pod-2 -n default --ignore-not-found 2>/dev/null || true
}
trap cleanup EXIT

echo "=== RBAC Tests: Pod-to-Cluster API Access ==="
echo ""

# ── Setup: Create a test pod ──────────────────────────────────────
echo "Setup: Create test pod 'rbac-test-pod'"
kubectl apply -n default -f - <<'EOF' >/dev/null 2>&1
apiVersion: v1
kind: Pod
metadata:
  name: rbac-test-pod
  namespace: default
spec:
  containers:
    - name: test
      image: busybox
      command: ["sleep", "3600"]
EOF
sleep 1
if kubectl get pod rbac-test-pod -n default >/dev/null 2>&1; then
    pass "Test pod created"
else
    fail "Test pod creation" "pod not found"
fi

# ── 1. Create a Role: pod-viewer can only get/list/watch pods ────
echo ""
echo "1. Create Role 'pod-viewer' (read-only pods)"
kubectl apply -n default -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: pod-viewer
  namespace: default
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch"]
EOF
if kubectl get role pod-viewer -n default >/dev/null 2>&1; then
    pass "Role 'pod-viewer' created"
else
    fail "Role creation" ""
fi

# ── 2. Create a RoleBinding: deploy-bot SA → pod-viewer role ────
echo ""
echo "2. Create RoleBinding: deploy-bot SA → pod-viewer"
kubectl apply -n default -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: viewer-binding
  namespace: default
subjects:
  - kind: ServiceAccount
    namespace: default
    name: deploy-bot
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: pod-viewer
EOF
if kubectl get rolebinding viewer-binding -n default >/dev/null 2>&1; then
    pass "RoleBinding created"
else
    fail "RoleBinding creation" ""
fi

# ── 3. Test: deploy-bot SA can GET pods ──────────────────────────
echo ""
echo "3. deploy-bot SA can GET pods (allowed by Role)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "200" ]; then
    pass "GET pods → 200 (allowed)"
else
    fail "GET pods" "HTTP $RESP (expected 200)"
fi

# ── 4. Test: deploy-bot SA can LIST pods ─────────────────────────
echo ""
echo "4. deploy-bot SA can LIST pods (allowed by Role)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "200" ]; then
    pass "LIST pods → 200 (allowed)"
else
    fail "LIST pods" "HTTP $RESP (expected 200)"
fi

# ── 5. Test: deploy-bot SA CANNOT DELETE pods (role only allows get/list/watch)
echo ""
echo "5. deploy-bot SA CANNOT DELETE pods (not in Role)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods/rbac-test-pod")
if [ "$RESP" = "403" ]; then
    pass "DELETE pods → 403 (denied)"
else
    fail "DELETE pods" "HTTP $RESP (expected 403)"
fi

# ── 6. Test: deploy-bot SA CANNOT CREATE pods ────────────────────
echo ""
echo "6. deploy-bot SA CANNOT CREATE pods (not in Role)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X POST -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    -H "Content-Type: application/json" \
    -d '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"rbac-test-pod-2"},"spec":{"containers":[{"name":"test","image":"busybox"}]}}' \
    "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "403" ]; then
    pass "CREATE pods → 403 (denied)"
else
    fail "CREATE pods" "HTTP $RESP (expected 403)"
fi

# ── 7. Test: anonymous user has NO access (no RoleBinding for anonymous) ──
echo ""
echo "7. Anonymous user CANNOT DELETE pods (no RoleBinding)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE \
    "$API/api/v1/namespaces/default/pods/rbac-test-pod")
if [ "$RESP" = "403" ]; then
    pass "Anonymous DELETE → 403 (denied)"
else
    fail "Anonymous DELETE" "HTTP $RESP (expected 403)"
fi

# ── 8. Test: GET without RBAC headers still works (read-only) ───
echo ""
echo "8. Unauthenticated GET pods → 200 (read-only allowed)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "200" ]; then
    pass "Unauthenticated GET → 200 (read-only allowed)"
else
    fail "Unauthenticated GET" "HTTP $RESP (expected 200)"
fi

# ── 9. Create a more powerful role: pod-manager ─────────────────
echo ""
echo "9. Create Role 'pod-manager' (full CRUD on pods)"
kubectl apply -n default -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: pod-manager
  namespace: default
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch", "create", "update", "delete"]
EOF
if kubectl get role pod-manager -n default >/dev/null 2>&1; then
    pass "Role 'pod-manager' created"
else
    fail "Role creation" ""
fi

# ── 10. Bind deploy-bot to pod-manager too ──────────────────────
echo ""
echo "10. Bind deploy-bot SA → pod-manager (upgrade permissions)"
kubectl apply -n default -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: deploy-bot-binding
  namespace: default
subjects:
  - kind: ServiceAccount
    namespace: default
    name: deploy-bot
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: pod-manager
EOF
if kubectl get rolebinding deploy-bot-binding -n default >/dev/null 2>&1; then
    pass "RoleBinding created (deploy-bot → pod-manager)"
else
    fail "RoleBinding creation" ""
fi

# ── 11. Test: deploy-bot SA can now CREATE pods ─────────────────
echo ""
echo "11. deploy-bot SA can now CREATE pods (pod-manager role)"
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X POST -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    -H "Content-Type: application/json" \
    -d '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"rbac-test-pod-2"},"spec":{"containers":[{"name":"test","image":"busybox","command":["sleep","3600"]}]}}' \
    "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "201" ] || [ "$RESP" = "200" ]; then
    pass "CREATE pods → $RESP (allowed after upgrade)"
else
    fail "CREATE pods" "HTTP $RESP (expected 201 or 200)"
fi

# ── 12. Test: deploy-bot SA can now DELETE pods ─────────────────
echo ""
echo "12. deploy-bot SA can now DELETE pods (pod-manager role)"
sleep 1
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods/rbac-test-pod-2")
if [ "$RESP" = "200" ] || [ "$RESP" = "204" ]; then
    pass "DELETE pods → $RESP (allowed after upgrade)"
else
    fail "DELETE pods" "HTTP $RESP (expected 200 or 204)"
fi

# ── 13. Test: deploy-bot SA still cannot touch services ─────────
echo ""
echo "13. deploy-bot SA cannot DELETE services (not in any Role)"
kubectl apply -n default -f - <<'EOF' >/dev/null 2>&1
apiVersion: v1
kind: Service
metadata:
  name: rbac-test-svc
  namespace: default
spec:
  selector:
    app: test
  ports:
    - port: 80
EOF
sleep 1
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/services/rbac-test-svc")
if [ "$RESP" = "403" ]; then
    pass "DELETE services → 403 (not in any Role)"
else
    fail "DELETE services" "HTTP $RESP (expected 403)"
fi
kubectl delete service rbac-test-svc -n default --ignore-not-found 2>/dev/null || true

# ── 14. Verify: remove the viewer binding, deploy-bot loses read access
echo ""
echo "14. Remove viewer-binding → deploy-bot loses read access to pods"
kubectl delete rolebinding viewer-binding -n default >/dev/null 2>&1
# deploy-bot still has pod-manager binding, so should still have access
RESP=$(curl -s -o /dev/null -w "%{http_code}" -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods")
if [ "$RESP" = "200" ]; then
    pass "GET pods → 200 (still allowed via pod-manager binding)"
else
    fail "GET pods after removing viewer" "HTTP $RESP (expected 200)"
fi

# ── 15. Remove pod-manager binding too → deploy-bot loses all access
echo ""
echo "15. Remove deploy-bot-binding → deploy-bot loses ALL pod access"
kubectl delete rolebinding deploy-bot-binding -n default >/dev/null 2>&1
RESP=$(curl -s -o /dev/null -w "%{http_code}" -X DELETE -H "X-Remote-User: system:serviceaccount:default:deploy-bot" \
    "$API/api/v1/namespaces/default/pods/rbac-test-pod")
if [ "$RESP" = "403" ]; then
    pass "DELETE pods → 403 (all bindings removed)"
else
    fail "DELETE pods after removing all bindings" "HTTP $RESP (expected 403)"
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "=== Results: $PASS passed, $FAIL failed, $SKIP skipped ==="
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
