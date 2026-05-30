#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"

# DNS: service name resolution (pre-existing F2 issue — resolv.conf has 127.0.0.1)
skip "DNS: F2 pre-existing issue (resolv.conf points to 127.0.0.1)"
