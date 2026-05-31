#!/usr/bin/env bash
set -uo pipefail

KUBECTL="$(command -v kubectl 2>/dev/null || echo /home/abb/.local/bin/kubectl)"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
NS="hub-spokes"
STATE_FILE="/tmp/z8s-hub-spoke-state.sh"
DATA_DIR="${DATA_DIR:-/tmp/z8s-test-db}"

k() { "$KUBECTL" --server="$SERVER" "$@" 2>&1 || true; }
kapply() { "$KUBECTL" --validate=false --server="$SERVER" apply -f - 2>&1; }

wait_deploy_ready() {
    local name="$1" ns="${2:-$NS}" timeout="${3:-90}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local pod=$(k get pods -l app="$name" -n "$ns" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        if [[ -n "$pod" ]]; then
            local phase=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
            local ready=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
            if [[ "$phase" == "Running" && "$ready" == "true" ]]; then echo "$pod"; return 0; fi
        fi
        sleep 2
    done; echo ""; return 1
}

wait_svc_ready() {
    local name="$1" ns="${2:-$NS}" timeout="${3:-30}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        cip=$(k get svc "$name" -n "$ns" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
        [[ -n "$cip" && "$cip" != "None" ]] && return 0
        sleep 2
    done; return 1
}

wait_crd_ready() {
    local kind="$1" name="$2" timeout="${3:-15}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local exists=$(k get "$kind" "$name" -o jsonpath='{.metadata.name}' 2>/dev/null)
        [[ -n "$exists" ]] && return 0; sleep 2
    done; return 1
}

# ── Create namespace ──────────────────────────────────────────────────
echo "Creating namespace $NS..."
"$KUBECTL" --server="$SERVER" create namespace "$NS" 2>&1 || { echo "Failed to create namespace $NS"; exit 1; }
sleep 1

# ── 1. VNet ──────────────────────────────────────────────────────────
kapply - <<YAML
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: test-vnet
  annotations:
    z8s.io/vnet: test-vnet
spec:
  cidr: 10.200.0.0/16
  internet_access: true
  role: hub
YAML
wait_crd_ready vnet test-vnet 5 || { echo "VNet not created"; exit 1; }

# ── 2. Subnets ────────────────────────────────────────────────────────
kapply - <<YAML
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: hub-sub
  annotations:
    z8s.io/vnet: test-vnet
    z8s.io/subnet: hub-sub
spec:
  vnet: test-vnet
  cidr: 10.200.0.0/24
---
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: spoke-1-sub
  annotations:
    z8s.io/vnet: test-vnet
    z8s.io/subnet: spoke-1-sub
spec:
  vnet: test-vnet
  cidr: 10.200.1.0/24
---
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: spoke-2-sub
  annotations:
    z8s.io/vnet: test-vnet
    z8s.io/subnet: spoke-2-sub
spec:
  vnet: test-vnet
  cidr: 10.200.2.0/24
YAML
sleep 2

# ── 3. NSG ────────────────────────────────────────────────────────────
kapply - <<YAML
apiVersion: z8s.io/v1
kind: NSG
metadata:
  name: test-nsg
  annotations:
    z8s.io/vnet: test-vnet
spec:
  target_vnets:
    - test-vnet
  rules:
    - name: allow-hub-to-spoke1
      action: allow
      src_cidrs: ["10.200.0.0/24"]
      dst_cidrs: ["10.200.1.0/24"]
      ports: ["80"]
      protocol: tcp
      priority: 100
    - name: allow-hub-to-spoke2
      action: allow
      src_cidrs: ["10.200.0.0/24"]
      dst_cidrs: ["10.200.2.0/24"]
      ports: ["80"]
      protocol: tcp
      priority: 200
    - name: allow-hub-to-internet
      action: allow
      src_cidrs: ["10.200.0.0/24"]
      dst_cidrs: ["0.0.0.0/0"]
      ports: ["*"]
      protocol: tcp
      priority: 300
YAML
sleep 1

# ── 4. RouteTable ─────────────────────────────────────────────────────
kapply - <<YAML
apiVersion: z8s.io/v1
kind: RouteTable
metadata:
  name: hub-routes
  annotations:
    z8s.io/subnet: hub-sub
    z8s.io/vnet: test-vnet
spec:
  rules:
    - name: allow-get-only
      methods: ["GET"]
      paths: ["/"]
      action: allow
    - name: deny-other-methods
      methods: ["POST", "PUT", "DELETE", "PATCH"]
      action: deny
YAML
sleep 1

# ── 5. Spoke deployments ──────────────────────────────────────────────
kapply - <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-spoke1
  namespace: $NS
  labels: { app: spoke1 }
  annotations:
    z8s.io/subnet: spoke-1-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels: { app: spoke1 }
  template:
    metadata:
      labels: { app: spoke1 }
      annotations:
        z8s.io/subnet: spoke-1-sub
        z8s.io/vnet: test-vnet
    spec:
      containers:
      - name: srv
        image: hashicorp/http-echo
        args: ["-text=Spoke1", "-listen=:8080"]
        ports:
        - containerPort: 8080
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-spoke2
  namespace: $NS
  labels: { app: spoke2 }
  annotations:
    z8s.io/subnet: spoke-2-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels: { app: spoke2 }
  template:
    metadata:
      labels: { app: spoke2 }
      annotations:
        z8s.io/subnet: spoke-2-sub
        z8s.io/vnet: test-vnet
    spec:
      containers:
      - name: srv
        image: hashicorp/http-echo
        args: ["-text=Spoke2", "-listen=:8080"]
        ports:
        - containerPort: 8080
