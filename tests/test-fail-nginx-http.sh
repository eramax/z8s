#!/usr/bin/env bash
set -uo pipefail
SERVER="${Z8S_SERVER:-http://localhost:6443}"
k() { /home/abb/.local/bin/kubectl --server="$SERVER" "$@" 2>&1 || true; }

POD_NAME=$(k get pods -n default -l app=nginx -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
echo "Pod: $POD_NAME"
if [[ -z "$POD_NAME" ]]; then echo "No nginx pod found"; exit 1; fi

for try in 1 2 3; do
    out=$(k exec -n default "$POD_NAME" -- wget -q -O- -T 5 http://127.0.0.1:80/ 2>&1) || true
    if echo "$out" | grep -qiE "nginx|html|welcome"; then
        echo "PASS (try $try)"; exit 0
    fi
    sleep 2
    echo "try $try: $out"
done
echo "FAIL: $out"
