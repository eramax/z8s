kubectl apply --validate=false -f - <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: whoami-deployment
  labels:
    app: whoami
spec:
  replicas: 3
  selector:
    matchLabels:
      app: whoami
  template:
    metadata:
      labels:
        app: whoami
    spec:
      containers:
      - name: whoami
        image: traefik/whoami:latest
        ports:
        - containerPort: 80
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: whoami-service
spec:
  selector:
    app: whoami
  ports:
  - name: http
    port: 80
    targetPort: 80
    nodePort: 30081
  type: NodePort
EOF

echo "Waiting for deployment to be ready..."
for i in $(seq 1 30); do
  ready=$(kubectl get deployment whoami-deployment -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
  if [[ "$ready" -ge 3 ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Testing load balancing across replicas..."
for i in $(seq 1 6); do
  hostname=$(curl -s http://localhost:30081 2>/dev/null | grep -i "Hostname" || echo "unknown")
  echo "  Request $i: $hostname"
done
