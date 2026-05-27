kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: describe-test-pod
  labels:
    app: describe-test
spec:
  containers:
  - name: server
    image: python:3-slim
    command: ["/bin/sh", "-c"]
    args:
    - python3 -c "from http.server import HTTPServer, SimpleHTTPRequestHandler; HTTPServer(('127.0.0.1', 8888), SimpleHTTPRequestHandler).serve_forever()"
    ports:
    - containerPort: 8888
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: describe-test-svc
spec:
  selector:
    app: describe-test
  ports:
  - port: 9999
    targetPort: 8888
    protocol: TCP
EOF

echo "Waiting for pod..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod describe-test-pod -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Testing kubectl describe svc (should not crash)..."
out=$(kubectl describe svc describe-test-svc 2>&1)
if echo "$out" | grep -q "error\|panic\|SIGSEGV\|segmentation"; then
  echo "FAIL: describe svc crashed:"
  echo "$out" | tail -5
  exit 1
else
  echo "PASS: describe svc works"
  echo "$out" | head -10
fi

kubectl delete svc describe-test-svc --ignore-not-found
kubectl delete pod describe-test-pod --ignore-not-found
