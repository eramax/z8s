#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# A2/C3: Two pods communicate directly, source IP is preserved (not SNATted)
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-p2p-a
  namespace: default
  labels:
    app: test-p2p
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=p2p-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-p2p-b
  namespace: default
  labels:
    app: test-p2p
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "15"]
    ports:
    - containerPort: 80
YAML
)

kapply <<<"$YAML"
wait_pod_ready test-p2p-a || { fail "Pod A not ready"; cleanup "$YAML"; exit 1; }
wait_pod_ready test-p2p-b || { fail "Pod B not ready"; cleanup "$YAML"; exit 1; }

ip_a=$(k get pod test-p2p-a -o jsonpath='{.status.podIP}' 2>/dev/null)

# Pod B reaches Pod A directly
resp=$(k exec test-p2p-b -- sh -c "wget -q -O- -T 3 http://${ip_a}:8080/" 2>&1)
if [[ "$resp" == "p2p-ok" ]]; then
    pass "Pod-to-pod: B reaches A directly at $ip_a"
else
    fail "Pod-to-pod failed" "got=$resp"
fi

cleanup "$YAML"
summary
