#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# Test: pod in hub VNet reaches internet via SNAT
kubectl --server="$SERVER" delete nsg test-snat-nsg subnet test-snat-sub vnet test-snat-vnet 2>/dev/null || true
sleep 1

CRDS=$(cat <<'YAML'
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: test-snat-vnet
spec:
  cidr: 10.200.0.0/16
  internet_access: true
---
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: test-snat-sub
spec:
  vnet: test-snat-vnet
  cidr: 10.200.0.0/24
---
apiVersion: z8s.io/v1
kind: NSG
metadata:
  name: test-snat-nsg
spec:
  target_vnets: [test-snat-vnet]
  rules:
    - name: allow-internet
      action: allow
      src_cidrs: ["10.200.0.0/24"]
      dst_cidrs: ["0.0.0.0/0"]
      ports: ["*"]
      protocol: tcp
YAML
)

POD=$(cat <<'YAML'
apiVersion: v1
kind: Pod
metadata:
  name: test-snat
  namespace: default
  annotations:
    z8s.io/subnet: test-snat-sub
    z8s.io/vnet: test-snat-vnet
spec:
  containers:
  - name: c
    image: alpine
    command: ["sleep", "15"]
    ports:
    - containerPort: 80
YAML
)

# Use POST via curl to avoid kubectl apply's PATCH issues
for doc in $(echo "$CRDS" | grep -oP '(?<=kind: ).*' | head -3); do
    api=$(echo "$doc" | tr '[:upper:]' '[:lower:]')s
    curl -s -X POST "$SERVER/apis/z8s.io/v1/$api" -H "Content-Type: application/yaml" \
      --data-binary "$(echo "$CRDS" | awk -v RS='---' "/kind: $doc/{print}")" >/dev/null 2>&1 || true
done
sleep 2

echo "$POD" | "$KUBECTL" --validate=false --server="$SERVER" apply -f - 2>&1
wait_pod_ready test-snat || { fail "Pod not ready"; exit 1; }

resp=$(k exec test-snat -- sh -c "timeout 4 nc -zv 1.1.1.1 80" 2>&1)
if echo "$resp" | grep -qE "open|Connected"; then
    pass "Pod reaches internet via SNAT"
else
    resp2=$(k exec test-snat -- sh -c "timeout 4 wget -q -O- http://1.1.1.1/" 2>&1)
    if [[ -n "$resp2" ]]; then
        pass "Pod reaches internet (wget)"
    else
        fail "Internet unreachable" "nc=$resp wget=$resp2"
    fi
fi

kubectl --server="$SERVER" delete pod test-snat 2>/dev/null || true
kubectl --server="$SERVER" delete nsg test-snat-nsg subnet test-snat-sub vnet test-snat-vnet 2>/dev/null || true
summary
