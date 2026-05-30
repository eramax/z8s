#!/usr/bin/env bash
# Shared test library — sourced by all test scripts
set -uo pipefail

KUBECTL="${KUBECTL:-$(command -v kubectl 2>/dev/null || echo /home/abb/.local/bin/kubectl)}"
SERVER="${Z8S_SERVER:-http://localhost:6443}"
PASS=0; FAIL=0
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[1;33m'; NC='\033[0m'

pass() { echo -e "${GREEN}PASS${NC} $1"; PASS=$((PASS+1)); }
fail() { echo -e "${RED}FAIL${NC} $1${2:+: $2}"; FAIL=$((FAIL+1)); }
skip() { echo -e "${YELLOW}SKIP${NC} $1"; }

k() { "$KUBECTL" --server="$SERVER" "$@" 2>&1 || true; }
kapply() { "$KUBECTL" --validate=false --server="$SERVER" apply -f - 2>&1; }
kdelete() { "$KUBECTL" --server="$SERVER" delete -f - 2>&1; }

# Pod helpers
wait_pod_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-30}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local phase=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
        local ready=$(k get pod "$name" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
        [[ "$phase" == "Running" && "$ready" == "true" ]] && return 0
        sleep 1
    done
    return 1
}

wait_svc_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-15}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local cip=$(k get svc "$name" -n "$ns" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
        [[ -n "$cip" && "$cip" != "None" ]] && return 0
        sleep 1
    done
    return 1
}

wait_deploy_ready() {
    local name="$1" ns="${2:-default}" timeout="${3:-30}"
    local deadline=$(( $(date +%s) + timeout ))
    while [[ $(date +%s) -lt $deadline ]]; do
        local pod=$(k get pods -l app="$name" -n "$ns" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
        if [[ -n "$pod" ]]; then
            local phase=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)
            local ready=$(k get pod "$pod" -n "$ns" -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
            [[ "$phase" == "Running" && "$ready" == "true" ]] && { echo "$pod"; return 0; }
        fi
        sleep 1
    done
    return 1
}

cleanup() {
    local yaml="$1"
    echo "$yaml" | "$KUBECTL" --server="$SERVER" delete -f - 2>/dev/null || true
}

summary() {
    echo ""
    echo -e "${GREEN}Passed: ${PASS}${NC}, ${RED}Failed: ${FAIL}${NC}"
    return $FAIL
}
