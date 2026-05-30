#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Clean up any stale resources from previous runs
kubectl --server="$SERVER" delete nsg mini-nsg subnet mini-hub subnet mini-spoke vnet mini-vnet 2>/dev/null || true
sleep 1

CRDS=$(cat <<'YAML'
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: mini-vnet
spec:
  cidr: 10.200.0.0/16
  internet_access: true
---
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: mini-hub
spec:
  vnet: mini-vnet
  cidr: 10.200.0.0/24
---
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: mini-spoke
spec:
  vnet: mini-vnet
  cidr: 10.200.1.0/24
---
apiVersion: z8s.io/v1
kind: NSG
metadata:
  name: mini-nsg
spec:
  target_vnets: [mini-vnet]
  rules:
    - name: hub-to-spoke
      action: allow
      src_cidrs: ["10.200.0.0/24"]
      dst_cidrs: ["10.200.1.0/24"]
      ports: ["80"]
      protocol: tcp
YAML
)

PODS=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: mhub
  namespace: default
  labels:
    app: mhub
  annotations:
    z8s.io/subnet: mini-hub
    z8s.io/vnet: mini-vnet
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=hub-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Pod
metadata:
  name: mspoke
  namespace: default
  labels:
    app: mspoke
  annotations:
    z8s.io/subnet: mini-spoke
    z8s.io/vnet: mini-vnet
spec:
  containers:
  - name: srv
    image: hashicorp/http-echo
    args: ["-text=spoke-ok", "-listen=:8080"]
    ports:
    - containerPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: mhub-svc
  namespace: default
spec:
  selector:
    app: mhub
  ports:
  - port: 80
    targetPort: 8080
---
apiVersion: v1
kind: Service
metadata:
  name: mspoke-svc
  namespace: default
spec:
  selector:
    app: mspoke
  ports:
  - port: 80
    targetPort: 8080
YAML
)

kapply <<<"$CRDS" 2>&1 || true
sleep 2
kapply <<<"$PODS" 2>&1
wait_pod_ready mhub || { fail "Hub not ready"; exit 1; }
wait_pod_ready mspoke || { fail "Spoke not ready"; exit 1; }
wait_svc_ready mhub-svc && wait_svc_ready mspoke-svc

cip_spoke=$(k get svc mspoke-svc -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
cip_hub=$(k get svc mhub-svc -o jsonpath='{.spec.clusterIP}' 2>/dev/null)

r1=$(k exec mhub -- sh -c "wget -q -O- -T 3 http://${cip_spoke}:80/" 2>&1)
if [[ "$r1" == "spoke-ok" ]]; then pass "Hub reaches spoke"; else fail "Hub→spoke failed" "got=$r1"; fi

# Spoke→hub — allowed (forward chain default policy is accept).
# Block unidirectional traffic via NSG deny rules when needed.
r2=$(k exec mspoke -- sh -c "wget -q -O- -T 3 http://${cip_hub}:80/" 2>&1)
if echo "$r2" | grep -q "hub-ok"; then pass "Spoke reaches hub (accept policy)"; else fail "Spoke→hub failed" "got=$r2"; fi

cleanup "$PODS"; sleep 1; cleanup "$CRDS"
summary
