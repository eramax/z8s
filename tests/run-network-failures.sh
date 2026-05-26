#!/usr/bin/env bash
# Regression tests extracted from full-suite networking / info failures (~2–4 min).
# Covers: ClusterIP services, nginx-hello deploy, info-pod exec (not ubuntu/GLIBC).
#
# Usage:
#   cargo build && ./z8s.sh stop && ./z8s.sh start
#   ./tests/run-network-failures.sh
set -eo pipefail

SERVER="${Z8S_SERVER:-http://localhost:6443}"
YAML_DIR="$(dirname "$0")"
NS="default"
PASS=0
FAIL=0
SKIP=0
ERRORS=()

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS + 1)); }
fail() {
  local m="$1" d="${2:-}"
  echo -e "${RED}FAIL${NC} $m${d:+: $d}"
  ERRORS+=("$m${d:+: $d}")
  FAIL=$((FAIL + 1))
}
skip() { echo -e "${YELLOW}SKIP${NC} $1"; SKIP=$((SKIP + 1)); }
section() { echo -e "\n${YELLOW}══ $1 ══${NC}"; }
sub() { echo -e "${CYAN}  ▸ $1${NC}"; }

k() { kubectl --server="$SERVER" "$@" 2>&1; }
kapply() { kubectl --server="$SERVER" "$@" 2>&1; }

