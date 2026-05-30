#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: pod gets IP from pool, has veth + default route via gateway
POD_YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-pod-net
  namespace: default
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "10"]
    ports:
    - containerPort: 80
YAML
)

kapply <<<"$POD_YAML"
wait_pod_ready test-pod-net || { fail "Pod not ready"; cleanup "$POD_YAML"; exit 1; }

# Test 1: pod IP is not 127.0.0.1
ip=$(k get pod test-pod-net -o jsonpath='{.status.podIP}' 2>/dev/null)
if [[ "$ip" != "127.0.0.1" && -n "$ip" ]]; then
    pass "Pod has IP $ip"
else
    fail "Pod IP is $ip (expected real IP)"
fi

# Test 2: pod has default route via gateway
route=$(k exec test-pod-net -- ip route 2>/dev/null)
if echo "$route" | grep -q "default via"; then
    pass "Pod has default route"
else
    fail "No default route"
fi

# Test 3: pod interface is zeth-* not eth0
if echo "$route" | grep -q "zeth-"; then
    pass "Pod uses zeth interface"
else
    fail "Interface is not zeth"
fi

cleanup "$POD_YAML"
summary
