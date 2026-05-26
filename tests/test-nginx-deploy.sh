kubectl apply --validate=false -f - <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: nginx-deployment
  labels:
    app: nginx
spec:
  replicas: 2
  selector:
    matchLabels:
      app: nginx
  template:
    metadata:
      labels:
        app: nginx
    spec:
      containers:
      - name: nginx
        image: nginx:latest
        command: ["/bin/sh", "-c"]
        args:
          - sed -i 's/^user .*/user root;/' /etc/nginx/nginx.conf && exec nginx -g 'daemon off;'
        ports:
        - containerPort: 80
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: nginx-service
spec:
  selector:
    app: nginx
  ports:
  - name: http
    port: 80
    targetPort: 80
    nodePort: 30080
  type: NodePort
EOF

echo "Waiting for deployment to be ready..."
for i in $(seq 1 30); do
  ready=$(kubectl get deployment nginx-deployment -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
  if [[ "$ready" -ge 2 ]]; then echo "ready"; break; fi
  sleep 2
done

echo "Testing access via NodePort..."
for i in $(seq 1 10); do
  resp=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:30080 2>/dev/null)
  if [[ "$resp" == "200" ]]; then
    echo "nginx is accessible at http://localhost:30080"
    curl -s http://localhost:30080 | head -5
    exit 0
  fi
  sleep 2
done
echo "Failed to reach nginx on :30080"
exit 1
