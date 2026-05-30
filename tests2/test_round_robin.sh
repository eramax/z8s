#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# B2: Round-robin across multiple backends
# NOTE: numgen round-robin blocked on rustables. Current DNAT is first-match.
skip "B2: numgen round-robin not supported by rustables yet"
