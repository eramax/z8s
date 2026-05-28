#!/usr/bin/env bash
set -uo pipefail
SERVER="${Z8S_SERVER:-http://localhost:6443}"
k() { /home/abb/.local/bin/kubectl --server="$SERVER" "$@" 2>&1 || true; }

CLIENT=""
for try in python-pod logger-pod postgres-pod; do
    if k exec "$try" -- wget --version >/dev/null 2>&1; then CLIENT="$try"; break; fi
done
if [[ -z "$CLIENT" ]]; then
    k apply -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata: {name: svc-client, namespace: default}
spec: {containers: [{name: client, image: alpine:latest, command: ["sleep", "infinity"]}]}
EOF
    for i in $(seq 1 30); do
        phase=$(k get pod svc-client -o jsonpath='{.status.phase}' 2>/dev/null)
        ready=$(k get pod svc-client -o jsonpath='{.status.containerStatuses[0].ready}' 2>/dev/null)
        if [[ "$phase" == "Running" && "$ready" == "true" ]]; then CLIENT="svc-client"; break; fi
        sleep 1
    done
fi
echo "Client: $CLIENT"
[[ -z "$CLIENT" ]] && { echo "FAIL: no client"; exit 1; }

test_svc() {
    local svc="$1" port="$2" expected="$3" label="$4"
    ip=$(k get svc "$svc" -n default -o jsonpath='{.spec.clusterIP}' 2>/dev/null); [[ -z "$ip" ]] && ip="$svc"
    for try in 1 2 3; do
        out=$(k exec "$CLIENT" -- wget -q -O- -T 3 "http://${ip}:${port}/" 2>&1) || true
        if echo "$out" | grep -qiE "$expected"; then echo "PASS: $label (try $try)"; return; fi
        sleep 2
    done
    echo "FAIL: $label (clusterIP=$ip) — $out"
}

test_svc "python-svc" "18080" "directory listing|http|html" "python HTTP server"
test_svc "nginx-svc" "80" "nginx|html|welcome" "nginx default page"
test_svc "whoami-svc" "80" "Hostname|IP|hostname" "whoami info page"
test_svc "http-echo-svc" "5678" "hello from z8s" "http-echo text"
    test_svc "hostinfo-svc" "18081" "Hostinfo|hostname|Hostname" "hostinfo page"
test_svc "nginx-hello-svc" "80" "Server|server|html" "nginx-hello page"
