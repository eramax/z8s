#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: ClusterIP service DNAT reaches backend pod
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-cip-srv
  namespace: default
  labels:
    app: test-cip
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=cip-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-cip-svc
  namespace: default
spec:
  selector:
    app: test-cip
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: test-cip-client
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
wait_pod_ready test-cip-srv || { fail "Server not ready"; cleanup "$YAML"; exit 1; }
wait_pod_ready test-cip-client || { fail "Client not ready"; cleanup "$YAML"; exit 1; }
wait_svc_ready test-cip-svc || { fail "Service not ready"; cleanup "$YAML"; exit 1; }

cip=$(k get svc test-cip-svc -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
resp=$(k exec test-cip-client -- sh -c "wget -q -O- -T 3 http://${cip}:80/" 2>&1)
if [[ "$resp" == "cip-ok" ]]; then
    pass "ClusterIP $cip returns 'cip-ok'"
else
    fail "ClusterIP response" "got=$resp"
fi

cleanup "$YAML"
summary
