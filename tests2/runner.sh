#!/usr/bin/env bash
# Run all test scripts in this directory
DIR="$(dirname "$0")"
for t in "$DIR"/test_*.sh; do
    [ -f "$t" ] || continue
    echo -e "\n=== $(basename "$t" .sh) ==="
    bash "$t"
done
