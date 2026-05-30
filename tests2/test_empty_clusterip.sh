#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# B4: Empty ClusterIP (no matching pods) drops traffic / connection refused
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: test-empty-svc
  namespace: default
spec:
  selector:
    app: nonexistent-app
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-empty-client
  namespace: default
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
wait_pod_ready test-empty-client || { fail "Client not ready"; cleanup "$YAML"; exit 1; }
wait_svc_ready test-empty-svc || { fail "Service not ready"; cleanup "$YAML"; exit 1; }

cip=$(k get svc test-empty-svc -o jsonpath='{.spec.clusterIP}' 2>/dev/null)

# Connection should timeout (DNAT with no backends = no route)
resp=$(k exec test-empty-client -- sh -c "timeout 4 wget -q -O- http://${cip}:80/" 2>&1)
if [[ -z "$resp" ]]; then
    pass "Empty ClusterIP drops traffic (timeout)"
else
    fail "Empty ClusterIP should not respond" "got=$resp"
fi

cleanup "$YAML"
summary
