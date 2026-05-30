#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: NodePort is reachable from host
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-np-srv
  namespace: default
  labels:
    app: test-np
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=np-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-np-svc
  namespace: default
spec:
  type: NodePort
  selector:
    app: test-np
  ports:
  - port: 80
    targetPort: 8080
    nodePort: 30100
YAML
)

kapply <<<"$YAML"
wait_pod_ready test-np-srv || { fail "Server not ready"; cleanup "$YAML"; exit 1; }
wait_svc_ready test-np-svc || { fail "Service not ready"; cleanup "$YAML"; exit 1; }

host_ip=$(ip -4 addr show eth0 2>/dev/null | grep inet | awk '{print $2}' | cut -d/ -f1)
resp=$(timeout 4 wget -q -O- "http://${host_ip}:30100/" 2>&1)
if [[ "$resp" == "np-ok" ]]; then
    pass "NodePort 30100 returns 'np-ok'"
else
    fail "NodePort response" "got=$resp"
fi

cleanup "$YAML"
summary
