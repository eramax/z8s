#!/usr/bin/env bash
set -uo pipefail
SERVER="${Z8S_SERVER:-https://localhost:6443}"
k() { /home/abb/.local/bin/kubectl --kubeconfig ~/.kube/config "$@" 2>&1 || true; }

echo "=== python-deploy scale 2->3 ==="
k scale deployment python-deploy --replicas=3
for i in $(seq 1 90); do
    ready=$(k get deployment python-deploy -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
    if [[ "$ready" -ge 3 ]]; then echo "PASS: readyReplicas=$ready"; exit 0; fi
    echo "  waiting... readyReplicas=$ready"
    sleep 1
done
ready=$(k get deployment python-deploy -o jsonpath='{.status.readyReplicas}' 2>/dev/null)
echo "FAIL: readyReplicas=${ready:-0} after 90s"
