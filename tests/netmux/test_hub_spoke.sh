#!/usr/bin/env bash
set -uo pipefail

KUBECTL="$(command -v kubectl 2>/dev/null || echo /home/abb/.local/bin/kubectl)"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
PASS=0; FAIL=0; ERRORS=()
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { local m="$1" d="${2:-}"; echo -e "${RED}FAIL${NC} $m${d:+: $d}"; ERRORS+=("$m${d:+: $d}"); FAIL=$((FAIL+1)); }
skip() { echo -e "${YELLOW}SKIP${NC} $1"; }

k() { "$KUBECTL" --server="$SERVER" "$@" 2>&1 || true; }
kapply() { "$KUBECTL" --validate=false --server="$SERVER" apply -f - 2>&1; }

wait_deploy_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-90}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local pod=$(k get pods -l app="$name" -n "$ns" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        if [[ -n "$pod" ]]; then
            local phase=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
            local ready=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
            if [[ "$phase" == "Running" && "$ready" == "true" ]]; then echo "$pod"; return 0; fi
        fi
        sleep 2
    done
    echo ""; return 1
}

wait_svc_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-30}"
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

timeout_run() {
    local secs="$1"; shift
    local tmpf=$(mktemp)
    ("$@" > "$tmpf" 2>&1) & local pid=$!
    sleep "$secs" && kill "$pid" 2>/dev/null & local killer=$!
    wait "$pid" 2>/dev/null; kill "$killer" 2>/dev/null
    cat "$tmpf"; rm -f "$tmpf"
}

# ──────────────────────────────────────────────────────────────────────
# Hub-and-Spoke topology test
#
# Topology:
#   VNet:      test-vnet      10.200.0.0/16
#   Hub-sub:   10.200.0.0/24  internet_access=true  role=hub
#   Spoke-1:   10.200.1.0/24  spoke role
#   Spoke-2:   10.200.2.0/24  spoke role
#
#   dep-hub     hub-sub    :8080 → "Hub"
#   dep-spoke1  spoke-1    :8080 → "Spoke1"
#   dep-spoke2  spoke-2    :8080 → "Spoke2"
#
#   svc-hub     NodePort 30005   annotations: z8s.io/subnet=hub-sub
#   svc-spoke1  ClusterIP        annotations: z8s.io/subnet=spoke-1-sub
#   svc-spoke2  ClusterIP        annotations: z8s.io/subnet=spoke-2-sub
#
# NSG (L3/L4): allow tcp/80 hub→spoke1, hub→spoke2 only
# RouteTable (L7): hub only accepts GET / on port 80
# Ingress: global — hub1.local.cluster → svc-hub:80
#
# Directionality:
#   Hub → Spoke1  : allowed (tcp/80)
#   Hub → Spoke2  : allowed (tcp/80)
#   Hub → Internet: allowed
#   Spoke1 → *    : DENIED
#   Spoke2 → *    : DENIED
#   Hub GET  /    : allowed (RouteTable)
#   Hub POST /    : denied  (RouteTable)
# ──────────────────────────────────────────────────────────────────────

