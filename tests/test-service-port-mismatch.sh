kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: svc-mismatch-server
  labels:
    app: mismatch-test
spec:
  containers:
  - name: server
    image: python:3-slim
    command: ["/bin/sh", "-c"]
    args:
    - python3 -c "from http.server import HTTPServer, SimpleHTTPRequestHandler; HTTPServer(('127.0.0.1', 7777), SimpleHTTPRequestHandler).serve_forever()"
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: svc-mismatch
spec:
  selector:
    app: mismatch-test
  ports:
  - name: http
    port: 18888
    targetPort: 7777
    protocol: TCP
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: svc-mismatch-client
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
EOF

echo "Waiting for server pod..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod svc-mismatch-server -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "server ready"; break; fi
  sleep 2
done

echo "Waiting for client pod..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod svc-mismatch-client -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "client ready"; break; fi
  sleep 2
done

CLUSTER_IP=$(kubectl get svc svc-mismatch -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
echo "Service ClusterIP: $CLUSTER_IP"

echo "Testing service with port=18888, targetPort=7777..."
for try in 1 2 3; do
  out=$(kubectl exec svc-mismatch-client -- wget -q -O- -T 5 "http://${CLUSTER_IP}:18888/" 2>&1) || true
  if echo "$out" | grep -qiE "directory listing|http|html"; then
    echo "PASS: service reachable (try $try)"
    break
  fi
  if [[ $try -eq 3 ]]; then
    echo "FAIL: service not reachable after 3 tries"
    echo "Output: $(echo "$out" | head -c 200)"
    echo ""
    echo "=== kubectl describe svc ==="
    kubectl describe svc svc-mismatch 2>&1 | head -20
    echo ""
    echo "=== kubectl get endpoints ==="
    kubectl get endpoints svc-mismatch 2>&1
  fi
  sleep 3
done

kubectl delete pod svc-mismatch-server --ignore-not-found
kubectl delete pod svc-mismatch-client --ignore-not-found
kubectl delete svc svc-mismatch --ignore-not-found
