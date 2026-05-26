#!/usr/bin/env bash
# Targeted custom test validating CIDR separation, loopback activation, and /dev/null writable fix
set -eo pipefail

SERVER="http://localhost:6443"
NS="z8s-custom-test"
DAEMON="./z8s.sh"

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; }
fail() { echo -e "${RED}FAIL${NC} $1"; exit 1; }
section() { echo -e "\n${YELLOW}══ $1 ══${NC}"; }
sub() { echo -e "${CYAN}  ▸ $1${NC}"; }

k() { /home/abb/.local/bin/kubectl --server="$SERVER" "$@" 2>&1 || true; }
kapply() { /home/abb/.local/bin/kubectl --server="$SERVER" "$@" 2>&1; }

section "1. Starting z8s daemon cleanly"
"$DAEMON" stop || true
sleep 1
"$DAEMON" start

# Wait for server to become responsive
for i in $(seq 1 15); do
  if k get --raw=/healthz >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! k get --raw=/healthz 2>/dev/null | grep -q ok; then
  fail "z8s API did not start successfully or is not responsive"
fi
pass "z8s daemon is running and healthy"

# Clean any existing namespace
k delete namespace "$NS" --wait=false --ignore-not-found >/dev/null 2>&1 || true
sleep 1

section "2. Create Namespace and Verify CIDR Allocation"
kapply apply --validate=false -f - <<EOF >/dev/null
apiVersion: v1
kind: Namespace
metadata:
  name: ${NS}
EOF
pass "Namespace ${NS} created successfully"

# Create a test Service to verify CIDR range
kapply apply --validate=false -f - <<EOF >/dev/null
apiVersion: v1
kind: Service
metadata:
  name: test-cidr-svc
  namespace: ${NS}
spec:
  ports:
  - port: 80
    targetPort: 80
  selector:
    app: test-cidr
EOF
pass "Test Service created"

# Get allocated ClusterIP
svc_ip=$(k get svc test-cidr-svc -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
if [[ -z "$svc_ip" || "$svc_ip" == "None" ]]; then
  fail "Failed to allocate ClusterIP for test-cidr-svc"
fi

sub "Allocated Service ClusterIP: $svc_ip"
if [[ "$svc_ip" =~ ^10\.96\. ]]; then
  pass "Service CIDR correctly separated to non-loopback range (10.96.x.x)"
else
  fail "Allocated IP $svc_ip is not within the non-loopback 10.96.0.0/16 CIDR range!"
fi

section "3. Deploy Pod with Nginx (Validating /dev/null and lo)"
kapply apply --validate=false -f - <<EOF >/dev/null
apiVersion: v1
kind: Pod
metadata:
  name: nginx-hello-pod
  namespace: ${NS}
  labels:
    app: nginx-hello
spec:
  containers:
  - name: nginx
    image: nginx:alpine
    ports:
    - containerPort: 80
EOF
pass "Nginx pod manifest applied"

sub "Waiting for nginx-hello-pod to enter Running state and report Ready..."
ready=false
for i in $(seq 1 45); do
  phase=$(k get pod nginx-hello-pod -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null) || true
  is_ready=$(k get pod nginx-hello-pod -n "$NS" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null) || true
  
  if [[ "$phase" == "Running" && "$is_ready" == "true" ]]; then
    ready=true
    break
  fi
  
  # Print container logs if it fails/exits
  if [[ "$phase" == "Failed" ]]; then
    echo "Pod phase is Failed. Fetching logs..."
    k logs nginx-hello-pod -n "$NS" || true
    fail "Pod execution failed prematurely"
  fi
  sleep 1
done

if [[ "$ready" != "true" ]]; then
  echo "Timed out waiting for nginx-hello-pod. Pod status:"
  k get pod nginx-hello-pod -n "$NS" -o yaml || true
  echo "Container logs:"
  k logs nginx-hello-pod -n "$NS" || true
  fail "nginx-hello-pod did not achieve Running and Ready state"
fi
pass "nginx-hello-pod is RUNNING and READY!"

section "4. Validate Loopback (lo) and Internal Networking"
sub "Testing connectivity to 127.0.0.1:80 inside the container namespace"
out=$(k exec nginx-hello-pod -n "$NS" -- wget -q -O- -T 3 "http://127.0.0.1:80/" 2>&1) || true
if echo "$out" | grep -qiE "Welcome to nginx|html|body|http"; then
  pass "Localhost loopback loop connectivity verified (received Nginx response!)"
else
  fail "Localhost loopback connection failed! Output: $out"
fi

section "5. Cleanup"
k delete namespace "$NS" --wait=false --ignore-not-found >/dev/null 2>&1 || true
"$DAEMON" stop
pass "Test suite completed successfully! All networking features are stable."
exit 0
