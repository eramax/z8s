# Testing the Network Module

The network crate has three testing layers: **unit tests** (pure, no kernel
access), **kernel integration tests** (require CAP_NET_ADMIN), and
**end-to-end cluster tests** (require a running z8s node). Each layer catches
different classes of bugs.

## Layer 1: Unit Tests (no privileges needed)

Pure functions and wire encoding. These run in CI without root.

```bash
CARGO_TARGET_DIR=target-net cargo test -p network --lib
```

84 tests covering:

| Module | What is tested | Count |
|--------|---------------|-------|
| `model` | Family/hook enum values, set construction, rule accept/drop, state merge | 8 |
| `engine` | Reconcile add/del table/chain/rule/set/route, dry-run apply, generation, builder | 22 |
| `plan` | Per-node table creation, ClusterIP DNAT with backends, service with no backends skipped, DNS records, remote pod routes, local pod has no route | 6 |
| `syscalls` | NlaBuf put_u32/put_slice/put_str/put_nested, attribute padding, nft msg type/flags, set element encoding, expr name emission (meta/payload/bitwise/numgen/verdict), jump rule with chain | 16 |
| `rtnetlink` | rtattr padding, u32 attr layout, nlmsghdr length, uid_suffix, route body scope with/without gateway | 7 |
| `dns` | Zone case-insensitive lookup, query parse roundtrip, short packet rejection, A record answer, NXDOMAIN response | 5 |
| `ipam` | Subnet-aligned allocation, IPv6 pool | 2 |
| `lib` | Re-export sanity, builder + engine pod add/remove | 3 |

### Running with a different target dir (avoids root-owned artifacts)

If prior `sudo cargo test` runs left root-owned files in `target/`:

```bash
CARGO_TARGET_DIR=target-net cargo test -p network --lib
```

### Running with clippy

```bash
CARGO_TARGET_DIR=target-net cargo clippy -p network --lib
```

### Specific test patterns

```bash
# Just the planner tests
CARGO_TARGET_DIR=target-net cargo test -p network --lib -- plan

# Just the DNS codec tests
CARGO_TARGET_DIR=target-net cargo test -p network --lib -- dns

# Just the nftables wire encoding tests
CARGO_TARGET_DIR=target-net cargo test -p network --lib -- syscalls
```

## Layer 2: Kernel Integration Tests (requires root / CAP_NET_ADMIN)

These tests actually create veth pairs, add routes, and install nftables
rules. They require `CAP_NET_ADMIN` and are tagged `#[ignore]` so they don't
run in CI.

### Manual test: veth pair + route

```bash
sudo CARGO_TARGET_DIR=target-net cargo test -p network --lib -- ignored --test-threads=1
```

### Manual test: nftables rule installation

```bash
# Verify nft is available for post-test inspection
which nft || sudo apt install nftables

# Run a single kernel test
sudo CARGO_TARGET_DIR=target-net cargo test -p network --lib -- <test_name> --ignored --test-threads=1

# Inspect what was installed
sudo nft list tables
sudo nft list table ip z8s_nat_node-a
sudo nft list table ip z8s_filter_node-a

# Clean up
sudo nft delete table ip z8s_nat_node-a
sudo nft delete table ip z8s_filter_node-a
```

### Manual test: pod attach/detach

This requires a running container (or at least a process in a separate netns).
The `Netmux::attach_pod` call needs a PID that has its own network namespace.

```bash
# Create a process in a new netns
sudo unshare -n sleep 600 &
POD_PID=$!

# Run the attach test
sudo CARGO_TARGET_DIR=target-net cargo test -p network --lib -- attach --ignored --test-threads=1

# Or test programmatically (Rust):
#   let mut engine = Netmux::connect()?;
#   let pair = engine.attach_pod("uid-test", pod_ip, pid)?;
#   engine.detach_pod("uid-test")?;

# Clean up
sudo kill $POD_PID
```

### Manual test: DNS server

```bash
# Start the DNS server on a high port (avoids needing port 53)
sudo CARGO_TARGET_DIR=target-net cargo test -p network --lib -- dns_server --ignored --test-threads=1

# Or run it ad-hoc and query with dig:
# (requires a small test binary or tokio runtime)
dig @127.0.0.1 -p 5353 web.default.svc.cluster.local A
```

### Inspecting kernel state after tests

```bash
# Routes
ip route show | grep z8s
ip route show | grep 10.42

# Interfaces
ip link show | grep veth

# Netns
lsns -t net

# nftables
sudo nft list ruleset
```

### Cleaning up after failed kernel tests