test_H1() {
    echo -e "${CYAN}H1: Hub-and-Spoke topology — full integration${NC}"

    # ── 1. VNet (cluster-scoped, no namespace) ───────────────────────
    kapply - <<'YAML'
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
    wait_crd_ready vnet test-vnet 5 || { fail "H1: VNet not created"; return; }

    # ── 2. Subnets ───────────────────────────────────────────────────
    kapply - <<'YAML'
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

    # ── 3. NSG (L3/L4) — tcp/80 only, unidirectional hub→spoke ──────
    kapply - <<'YAML'
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
YAML
    sleep 1

    # ── 4. RouteTable (L7) — hub accepts only GET / ─────────────────
    kapply - <<'YAML'
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

    # ── 5. Spoke deployments first ─────────────────────────────────
    kapply - <<'YAML'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-spoke1
  namespace: default
  labels:
    app: spoke1
  annotations:
    z8s.io/subnet: spoke-1-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels:
      app: spoke1
  template:
    metadata:
      labels:
        app: spoke1
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
  namespace: default
  labels:
    app: spoke2
  annotations:
    z8s.io/subnet: spoke-2-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels:
      app: spoke2
  template:
    metadata:
      labels:
        app: spoke2
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

    # ── 6. Spoke services ──────────────────────────────────────────
    kapply - <<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: svc-spoke1
  namespace: default
  annotations:
    z8s.io/subnet: spoke-1-sub
    z8s.io/vnet: test-vnet
spec:
  selector:
    app: spoke1
  ports:
  - port: 80
    targetPort: 8080
    protocol: TCP
---
apiVersion: v1
kind: Service
metadata:
  name: svc-spoke2
  namespace: default
  annotations:
    z8s.io/subnet: spoke-2-sub
    z8s.io/vnet: test-vnet
spec:
  selector:
    app: spoke2
  ports:
  - port: 80
    targetPort: 8080
    protocol: TCP
YAML

    # Wait for spoke services to get ClusterIPs
    wait_svc_ready svc-spoke1 || { fail "H1: svc-spoke1 not ready"; return; }
    wait_svc_ready svc-spoke2 || { fail "H1: svc-spoke2 not ready"; return; }
    local cip_s1=$(k get svc svc-spoke1 -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    local cip_s2=$(k get svc svc-spoke2 -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    echo "  spoke1=$cip_s1 spoke2=$cip_s2"

    # ── 7. Hub deployment (with spoke ClusterIPs) ──────────────────
    # Write YAML with interpolated env vars to a temp file
    cat > /tmp/hub-deploy.yaml <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: dep-hub
  namespace: default
  labels:
    app: hub
  annotations:
    z8s.io/subnet: hub-sub
    z8s.io/vnet: test-vnet
spec:
  replicas: 1
  selector:
    matchLabels:
      app: hub
  template:
    metadata:
      labels:
        app: hub
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
          value: "${cip_s1}"
        - name: SPOKE2
          value: "${cip_s2}"
        ports:
        - containerPort: 8080
EOF
    "$KUBECTL" --validate=false --server="$SERVER" apply -f /tmp/hub-deploy.yaml 2>&1
    rm -f /tmp/hub-deploy.yaml

    # ── 8. Hub service + Ingress ───────────────────────────────────
    kapply - <<'YAML'
apiVersion: v1
kind: Service
metadata:
  name: svc-hub
  namespace: default
  annotations:
    z8s.io/subnet: hub-sub
    z8s.io/vnet: test-vnet
spec:
  type: NodePort
  selector:
    app: hub
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
  namespace: default
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

    # ── Wait for resources ───────────────────────────────────────────
    echo "  Waiting for deployments..."
    local s1_pod=$(wait_deploy_ready spoke1) || { fail "H1: dep-spoke1 not ready"; return; }
    local s2_pod=$(wait_deploy_ready spoke2) || { fail "H1: dep-spoke2 not ready"; return; }
    local hub_pod=$(wait_deploy_ready hub) || { fail "H1: dep-hub not ready"; return; }

    wait_svc_ready svc-hub || { fail "H1: svc-hub not ready"; return; }

    sleep 3

    local cip_hub=$(k get svc svc-hub -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    echo "  hub=$hub_pod cip_hub=$cip_hub cip_s1=$cip_s1 cip_s2=$cip_s2"

    # ──────────────────────────────────────────────────────────────
    # Test 1: Hub reaches svc-spoke1 (tcp/80) → "Spoke1"
    # ──────────────────────────────────────────────────────────────
    local r1=$(timeout_run 5 k exec "$hub_pod" -- sh -c "wget -q -O- http://${cip_s1}:80/" 2>&1)
    if echo "$r1" | grep -q "Spoke1"; then
        pass "H1: Hub → svc-spoke1 returns Spoke1"
    else
        fail "H1: Hub → svc-spoke1" "response=$r1"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 2: Hub reaches svc-spoke2 (tcp/80) → "Spoke2"
    # ──────────────────────────────────────────────────────────────
    local r2=$(timeout_run 5 k exec "$hub_pod" -- sh -c "wget -q -O- http://${cip_s2}:80/" 2>&1)
    if echo "$r2" | grep -q "Spoke2"; then
        pass "H1: Hub → svc-spoke2 returns Spoke2"
    else
        fail "H1: Hub → svc-spoke2" "response=$r2"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 3: Hub reaches internet (vnet internet_access=true)
    # ──────────────────────────────────────────────────────────────
    local r3=$(timeout_run 5 k exec "$hub_pod" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if echo "$r3" | grep -q "open"; then
        pass "H1: Hub reaches internet"
    else
        fail "H1: Hub internet access" "response=$r3"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 4: Spoke1 cannot reach hub (unidirectional)
    # ──────────────────────────────────────────────────────────────
    local r4=$(timeout_run 5 k exec "$s1_pod" -- sh -c "wget -q -O- http://${cip_hub}:80/" 2>&1)
    if [[ -z "$r4" ]]; then
        pass "H1: Spoke1 cannot reach hub (timeout)"
    else
        fail "H1: Spoke1→hub blocked" "expected timeout, got=$r4"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 5: Spoke2 cannot reach hub (unidirectional)
    # ──────────────────────────────────────────────────────────────
    local r5=$(timeout_run 5 k exec "$s2_pod" -- sh -c "wget -q -O- http://${cip_hub}:80/" 2>&1)
    if [[ -z "$r5" ]]; then
        pass "H1: Spoke2 cannot reach hub (timeout)"
    else
        fail "H1: Spoke2→hub blocked" "expected timeout, got=$r5"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 6: Spoke1 cannot reach internet (spoke role)
    # ──────────────────────────────────────────────────────────────
    local r6=$(timeout_run 5 k exec "$s1_pod" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if [[ -z "$r6" ]]; then
        pass "H1: Spoke1 cannot reach internet (timeout)"
    else
        fail "H1: Spoke1 internet blocked" "expected timeout, got=$r6"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 7: Spoke2 cannot reach internet (spoke role)
    # ──────────────────────────────────────────────────────────────
    local r7=$(timeout_run 5 k exec "$s2_pod" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if [[ -z "$r7" ]]; then
        pass "H1: Spoke2 cannot reach internet (timeout)"
    else
        fail "H1: Spoke2 internet blocked" "expected timeout, got=$r7"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 8: NodePort 30005 → "Hub(Spoke1,Spoke2)" (via host IP)
    # ──────────────────────────────────────────────────────────────
    local host_ip=$(ip -4 addr show eth0 2>/dev/null | grep inet | awk '{print $2}' | cut -d/ -f1 2>/dev/null)
    local r8=$(timeout_run 5 wget -q -O- http://${host_ip}:30005/ 2>&1)
    if echo "$r8" | grep -q "Hub(Spoke1,Spoke2)"; then
        pass "H1: NodePort 30005 returns Hub(Spoke1,Spoke2)"
    else
        fail "H1: NodePort 30005" "response=$r8"
    fi

    # ──────────────────────────────────────────────────────────────
    # Test 9: Ingress hub1.local.cluster → "Hub(Spoke1,Spoke2)"
    # ──────────────────────────────────────────────────────────────
    local gw="10.100.0.1"
    local r9=$(timeout_run 5 k exec "$hub_pod" -- sh -c "wget -q -O- --header='Host: hub1.local.cluster' http://${gw}:80/" 2>&1)
    if echo "$r9" | grep -q "Hub(Spoke1,Spoke2)"; then
        pass "H1: Ingress hub1.local.cluster returns Hub(Spoke1,Spoke2)"
    else
        fail "H1: Ingress hub1.local.cluster" "response=$r9"
    fi
}

echo -e "${CYAN}=== Hub-and-Spoke Integration Tests ===${NC}"
test_H1
echo ""
echo -e "${GREEN}Passed: ${PASS}${NC}, ${RED}Failed: ${FAIL}${NC}"
for e in "${ERRORS[@]}"; do echo "  - $e"; done
exit $FAIL
