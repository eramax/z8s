kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: exec-scratch-test
  labels:
    app: exec-scratch
spec:
  containers:
  - name: http-echo
    image: hashicorp/http-echo:latest
    args: ["-text=hello from scratch"]
    ports:
    - containerPort: 5678
EOF

echo "Waiting for pod..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod exec-scratch-test -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Testing exec with wget (scratch image - no wget)..."
out=$(kubectl exec exec-scratch-test -- wget --version 2>&1) || true
if echo "$out" | grep -q "No such file or directory\|not found"; then
  echo "KNOWN ISSUE: exec ENOENT - wget not in scratch image"
else
  echo "INTERESTING: $(echo "$out" | head -1)"
fi

echo ""
echo "Testing HTTP via service proxy instead..."
kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: exec-scratch-svc
spec:
  selector:
    app: exec-scratch
  ports:
  - name: http
    port: 15678
    targetPort: 5678
    protocol: TCP
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: exec-scratch-client
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
EOF

echo "Waiting for client..."
for i in $(seq 1 30); do
  phase=$(kubectl get pod exec-scratch-client -o jsonpath='{.status.phase}' 2>/dev/null)
  if [[ "$phase" == "Running" ]]; then echo "client ready"; break; fi
  sleep 2
done

SVC_IP=$(kubectl get svc exec-scratch-svc -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
echo "Service ClusterIP: $SVC_IP"

for try in 1 2 3; do
  out=$(kubectl exec exec-scratch-client -- wget -q -O- -T 5 "http://${SVC_IP}:15678/" 2>&1) || true
  if echo "$out" | grep -q "hello from scratch"; then
    echo "PASS: http-echo service works via proxy (try $try)"
    break
  fi
  if [[ $try -eq 3 ]]; then
    echo "FAIL: http-echo service not reachable: $(echo "$out" | head -c 200)"
  fi
  sleep 3
done

kubectl delete pod exec-scratch-test --ignore-not-found
kubectl delete pod exec-scratch-client --ignore-not-found
kubectl delete svc exec-scratch-svc --ignore-not-found
