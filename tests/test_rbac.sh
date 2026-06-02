#!/usr/bin/env bash
# test_rbac.sh — Test RBAC (Role/RoleBinding) functionality using kubectl
# Requires: z8s running, kubectl configured to point at z8s API
set -euo pipefail

PASS=0; FAIL=0; SKIP=0

pass() { echo "  PASS: $1"; ((PASS++)); }
fail() { echo "  FAIL: $1 — $2"; ((FAIL++)); }
skip() { echo "  SKIP: $1 — $2"; ((SKIP++)); }

cleanup() {
    echo "Cleaning up..."
    kubectl delete rolebinding test-binding -n default --ignore-not-found 2>/dev/null || true
    kubectl delete role pod-manager -n default --ignore-not-found 2>/dev/null || true
    kubectl delete pod rbac-test-pod -n default --ignore-not-found 2>/dev/null || true
}
trap cleanup EXIT

echo "=== RBAC Tests ==="
echo ""

# ── 1. Create a Role ──────────────────────────────────────────────
echo "1. Create Role 'pod-manager'"
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
  - apiGroups: [""]
    resources: ["configmaps"]
    verbs: ["get", "list"]
EOF
if kubectl get role pod-manager -n default >/dev/null 2>&1; then
    pass "Role created"
else
    fail "Role creation" "kubectl get failed"
fi

# ── 2. Verify Role rules ────────────────────────────────────────
echo "2. Verify Role has correct rules"
RULES=$(kubectl get role pod-manager -n default -o jsonpath='{.rules}')
if echo "$RULES" | grep -q "pods"; then
    pass "Role has pods resource"
else
    fail "Role rules" "rules: $RULES"
fi
if echo "$RULES" | grep -q "configmaps"; then
    pass "Role has configmaps resource"
else
    fail "Role rules" "missing configmaps"
fi

# ── 3. List Roles ────────────────────────────────────────────────
echo "3. List Roles in default namespace"
ROLE_COUNT=$(kubectl get roles -n default --no-headers 2>/dev/null | wc -l)
if [ "$ROLE_COUNT" -ge 1 ]; then
    pass "Role list has >= 1 item"
else
    fail "Role list" "count=$ROLE_COUNT"
fi

# ── 4. Create a RoleBinding ──────────────────────────────────────
echo "4. Create RoleBinding 'test-binding' (user=admin → pod-manager)"
kubectl apply -n default -f - <<'EOF'
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: test-binding
  namespace: default
subjects:
  - kind: User
    name: admin
  - kind: ServiceAccount
    namespace: default
    name: deploy-bot
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: pod-manager
EOF
if kubectl get rolebinding test-binding -n default >/dev/null 2>&1; then
    pass "RoleBinding created"
else
    fail "RoleBinding creation" "kubectl get failed"
fi

# ── 5. Verify RoleBinding subjects ──────────────────────────────
echo "5. Verify RoleBinding has correct subjects and roleRef"
SUBJECTS=$(kubectl get rolebinding test-binding -n default -o jsonpath='{.subjects[*].name}')
ROLE_REF=$(kubectl get rolebinding test-binding -n default -o jsonpath='{.roleRef.name}')
if echo "$SUBJECTS" | grep -q "admin"; then
    pass "RoleBinding has User 'admin'"
else
    fail "RoleBinding subjects" "subjects: $SUBJECTS"
fi
if echo "$SUBJECTS" | grep -q "deploy-bot"; then
    pass "RoleBinding has ServiceAccount 'deploy-bot'"
else
    fail "RoleBinding subjects" "missing deploy-bot"
fi
if [ "$ROLE_REF" = "pod-manager" ]; then
    pass "RoleBinding points to pod-manager role"
else
    fail "RoleBinding roleRef" "roleRef: $ROLE_REF"
fi

# ── 6. List RoleBindings ────────────────────────────────────────
echo "6. List RoleBindings in default namespace"
RB_COUNT=$(kubectl get rolebindings -n default --no-headers 2>/dev/null | wc -l)
if [ "$RB_COUNT" -ge 1 ]; then
    pass "RoleBinding list has >= 1 item"
else
    fail "RoleBinding list" "count=$RB_COUNT"
fi

# ── 7. Verify namespace isolation ────────────────────────────────
echo "7. RoleBinding is namespace-scoped to default"
RB_NS=$(kubectl get rolebinding test-binding -n default -o jsonpath='{.metadata.namespace}')
if [ "$RB_NS" = "default" ]; then
    pass "RoleBinding namespace is 'default'"
else
    fail "Namespace isolation" "namespace: $RB_NS"
fi

# ── 8. Non-existent resource returns error ──────────────────────
echo "8. Get non-existent RoleBinding returns error"
if kubectl get rolebinding nonexistent -n default >/dev/null 2>&1; then
    fail "404 check" "expected error but got success"
else
    pass "Non-existent RoleBinding returns error"
fi

# ── 9. Delete RoleBinding ──────────────────────────────────────
echo "9. Delete RoleBinding 'test-binding'"
kubectl delete rolebinding test-binding -n default >/dev/null 2>&1
if ! kubectl get rolebinding test-binding -n default >/dev/null 2>&1; then
    pass "RoleBinding deleted"
else
    fail "RoleBinding deletion" "still exists"
fi

# ── 10. Delete Role ────────────────────────────────────────────
echo "10. Delete Role 'pod-manager'"
kubectl delete role pod-manager -n default >/dev/null 2>&1
if ! kubectl get role pod-manager -n default >/dev/null 2>&1; then
    pass "Role deleted"
else
    fail "Role deletion" "still exists"
fi

# ── Summary ──────────────────────────────────────────────────────
echo ""
echo "=== Results: $PASS passed, $FAIL failed, $SKIP skipped ==="
[ "$FAIL" -eq 0 ] && exit 0 || exit 1
