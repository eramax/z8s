#!/usr/bin/env bash
set -uo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"

echo "=== Hub-and-Spoke: Setup + Verify ==="
echo ""

# Run setup (creates resources, waits for readiness, saves state)
DATA_DIR="${DATA_DIR:-/tmp/z8s-test-db}" "$DIR/test_hub_spoke_setup.sh"

# Run verify
DATA_DIR="${DATA_DIR:-/tmp/z8s-test-db}" "$DIR/test_hub_spoke_verify.sh"
