# NetMux — Remaining Work

> Based on `docs/network-architecture-plan.md` and Phase 0 test results.
> Last updated: 2026-05-29

## Priority Key

| Priority | Meaning |
|----------|---------|
| **P0** | Blocks Phase 0 test from passing. Must fix. |
| **P1** | Feature is partially implemented but missing core logic. |
| **P2** | Feature infra exists but controller body never written. |
| **P3** | Future phase (multi-node, TLS, performance). |

---

## 1. Phase 0 Test Failures (P0)

### 1.1 A1 — Pod has no `eth0` (interface named `veth-*`)

- **Planned:** Pod interface named `eth0` (Phase 1, §14 A1)
- **Actual:** Pod interface is named `veth-<uid8>-e` (the peer veth name)
- **Fix:** Either rename peer to `eth0` inside pod netns, or fix the test to match `veth-*`
- **Effort:** Trivial — test regex fix or add `ip link set name eth0` in `configure_pod_netns`

### 1.2 A5 — Same IP after delete/recreate

- **Planned:** Different IP (§14 A5)
- **Actual:** Pool returns lowest free IP — same one since it was just released
- **Decision:** You said this is correct behavior. Update test to expect valid IP (not 127.0.0.1), not necessarily a different one.
- **Effort:** Trivial — test fix

### 1.3 A6 — `ip: command not found`

- **Planned:** `ip route show` for /32 route check
- **Actual:** `ip` binary not available in the test environment
- **Fix:** Use `cat /proc/net/route` or install `iproute2`
- **Effort:** Trivial — install package or parse /proc

### 1.4 B2 — Round-robin only hits one backend

- **Planned:** Traffic distributed across backends (§14 B2)
- **Actual:** Alpine's `nc` exits after first connection — second backend never receives traffic
- **Fix:** Use a proper HTTP server (`python3 -m http.server`, `http-echo`, or `socat`) instead of `nc`
- **Effort:** Small — test fix

### 1.5 B3 — DNAT broken after backend replacement

- **Planned:** New IP in DNAT map within ~1s (§14 B3)
- **Actual:** Same `nc` issue as B2 — initial connection also empty
- **Fix:** Same as B2 — use persistent server instead of `nc`
- **Effort:** Small — test fix

### 1.6 B4 — Empty ClusterIP exits 0 instead of refused

- **Planned:** Timeout / connection refused (§14 B4)
- **Actual:** TCP connect succeeds, just no data. `nc` exits 0.
- **Fix:** Check for timeout or empty response, not `Connection refused`
- **Effort:** Trivial — test fix

### 1.7 B5 — `nc: not found` on host

- **Planned:** NodePort works from external (§14 B5)
- **Actual:** `nc` binary not installed on the host
- **Fix:** Install `nmap-ncat` or use `/dev/tcp` bash built-in
- **Effort:** Trivial — install package

### 1.8 C3 — Pod-to-pod traffic SNATted (or test broken)

- **Planned:** Pod-to-pod retains source IP (§14 C3, §4.5)
- **Actual:** `nc -e` unsupported in busybox alpine — no way to echo source IP
- **Fix:** Use `socat` or a simple HTTP echo server instead of `nc -e`
- **Effort:** Small — test fix

### 1.9 F2 — External DNS fails (write to 127.0.0.1 refused)

- **Planned:** External domains forwarded to upstream DNS (§14 F2)
- **Actual:** Pod's `/etc/resolv.conf` points to `127.0.0.1` but embedded DNS server returns `Connection refused` for external queries
- **Root cause:** The embedded DNS server (in `src/netmux/dns.rs`) cannot reach upstream resolvers from inside the pod's network namespace. The pod tries to reach 127.0.0.1:53 which is the DNS server in the HOST netns, but from inside the pod's netns, 127.0.0.1 is the pod's own loopback.
- **Fix:** DNS server must listen on the veth gateway IP (e.g., 10.42.0.1) not 127.0.0.1. Pod's `/etc/resolv.conf` must point to the gateway IP.
- **Effort:** Medium — DNS architecture change

