#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# A5: Pod gets a valid IP after delete/recreate (may be different pool IP)
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-recreate
  namespace: default
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "5"]
    ports:
    - containerPort: 80
YAML
)

kapply <<<"$YAML"
wait_pod_ready test-recreate || { fail "First pod not ready"; cleanup "$YAML"; exit 1; }
ip1=$(k get pod test-recreate -o jsonpath='{.status.podIP}' 2>/dev/null)
k delete pod test-recreate --server="$SERVER" --now 2>/dev/null || true
sleep 2

kapply <<<"$YAML"
wait_pod_ready test-recreate || { fail "Second pod not ready"; cleanup "$YAML"; exit 1; }
ip2=$(k get pod test-recreate -o jsonpath='{.status.podIP}' 2>/dev/null)

if [[ -n "$ip2" && "$ip2" != "127.0.0.1" ]]; then
    pass "Recreated pod has valid IP $ip2 (was $ip1)"
else
    fail "Recreated pod IP invalid" "got=$ip2"
fi

cleanup "$YAML"
summary
