kubectl apply --validate=false -f - <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: scale-test
  labels:
    app: scale-test
spec:
  replicas: 1
  selector:
    matchLabels:
      app: scale-test
  template:
    metadata:
      labels:
        app: scale-test
    spec:
      containers:
      - name: sleeper
        image: alpine:latest
        command: ["sleep", "infinity"]
EOF

echo "Waiting for initial replica..."
for i in $(seq 1 30); do
  ready=$(kubectl get deployment scale-test -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
  if [[ "$ready" -ge 1 ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Scaling from 1 to 3..."
kubectl scale deployment scale-test --replicas=3

for i in $(seq 1 45); do
  ready=$(kubectl get deployment scale-test -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
  if [[ "$ready" -ge 3 ]]; then echo "PASS: scale 1->3 completed"; break; fi
  if [[ "$i" -eq 45 ]]; then
    echo "FAIL: scale 1->3 stuck at readyReplicas=$ready"
    kubectl get pods | grep scale-test
    kubectl get deployment scale-test -o yaml | grep -A 5 status
  fi
  sleep 2
done

kubectl delete deployment scale-test --ignore-not-found