### 1.10 G1 — Ingress CRD not registered

- **Planned:** HTTP by Host header works (§14 G1)
- **Actual:** `no matches for kind "Ingress" in version "v1"` — CRD kind not registered with API server
- **Root cause:** The `api/handlers/` don't know about the Ingress kind. The `Ingress` variant exists in `AnyResource` and YAML parsing, but the API server's resource registry doesn't include it.
- **Fix:** Register Ingress as a recognized kind in the API server handler
- **Effort:** Medium — API server registration

---

## 2. VNet / Subnet / NSG (Phase 3 — P1/P2)

### 2.1 VNet CIDR allocation (P1)

- **Planned:** Per-VNet /20 sub-allocation from pod CIDR (§3.1, §5)
- **Actual:** `IpPool::allocate_subnet()` implemented but never called. `CrdWatcher` passes CIDR from `VNet.spec.cidr` or falls back to hardcoded `"10.42.0.0/20"` — no pool-based allocation.
- **Fix:** Wire `NetMux::allocate_subnet()` into VNet controller. Store per-VNet pool in `NetMux`.
- **Effort:** Medium

### 2.2 Cross-VNet isolation (P1)

- **Planned:** Different VNets cannot communicate by default (§4.2, §14 D2)
- **Actual:** No per-VNet nftables sets exist. No forward chain rules enforce VNet boundaries.
- **Fix:** Create nftables sets per VNet with pod IPs. Add `ip saddr @vnet-A ip daddr @vnet-B drop` rules.
- **Effort:** Medium

### 2.3 Subnet controller (P2)

- **Planned:** Subnets organize pods by function within a VNet (§3.3)
- **Actual:** `CrdWatcher` dispatches Subnet but only logs `"Subnet applied"` — no controller function
- **Fix:** Implement `apply_subnet()` that creates nftables sets for the subnet CIDR
- **Effort:** Small

### 2.4 Hub-and-Spoke controller (P2)

- **Planned:** Hub reaches spoke, spoke cannot reach spoke directly (§8, §14 D6-D8)
- **Actual:** `apply_hub_spoke()` was deleted during code review cleanup. `CrdWatcher` only logs `"Hub/Spoke applied"`.
- **Fix:** Reimplement `apply_hub_spoke()` that adds forward chain rules for spoke isolation
- **Effort:** Medium

### 2.5 RouteTable controller (P2)

- **Planned:** RouteTable CRD for custom routes (§3)
- **Actual:** No controller function. `CrdWatcher` only logs.
- **Fix:** Implement `apply_route_table()` that calls `netlink::add_route()` for each entry
- **Effort:** Small

### 2.6 NSG rule enforcement (P1)

- **Planned:** NSG rules compile to forward chain (§14 D3-D5)
- **Actual:** `apply_nsg()` calls `add_forward_allow()` / `add_forward_deny()` which work, but they add rules that never get cleaned up (accumulate across reconciler cycles)
- **Fix:** Add rule handle tracking or per-NSG chains for NSG rules (same pattern as per-service DNAT chains)
- **Effort:** Medium

---

## 3. NetworkPolicy (Phase 4 — P1/P2)

### 3.1 Runtime set population (P1)

- **Planned:** Pod IPs added/removed from nftables sets on start/stop (§6.4, §14 E1-E4)
- **Actual:** `update_pod()` / `remove_pod()` wired into pod lifecycle. But `labels_match_selector()` compares against the NetworkPolicy's `podSelector` which may not match if the policy uses `namespaceSelector` or `ipBlock`.
- **Fix:** The controller creates sets on `apply_network_policy()` but never populates them with initial matching pods. Need to scan existing pods on apply.
- **Effort:** Medium

### 3.2 Forward chain enforcement (P1)

