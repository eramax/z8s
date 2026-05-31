#!/usr/bin/env bash
set -uo pipefail

BINARY="/home/abb/dev/z8s/target/debug/z8s"
BASE_PORT=6443
BASE_DB="/tmp/z8s-cluster"

# Kill any existing z8s instances
echo "Cleaning up..."
sudo pkill -9 -f "$BINARY" 2>/dev/null || true
sleep 2
sudo rm -rf "${BASE_DB}"-*

# Build first
echo "Building..."
(cd /home/abb/dev/z8s && cargo build 2>&1 | tail -1) || true

# Node A
echo "Starting Node A (port 6443)..."
setsid sudo "$BINARY" --port 6443 --node-name node-a --data-dir "${BASE_DB}-a" --peers node-b=127.0.0.1:7443,node-c=127.0.0.1:8443 > "${BASE_DB}-a.log" 2>&1 &

# Node B
echo "Starting Node B (port 7443)..."
setsid sudo "$BINARY" --port 7443 --node-name node-b --data-dir "${BASE_DB}-b" --peers node-a=127.0.0.1:6443,node-c=127.0.0.1:8443 > "${BASE_DB}-b.log" 2>&1 &

# Node C
echo "Starting Node C (port 8443)..."
setsid sudo "$BINARY" --port 8443 --node-name node-c --data-dir "${BASE_DB}-c" --peers node-a=127.0.0.1:6443,node-b=127.0.0.1:7443 > "${BASE_DB}-c.log" 2>&1 &
sleep 3
curl -s http://localhost:8443/healthz || { echo "Node C failed"; exit 1; }

echo ""
echo "=== Cluster ready ==="
echo "  Node A: http://localhost:6443 (PID $(pgrep -f "$BINARY.*6443" | head -1))"
echo "  Node B: http://localhost:7443 (PID $(pgrep -f "$BINARY.*7443" | head -1))"
echo "  Node C: http://localhost:8443 (PID $(pgrep -f "$BINARY.*8443" | head -1))"
echo ""
echo "Switch between nodes:  source tests/cluster/kc.sh"
echo "  k1 get pods -A    # query node A"
echo "  k2 get pods -A    # query node B"
echo "  k3 get pods -A    # query node C"
echo ""
echo "Tail logs: z8s-logs"
echo "Stop all:  sudo pkill -f \"\$BINARY\""
