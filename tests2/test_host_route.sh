#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# A6/A7: Host has /32 route for pod via veth, no bridge interfaces
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-route
  namespace: default
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "8"]
    ports:
    - containerPort: 80
YAML
)

kapply <<<"$YAML"
wait_pod_ready test-route || { fail "Pod not ready"; cleanup "$YAML"; exit 1; }

ip=$(k get pod test-route -o jsonpath='{.status.podIP}' 2>/dev/null)

# Check host route (via /proc/net/route or ip route)
host_route=$(ip route show "$ip" 2>/dev/null)
if echo "$host_route" | grep -q "veth-"; then
    pass "Host has /32 route for $ip via veth"
else
    # Fallback: check /proc/net/route
    route_hex=$(printf '%02X' ${ip//./ } 2>/dev/null)
    proc_route=$(cat /proc/net/route 2>/dev/null | grep -i "$route_hex" | head -1 || true)
    if [[ -n "$proc_route" ]]; then
        pass "Host route for $ip found in /proc/net/route"
    else
        fail "No host route for $ip" "ip route=$host_route"
    fi
fi

# No bridge interfaces
bridges=$(ip link show type bridge 2>/dev/null | grep -c "bridge" || true)
if [[ "$bridges" -eq 0 ]]; then
    pass "No bridge interfaces"
else
    fail "Bridge interfaces found: $bridges"
fi

cleanup "$YAML"
summary