```bash
# Remove all z8s nftables tables
for t in $(sudo nft -a list tables 2>/dev/null | grep -oP 'z8s_\w+'); do
  sudo nft delete table ip "$t" 2>/dev/null
done

# Remove orphan veths
for v in $(ls /sys/class/net/ 2>/dev/null | grep '^veth-'); do
  sudo ip link del "$v" 2>/dev/null
done

# Remove stale routes
ip route show | grep '10.42' | while read -r route; do
  sudo ip route del $route 2>/dev/null
done
```

## Layer 3: End-to-End Cluster Tests

These exercise the full stack: API server, controller, runtime, and network
working together. They require a running z8s node.

### Quick start

```bash
# Build and start a node
cargo build
sudo target/debug/z8s reset
sudo target/debug/z8s node start
sleep 7

# Run the network-specific integration scripts
./tests/run-network-fixes.sh       # ~1-3 min, covers: pod lifecycle, netns, ClusterIP proxy
./tests/run-network-failures.sh    # regression tests for known bugs
```

### What the network integration tests cover

| Script | Duration | What it tests |
|--------|----------|---------------|
| `run-network-fixes.sh` | ~1-3 min | Pod netns isolation, port publish, ClusterIP DNAT, service DNS |
| `run-network-failures.sh` | ~1-2 min | Regression tests for veth/route/nft creation failures |
| `tests/netmux/test_phase0_scenarios.sh` | ~5 min | Multi-node hub-and-spoke: remote pod routes, cross-node DNS, NSG isolation |
| `tests/netmux/test_hub_spoke.sh` | ~2 min | Two-node ping across veths |

### Manual end-to-end test

```bash
# Apply a service and verify DNAT
kubectl apply -f tests/11-service.yaml

# Check that the ClusterIP is reachable from a pod
kubectl exec -it alpine -- wget -qO- http://10.96.0.10:80

# Verify nftables rules
sudo nft list table ip z8s_nat_$(hostname)

# Check DNS resolution from inside a pod
kubectl exec -it alpine -- nslookup web.default.svc.cluster.local 10.96.0.10
```

### Multi-node test

```bash
# Start two nodes
sudo target/debug/z8s node start --port 6443
sudo target/debug/z8s node start --port 7443
sleep 7

# Run hub-spoke network tests
./tests/netmux/test_hub_spoke.sh

# Verify remote pod routes
ip route show | grep 'via'
# Should show: 10.42.1.x/32 via <peer_ip>
```

## Writing New Tests

### Unit test pattern (pure, no IO)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipam::Ipv4Cidr;

    #[test]
    fn my_pure_function_works() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
        let rule = masquerade_rule(&cidr);
        assert!(rule.exprs.iter().any(|e| matches!(e, NftExpr::Masquerade)));
    }
}
```

### Kernel integration test pattern (requires root)

```rust
#[cfg(test)]
mod kernel_tests {
    use super::*;

    #[test]
    #[ignore] // requires CAP_NET_ADMIN
    fn nft_table_create_and_delete() {
        let sock = NlSocket::open().expect("open netfilter socket");
        let op = NetlinkOp::AddTable {
            family: NftFamily::Ip,
            name: "z8s_test".into(),
        };
        sock.send(&op).expect("create table");
        // Verify with nft list tables
        let del = NetlinkOp::DelTable {
            family: NftFamily::Ip,
            name: "z8s_test".into(),
        };
        sock.send(&del).expect("delete table");
    }
}
```

Run with: `sudo cargo test -p network -- <name> --ignored --test-threads=1`

### End-to-end test pattern (shell script)

Follow the convention in `tests/run-network-fixes.sh`:

```bash
#!/usr/bin/env bash
set -eo pipefail
SERVER="${Z8S_SERVER:-https://localhost:6443}"
NS="my-test-ns"

# Create test namespace + resources
k apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: net-test
  namespace: $NS
spec:
  containers:
  - name: alpine
    image: alpine:latest
    command: [sleep, 3600]
EOF

wait_pod_ready net-test 30

# Test connectivity
k exec -n $NS net-test -- wget -qO- http://some-service:80

# Clean up
k delete ns $NS --force 2>/dev/null || true
```

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `Permission denied` on `target/debug/.fingerprint/` | Prior `sudo cargo test` left root-owned artifacts | Use `CARGO_TARGET_DIR=target-net` |
| `netlink route socket not connected` | `Netmux::unconnected()` used in production path | Use `Netmux::connect()` which opens both sockets |
| `nft list table` shows nothing after `apply` | `dry_run = true` was passed | Pass `dry_run = false` to `engine.apply()` |
| `veth-xxxxxx` still exists after detach | Process exited before detach ran | Call `engine.clean_orphan_veths(&active_uids)` |
| `setns` failed: `Invalid argument` | Container PID no longer exists | Pod may have exited; check `lsns -t net` |
| DNS returns NXDOMAIN for a service | Service has no ready backends | Pod must be Running + have `pod_ip` in status |
