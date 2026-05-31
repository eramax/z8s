#!/usr/bin/env bash
set -uo pipefail

KUBECTL="$(command -v kubectl 2>/dev/null || echo /home/abb/.local/bin/kubectl)"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
STATE_FILE="/tmp/z8s-hub-spoke-state.sh"
PASS=0; FAIL=0; ERRORS=()
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { local m="$1" d="${2:-}"; echo -e "${RED}FAIL${NC} $m${d:+: $d}"; ERRORS+=("$m${d:+: $d}"); FAIL=$((FAIL+1)); }

k() { "$KUBECTL" --server="$SERVER" "$@" 2>&1 || true; }

timeout_run() {
    local secs="$1"; shift
    local tmpf=$(mktemp)
    ("$@" > "$tmpf" 2>&1) & local pid=$!
    sleep "$secs" && kill "$pid" 2>/dev/null & local killer=$!
    wait "$pid" 2>/dev/null; kill "$killer" 2>/dev/null
    cat "$tmpf"; rm -f "$tmpf"
}

run_tests() {
    local label="$1"
    echo -e "${CYAN}--- Tests: $label ---${NC}"

    # Test 1: Hub → svc-spoke1
    local r1=$(timeout_run 5 k exec "$HUB_POD" -n "$NS" -- sh -c "wget -q -O- http://${CIP_S1}:80/" 2>&1)
    if echo "$r1" | grep -q "Spoke1"; then pass "$label: Hub → svc-spoke1"; else fail "$label: Hub → svc-spoke1" "response=$r1"; fi

    # Test 2: Hub → svc-spoke2
    local r2=$(timeout_run 5 k exec "$HUB_POD" -n "$NS" -- sh -c "wget -q -O- http://${CIP_S2}:80/" 2>&1)
    if echo "$r2" | grep -q "Spoke2"; then pass "$label: Hub → svc-spoke2"; else fail "$label: Hub → svc-spoke2" "response=$r2"; fi

    # Test 3: Hub reaches internet
    local r3=$(timeout_run 5 k exec "$HUB_POD" -n "$NS" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if echo "$r3" | grep -qE "open|Connected"; then pass "$label: Hub reaches internet"; else fail "$label: Hub internet access" "response=$r3"; fi

    # Test 4: Spoke1 cannot reach hub
    local r4=$(timeout_run 5 k exec "$S1_POD" -n "$NS" -- sh -c "wget -q -O- http://${CIP_HUB}:80/" 2>&1)
    if [[ -z "$r4" ]]; then pass "$label: Spoke1→hub blocked"; else fail "$label: Spoke1→hub blocked" "expected timeout, got=$r4"; fi

    # Test 5: Spoke2 cannot reach hub
    local r5=$(timeout_run 5 k exec "$S2_POD" -n "$NS" -- sh -c "wget -q -O- http://${CIP_HUB}:80/" 2>&1)
    if [[ -z "$r5" ]]; then pass "$label: Spoke2→hub blocked"; else fail "$label: Spoke2→hub blocked" "expected timeout, got=$r5"; fi

    # Test 6: Spoke1 cannot reach internet
    local r6=$(timeout_run 5 k exec "$S1_POD" -n "$NS" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if echo "$r6" | grep -q "Connected"; then fail "$label: Spoke1 internet leaked" "got=$r6"; else pass "$label: Spoke1 internet blocked"; fi

    # Test 7: Spoke2 cannot reach internet
    local r7=$(timeout_run 5 k exec "$S2_POD" -n "$NS" -- sh -c "nc -zv 1.1.1.1 80" 2>&1)
    if echo "$r7" | grep -q "Connected"; then fail "$label: Spoke2 internet leaked" "got=$r7"; else pass "$label: Spoke2 internet blocked"; fi

    # Test 8: NodePort
    local host_ip=$(ip -4 addr show eth0 2>/dev/null | grep inet | awk '{print $2}' | cut -d/ -f1 2>/dev/null)
    local r8=$(timeout_run 5 wget -q -O- http://${host_ip}:30005/ 2>&1)
    if echo "$r8" | grep -q "Hub(Spoke1,Spoke2)"; then pass "$label: NodePort 30005"; else fail "$label: NodePort 30005" "response=$r8"; fi

    # Test 9: Ingress
    local gw="10.100.0.1"
    local r9=$(timeout_run 5 k exec "$HUB_POD" -n "$NS" -- sh -c "wget -q -O- --header='Host: hub1.local.cluster' http://${gw}:80/" 2>&1)
    if echo "$r9" | grep -q "Hub(Spoke1,Spoke2)"; then pass "$label: Ingress hub1.local.cluster"; else fail "$label: Ingress hub1.local.cluster" "response=$r9"; fi
}

# ── Main ──────────────────────────────────────────────────────────────
echo -e "${CYAN}=== Hub-and-Spoke Verify ===${NC}"

if [[ ! -f "$STATE_FILE" ]]; then
    echo "State file not found. Run test_hub_spoke_setup.sh first."
    exit 1
fi
source "$STATE_FILE"

# Run tests before restart
run_tests "pre-restart"

# If --restart flag, restart z8s and verify state persisted
if [[ "${1:-}" == "--restart" ]]; then
    echo ""
    echo -e "${YELLOW}--- Restarting z8s (DB persistence test) ---${NC}"
    DATA_DIR=$(grep "DATA_DIR" "$STATE_FILE" | cut -d= -f2 | tr -d '"')
    sudo ./z8s.sh restart --data-dir "$DATA_DIR" || { fail "Restart failed"; exit 1; }
    sleep 3

    # Verify resources still exist after restart
    echo "  Verifying resources after restart..."

    # Check deployments
    d1=$(k get deploy dep-spoke1 -n "$NS" -o jsonpath='{.metadata.name}' 2>/dev/null)
    d2=$(k get deploy dep-spoke2 -n "$NS" -o jsonpath='{.metadata.name}' 2>/dev/null)
    d3=$(k get deploy dep-hub -n "$NS" -o jsonpath='{.metadata.name}' 2>/dev/null)
    if [[ -z "$d1" ]]; then fail "dep-spoke1 missing after restart"; else pass "dep-spoke1 persisted"; fi
    if [[ -z "$d2" ]]; then fail "dep-spoke2 missing after restart"; else pass "dep-spoke2 persisted"; fi
    if [[ -z "$d3" ]]; then fail "dep-hub missing after restart"; else pass "dep-hub persisted"; fi

    # Check services
    s1=$(k get svc svc-spoke1 -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
    s2=$(k get svc svc-spoke2 -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
    s3=$(k get svc svc-hub -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
    if [[ -z "$s1" ]]; then fail "svc-spoke1 missing"; else pass "svc-spoke1 persisted ($s1)"; fi
    if [[ -z "$s2" ]]; then fail "svc-spoke2 missing"; else pass "svc-spoke2 persisted ($s2)"; fi
    if [[ -z "$s3" ]]; then fail "svc-hub missing"; else pass "svc-hub persisted ($s3)"; fi

    # Check CRDs
    v=$(k get vnet test-vnet -o jsonpath='{.metadata.name}' 2>/dev/null)
    n=$(k get nsg test-nsg -o jsonpath='{.metadata.name}' 2>/dev/null)
    if [[ -z "$v" ]]; then fail "VNet missing"; else pass "VNet persisted"; fi
    if [[ -z "$n" ]]; then fail "NSG missing"; else pass "NSG persisted"; fi

    # Wait for pods to be recreated by reconciler
    echo "  Waiting for pods after restart..."
    deadline=$(( $(date +%s) + 90 ))
    while [[ $(date +%s) -lt $deadline ]]; do
        S1_POD=$(k get pods -l app=spoke1 -n "$NS" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        S2_POD=$(k get pods -l app=spoke2 -n "$NS" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        HUB_POD=$(k get pods -l app=hub -n "$NS" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        if [[ -n "$S1_POD" && -n "$S2_POD" && -n "$HUB_POD" ]]; then break; fi
        sleep 2
    done

    # Re-fetch ClusterIPs (may have changed)
    CIP_S1=$(k get svc svc-spoke1 -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    CIP_S2=$(k get svc svc-spoke2 -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    CIP_HUB=$(k get svc svc-hub -n "$NS" -o jsonpath='{.spec.clusterIP}' 2>/dev/null | tr -d '[:space:]')
    echo "  Hub=$HUB_POD cip_hub=$CIP_HUB cip_s1=$CIP_S1 cip_s2=$CIP_S2"

    # Wait for service proxy to settle after restart
    echo "  Waiting for services to settle..."
    sleep 10

    # Run tests again after restart
    run_tests "post-restart"
fi

# ── Summary ────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}Passed: ${PASS}${NC}, ${RED}Failed: ${FAIL}${NC}"
for e in "${ERRORS[@]}"; do echo "  - $e"; done
exit $FAIL
