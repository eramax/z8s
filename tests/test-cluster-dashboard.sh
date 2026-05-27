kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: cd-test
  labels:
    app: cd-test
spec:
  containers:
  - name: dashboard
    image: eramax/cluster-dashboard:latest
    ports:
    - containerPort: 80
EOF

echo "Waiting for pod..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod cd-test -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Checking what ports the app listens on..."
out=$(kubectl exec cd-test -- sh -c 'netstat -tlnp 2>/dev/null || ss -tlnp 2>/dev/null || cat /proc/1/net/tcp 2>/dev/null' 2>&1) || true
echo "Listening ports:"
echo "$out"

echo ""
echo "Trying curl on port 80..."
out=$(kubectl exec cd-test -- wget -q -O- -T 5 "http://127.0.0.1:80/" 2>&1) || true
exit_code=$?
echo "Exit code: $exit_code"
echo "Output: $(echo "$out" | head -c 300)"

echo ""
echo "Trying common ports..."
for port in 3000 8080 8888 80 443 3001 5000 8081; do
  out=$(kubectl exec cd-test -- wget -q -O- -T 2 "http://127.0.0.1:${port}/" 2>&1) || true
  if echo "$out" | grep -qiE "html|dashboard|cluster"; then
    echo "FOUND: listens on port $port"
    break
  fi
done

kubectl delete pod cd-test --ignore-not-found
