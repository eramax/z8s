#!/usr/bin/env bash
set -uo pipefail

KUBECONFIG="${KUBECONFIG:-$HOME/.kube/config}"
STATE_FILE="/tmp/z8s-hub-spoke-state.sh"
PASS=0; FAIL=0; ERRORS=()
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { local m="$1" d="${2:-}"; echo -e "${RED}FAIL${NC} $m${d:+: $d}"; ERRORS+=("$m${d:+: $d}"); FAIL=$((FAIL+1)); }

k() { kubectl --kubeconfig="$KUBECONFIG" "$@" 2>&1 || true; }

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

run_tests "hub-spoke"

# ── Summary ────────────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}Passed: ${PASS}${NC}, ${RED}Failed: ${FAIL}${NC}"
for e in "${ERRORS[@]}"; do echo "  - $e"; done
exit $FAIL