- **Planned:** NetworkPolicy rules on the forward chain (§6.2)
- **Actual:** `add_forward_allow_set_src()` creates rules, but they reference nftables sets that may be empty.
- **Fix:** Ensure sets are populated before rules reference them. Atomic batch for set+rule creation.
- **Effort:** Small

---

## 4. DNS (P1)

### 4.1 External DNS forwarding (F2 fix)

- **Planned:** External domains forwarded to upstream DNS (§14 F2)
- **Actual:** DNS server listens on `127.0.0.1:53`. Pods can't reach it from their netns (127.0.0.1 resolves to pod's loopback).
- **Fix:** Make DNS listen on the veth gateway IP (10.42.0.1). Configure pod `/etc/resolv.conf` to point to gateway.
- **Effort:** Medium (crosses NetMux + DNS + pod lifecycle)

### 4.2 Service DNS registration (P0)

- **Planned:** `<svc>.<ns>.svc.cluster.local` resolves (§14 F1)
- **Actual:** **Works** — F1 test passes. No work needed.

---

## 5. Ingress (P1)

### 5.1 CRD registration (G1 fix)

- **Planned:** Ingress CRD accepted by API server (§14 G1)
- **Actual:** API server rejects Ingress resources — kind not registered
- **Fix:** Register Ingress in API handler routing
- **Effort:** Medium

### 5.2 Ingress from pods (P1)

- **Planned:** Pods access ingress via Host header (§14 G1)
- **Actual:** Ingress listener spawned on `0.0.0.0:80` in the HOST netns. Pods in their own netns access it via the gateway IP (10.42.0.1), not 127.0.0.1.
- **Fix:** Test must use gateway IP, not 127.0.0.1. Also need DNAT to redirect host port 80 traffic to the ingress listener.
- **Effort:** Small — test fix

### 5.3 TLS (P3)

- **Planned:** TLS by SNI, termination, auto-TLS (§14 G2, G4, G5)
- **Actual:** Not implemented
- **Effort:** Large — deferred to Phase 7

---

## 6. Multi-Node (Phase 6 — P3)

### 6.1 Node discovery

- **Planned:** Join-handshake + two-level IPAM (§10)
- **Actual:** Cluster join announce code exists in `src/netmux/cluster.rs` but never tested.

### 6.2 Cross-node routing

- **Planned:** Cross-node /24 routes via peer host IP (§10.2)
- **Actual:** `add_subnet_route_raw()` exists in `NetMux`. Unused.

### 6.3 Cross-node DNAT sync

- **Planned:** Peer pod IPs included in local DNAT maps (§10.2)
- **Actual:** Not implemented.

---

## 7. Edge Cases & Housekeeping (P2)

### 7.1 `remove_snat` unused

- Public API method exists but never called from cleanup paths. Low priority — intentional.

### 7.2 Old port publishing code

- **Planned:** Delete `port_publish.rs`, `service_proxy.rs` after Phase 6 stable (§5, Phase 5)
- **Actual:** Already removed in earlier NetMux work.

### 7.3 `numgen` round-robin

- **Planned:** `numgen inc mod N map` for true kernel round-robin (§6.3)
- **Actual:** Blocked on rustables — no `Numgen` expression support. Current shuffle-based approach is acceptable.
- **Effort:** Blocked upstream

---

## Summary by Priority

| Priority | Count | Items |
|----------|-------|-------|
| **P0** | 10 | Test script bugs (A1, A5, A6, B2, B3, B4, B5, C3), DNS external (F2), Ingress CRD (G1) |
| **P1** | 6 | VNet pool allocation, cross-VNet isolation, NSG rule cleanup, NetworkPolicy set population + enforcement, Ingress from pods |
| **P2** | 4 | Subnet controller, Hub/Spoke controller, RouteTable controller, VNet boundary nftables sets |
| **P3** | 6 | TLS, auto-TLS, multi-node discovery, cross-node routing, cross-node DNAT, old code removal |
| **Blocked** | 1 | `numgen` round-robin (upstream rustables) |
