#!/usr/bin/env bash
set -uo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DB="/tmp/z8s-test-db-$(date +%s)"

echo "=== Hub-and-Spoke: Setup + Verify + DB Persistence ==="
echo ""

# Start z8s with data-dir for persistence
echo "Starting z8s with persistent DB at $TEST_DB..."
sudo ./z8s.sh restart --data-dir "$TEST_DB" || { echo "Failed to start z8s"; exit 1; }
sleep 2

# Run setup (creates resources, waits for readiness, saves state)
DATA_DIR="$TEST_DB" "$DIR/test_hub_spoke_setup.sh"

# Run verify with restart flag (tests before + after restart)
DATA_DIR="$TEST_DB" "$DIR/test_hub_spoke_verify.sh" --restart
