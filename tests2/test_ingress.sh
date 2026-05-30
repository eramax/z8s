#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: Ingress routes by Host header
YAML=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-ing-srv
  namespace: default
  labels:
    app: test-ing
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=ing-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: test-ing-svc
  namespace: default
spec:
  selector:
    app: test-ing
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: test-ing
  namespace: default
spec:
  rules:
  - host: test.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: test-ing-svc
            port:
              number: 80
---
apiVersion: v1
kind: Pod
metadata:
  name: test-ing-client
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
wait_pod_ready test-ing-srv || { fail "Server not ready"; cleanup "$YAML"; exit 1; }
wait_pod_ready test-ing-client || { fail "Client not ready"; cleanup "$YAML"; exit 1; }
wait_svc_ready test-ing-svc || { fail "Service not ready"; cleanup "$YAML"; exit 1; }

gw="10.100.0.1"
resp=$(k exec test-ing-client -- sh -c "wget -q -O- -T 3 --header='Host: test.example.com' http://${gw}:80/" 2>&1)
if [[ "$resp" == "ing-ok" ]]; then
    pass "Ingress test.example.com returns 'ing-ok'"
else
    fail "Ingress response" "got=$resp"
fi

cleanup "$YAML"
summary
