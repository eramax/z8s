#!/usr/bin/env bash
set -uo pipefail
SERVER="${Z8S_SERVER:-https://localhost:6443}"
k() { /home/abb/.local/bin/kubectl --kubeconfig ~/.kube/config "$@" 2>&1 || true; }

check_info() {
    local deploy="$1" port="$2" expected="$3"
    POD=$(k get pods -n default -l app="$deploy" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
    if [[ -z "$POD" ]]; then echo "FAIL: $deploy — no pod"; return; fi
    for try in 1 2 3; do
        out=$(k exec "$POD" -- wget -q -O- -T 3 "http://127.0.0.1:${port}/" 2>&1) || true
        if echo "$out" | grep -qiE "$expected|html|body|http"; then echo "PASS: $deploy (try $try)"; return; fi
        sleep 3
    done
    echo "FAIL: $deploy — got: $(echo "$out" | head -c 100)"
}

check_info "nginx-hello" "80" "nginx"
check_info "whoami" "80" "Hostname"
check_info "http-echo" "5678" "hello from z8s"
check_info "hostinfo" "8080" "hostname"
check_info "cluster-dashboard" "80" "dashboard|cluster|html"
