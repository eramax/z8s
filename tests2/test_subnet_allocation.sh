#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: pod with z8s.io/subnet annotation gets IP from subnet CIDR
# Delete any previous test subnet first
kubectl --server="$SERVER" delete subnet test-alloc-sub 2>/dev/null || true
sleep 1

SUBNET=$(cat <<'YAML'
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: test-alloc-sub
spec:
  vnet: test-alloc-vnet
  cidr: 10.200.5.0/24
YAML
)

POD=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-alloc
  namespace: default
  annotations:
    z8s.io/subnet: test-alloc-sub
    z8s.io/vnet: test-alloc-vnet
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "10"]
    ports:
    - containerPort: 80
YAML
)

echo "$SUBNET" | "$KUBECTL" --validate=false --server="$SERVER" apply --server-side=false -f - 2>&1 || {
    # Fallback: POST directly
    curl -s -X POST "$SERVER/apis/z8s.io/v1/subnets" -H "Content-Type: application/yaml" --data-binary "$SUBNET" >/dev/null
}
sleep 2
kapply <<<"$POD" || { fail "Pod creation failed"; exit 1; }
wait_pod_ready test-alloc || { fail "Pod not ready"; exit 1; }

ip=$(k get pod test-alloc -o jsonpath='{.status.podIP}' 2>/dev/null)
if echo "$ip" | grep -q "^10\.200\.5\."; then
    pass "Pod IP $ip is in subnet 10.200.5.0/24"
else
    fail "Pod IP $ip not in expected subnet"
fi

cleanup "$POD"
kubectl --server="$SERVER" delete subnet test-alloc-sub 2>/dev/null || true
summary