wait_pod_ready() {
  local name="$1" timeout="${2:-90}"
  local deadline=$(( $(date +%s) + timeout ))
  while [[ $(date +%s) -lt $deadline ]]; do
    local phase ready
    phase=$(k get pod "$name" -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null) || true
    ready=$(k get pod "$name" -n "$NS" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null) || true
    if [[ "$phase" == "Running" && "$ready" == "true" ]]; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_deploy_ready() {
  local name="$1" replicas="${2:-1}" timeout="${3:-90}"
  local deadline=$(( $(date +%s) + timeout ))
  while [[ $(date +%s) -lt $deadline ]]; do
    local ready
    ready=$(k get deployment "$name" -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null) || ready=""
    ready="${ready:-0}"
    [[ "$ready" =~ ^[0-9]+$ ]] || ready=0
    if (( ready >= replicas )); then
      return 0
    fi
    sleep 1
  done
  return 1
}

test_svc() {
  local svc="$1" port="$2" pattern="$3" label="$4"
  local ip out
  ip=$(k get svc "$svc" -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null) || true
  [[ -z "$ip" || "$ip" == "None" ]] && { fail "svc: $label" "no clusterIP"; return; }
  for try in 1 2 3 4 5; do
    out=$(k exec svc-client -n "$NS" -- wget -q -O- -T 5 "http://${ip}:${port}/" 2>&1) || true
    if echo "$out" | grep -qiE "$pattern"; then
      pass "svc: $label (try $try)"
      return
    fi
    sleep 2
  done
  fail "svc: $label" "no match (clusterIP=$ip port=$port) last: $(echo "$out" | head -c 120)"
}

validate_info_exec() {
  local deploy="$1" port="$2" pattern="$3"
  local pod
  pod=$(k get pods -n "$NS" -l "app=$deploy" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null) || true
  [[ -z "$pod" ]] && { fail "info exec: $deploy" "no pod"; return; }
  local phase
  phase=$(k get pod "$pod" -n "$NS" -o jsonpath='{.status.phase}' 2>/dev/null) || true
  [[ "$phase" != "Running" ]] && { fail "info exec: $deploy" "phase=$phase"; return; }
  local out
  out=$(k exec "$pod" -n "$NS" -- wget -q -O- -T 5 "http://127.0.0.1:${port}/" 2>&1) || true
  if echo "$out" | grep -qiE "$pattern"; then
    pass "info exec: $deploy :${port}"
  else
    fail "info exec: $deploy" "$(echo "$out" | head -c 160)"
  fi
}

cleanup() {
  sub "Deleting test resources..."
  k delete deployment nginx-hello whoami http-echo hostinfo cluster-dashboard nginx-deploy python-deploy \
    --ignore-not-found >/dev/null 2>&1 || true
  k delete pod python-pod svc-client --ignore-not-found >/dev/null 2>&1 || true
  k delete svc python-svc nginx-svc nginx-hello-svc whoami-svc http-echo-svc hostinfo-svc cluster-dashboard-svc \
    --ignore-not-found >/dev/null 2>&1 || true
}

trap cleanup EXIT

section "Setup"
if ! k get --raw=/healthz 2>/dev/null | grep -q ok; then
  fail "z8s API" "not reachable at $SERVER"
  exit 1
fi
pass "z8s API healthy"

cleanup
sleep 2

# Prereqs from full suite (configmaps for nginx/python)
for f in 02-configmap.yaml 03-secret.yaml; do
  [[ -f "$YAML_DIR/$f" ]] && kapply apply --validate=false -f "$YAML_DIR/$f" >/dev/null || true
done

section "Apply workloads (services before pods — matches full suite order)"
sub "Services first"
kapply apply --validate=false -f "$YAML_DIR/11-service.yaml" >/dev/null
# Info services are in 13-deployment-info.yaml — apply file once below

sub "Python pod + deploy (host port 18080 via Service targetPort)"
kapply apply --validate=false -f "$YAML_DIR/05-pod-python.yaml" >/dev/null
kapply apply --validate=false -f "$YAML_DIR/09-deployment-python.yaml" >/dev/null

sub "Nginx + info deployments"
kapply apply --validate=false -f "$YAML_DIR/12-deployment-nginx.yaml" >/dev/null
kapply apply --validate=false -f "$YAML_DIR/13-deployment-info.yaml" >/dev/null

kapply apply --validate=false -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: svc-client
  namespace: ${NS}
spec:
  containers:
  - name: client
    image: alpine:latest
    command: ["sleep", "infinity"]
EOF

section "Wait for readiness"
for dep in python-deploy nginx-deploy whoami http-echo hostinfo nginx-hello cluster-dashboard; do
  replicas=1
  [[ "$dep" == "python-deploy" || "$dep" == "nginx-deploy" ]] && replicas=2
  timeout=90
  [[ "$dep" == "nginx-hello" ]] && timeout=120
  if wait_deploy_ready "$dep" "$replicas" "$timeout"; then
    pass "deployment $dep ready ($replicas)"
  else
    ready=$(k get deployment "$dep" -n "$NS" -o jsonpath='{.status.readyReplicas}' 2>/dev/null) || ready=0
    fail "deployment $dep" "readyReplicas=${ready:-0}/$replicas"
  fi
done

if wait_pod_ready python-pod 60; then
  pass "pod python-pod ready"
else
  fail "pod python-pod" "not ready"
fi

if wait_pod_ready svc-client 45; then
  pass "svc-client ready"
else
  fail "svc-client" "not ready — service tests skipped"
  exit 1
fi

section "ClusterIP service HTTP (full-suite section 12 failures)"
test_svc "python-svc" "18080" "directory listing|http|html" "python HTTP :18080"
test_svc "nginx-svc" "80" "nginx|html|welcome" "nginx :80"
test_svc "whoami-svc" "80" "Hostname|hostname" "whoami :80"
test_svc "http-echo-svc" "5678" "hello from z8s" "http-echo :5678"
test_svc "hostinfo-svc" "18081" "hostname|Hostname|html" "hostinfo :18081"
test_svc "nginx-hello-svc" "80" "nginx|html|hello|Server" "nginx-hello :80"

section "In-pod exec + localhost HTTP (full-suite info failures)"
# hostinfo containerPort 8080, service maps 18081→8080
pod=$(k get pods -n "$NS" -l app=hostinfo -o jsonpath='{.items[0].metadata.name}' 2>/dev/null) || true
if [[ -n "$pod" ]]; then
  out=$(k exec "$pod" -n "$NS" -- wget -q -O- -T 5 "http://127.0.0.1:8080/" 2>&1) || true
  if echo "$out" | grep -qiE "hostname|Hostname|html"; then
    pass "info exec: hostinfo :8080"
  else
    fail "info exec: hostinfo" "$(echo "$out" | head -c 160)"
  fi
fi

validate_info_exec "whoami" "80" "Hostname|hostname"
validate_info_exec "http-echo" "5678" "hello from z8s"
validate_info_exec "nginx-hello" "80" "nginx|html|hello"
validate_info_exec "cluster-dashboard" "80" "dashboard|cluster|html"

skip "ubuntu GLIBC envFrom (host libc vs image — environment, not z8s networking)"

echo ""
echo "════════════════════════════════════════════"
echo " Results: ${PASS} passed, ${FAIL} failed, ${SKIP} skipped"
if [[ ${#ERRORS[@]} -gt 0 ]]; then
  echo ""
  echo " Failures:"
  for e in "${ERRORS[@]}"; do echo "  ✗ $e"; done
fi
echo "════════════════════════════════════════════"
echo " Log: /tmp/z8s.log"

[[ "$FAIL" -eq 0 ]]