YAML

# ── 6. Spoke services ─────────────────────────────────────────────────
kapply - <<YAML
apiVersion: v1
kind: Service
metadata:
  name: svc-spoke1
  namespace: $NS
  annotations:
    z8s.io/subnet: spoke-1-sub
    z8s.io/vnet: test-vnet
spec:
  selector: { app: spoke1 }
  ports:
  - port: 80
    targetPort: 8080
    protocol: TCP
---
apiVersion: v1
kind: Service
metadata:
  name: svc-spoke2
  namespace: $NS
  annotations:
    z8s.io/subnet: spoke-2-sub
    z8s.io/vnet: test-vnet
spec:
  selector: { app: spoke2 }
  ports:
  - port: 80
    targetPort: 8080
    protocol: TCP
YAML

wait_svc_ready svc-spoke1 "$NS" || { echo "svc-spoke1 not ready"; exit 1; }
wait_svc_ready svc-spoke2 "$NS" || { echo "svc-spoke2 not ready"; exit 1; }
CIP_S1=$(k get svc svc-spoke1 -n "$NS" -o jsonpath='{.spec.clusterIP}' | tr -d '[:space:]')
CIP_S2=$(k get svc svc-spoke2 -n "$NS" -o jsonpath='{.spec.clusterIP}' | tr -d '[:space:]')
echo "spoke1=$CIP_S1 spoke2=$CIP_S2"

# ── 7. Hub deployment ─────────────────────────────────────────────────
cat > /tmp/hub-deploy.yaml <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-hub
  namespace: $NS
  labels: { app: hub }
  annotations:
    z8s.io/subnet: hub-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels: { app: hub }
  template:
    metadata:
      labels: { app: hub }
      annotations:
        z8s.io/subnet: hub-sub
        z8s.io/vnet: test-vnet
    spec:
      containers:
      - name: srv
        image: python:3-alpine
        command:
        - python3
        - -c
        - |
          import http.server, urllib.request, os
          s1 = os.environ["SPOKE1"]
          s2 = os.environ["SPOKE2"]
          class H(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
              try:
                r1 = urllib.request.urlopen(f"http://{s1}:80/", timeout=3).read().decode().strip()
                r2 = urllib.request.urlopen(f"http://{s2}:80/", timeout=3).read().decode().strip()
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.end_headers()
                self.wfile.write(f"Hub({r1},{r2})".encode())
              except Exception as e:
                self.send_response(500)
                self.end_headers()
                self.wfile.write(f"error: {e}".encode())
          http.server.HTTPServer(("0.0.0.0", 8080), H).serve_forever()
        env:
        - name: SPOKE1
          value: "${CIP_S1}"
        - name: SPOKE2
          value: "${CIP_S2}"
        ports:
        - containerPort: 8080
EOF
"$KUBECTL" --validate=false --server="$SERVER" apply -f /tmp/hub-deploy.yaml 2>&1
rm -f /tmp/hub-deploy.yaml

# ── 8. Hub service + Ingress ──────────────────────────────────────────
kapply - <<YAML
apiVersion: v1
kind: Service
metadata:
  name: svc-hub
  namespace: $NS
  annotations:
    z8s.io/subnet: hub-sub
    z8s.io/vnet: test-vnet
spec:
  type: NodePort
  selector: { app: hub }
  ports:
  - port: 80
    targetPort: 8080
    protocol: TCP
    nodePort: 30005
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: ing-hub
  namespace: $NS
spec:
  rules:
  - host: hub1.local.cluster
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: svc-hub
            port:
              number: 80
YAML

# ── Wait for readiness ────────────────────────────────────────────────
echo "  Waiting for deployments..."
S1_POD=$(wait_deploy_ready spoke1 "$NS") || { echo "dep-spoke1 not ready"; exit 1; }
S2_POD=$(wait_deploy_ready spoke2 "$NS") || { echo "dep-spoke2 not ready"; exit 1; }
HUB_POD=$(wait_deploy_ready hub "$NS") || { echo "dep-hub not ready"; exit 1; }
wait_svc_ready svc-hub "$NS" || { echo "svc-hub not ready"; exit 1; }
sleep 3

CIP_HUB=$(k get svc svc-hub -n "$NS" -o jsonpath='{.spec.clusterIP}' | tr -d '[:space:]')
echo "  hub=$HUB_POD cip_hub=$CIP_HUB cip_s1=$CIP_S1 cip_s2=$CIP_S2"

# ── Save state for verify script ───────────────────────────────────────
cat > "$STATE_FILE" <<EOF
NS="$NS"
DATA_DIR="$DATA_DIR"
CIP_S1="$CIP_S1"
CIP_S2="$CIP_S2"
CIP_HUB="$CIP_HUB"
S1_POD="$S1_POD"
S2_POD="$S2_POD"
HUB_POD="$HUB_POD"
EOF

echo "State saved to $STATE_FILE"
echo "Setup complete — now run test_hub_spoke_verify.sh"
