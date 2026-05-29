# NetMux Code Review — Final Deep Review

> **Review date:** 2026-05-29
> **Scope:** All files under `src/netmux/` plus integration points
> **Test status:** 178 integration tests passing

---

## 1. Dead Code

### 1.1 Functions defined but never called

| Function | File | Notes |
|---|---|---|
| `add_host_route` | `routing.rs:8` | Only referenced in test. `veth::add_pod_host_route` used instead. |
| `del_host_route` | `routing.rs:15` | Never called. |
| `del_default_route` | `routing.rs:37` | Never called. |
| `add_pod_default_route` | `routing.rs:44` | Never called. `veth::add_default_route` used instead. |
| `delete_veth_by_index` | `veth.rs:89` | Never called. Only `delete_veth` (by name) is used. |
| `list_iface_addrs` | `netlink.rs:490` | Entire function dead. Parses `/proc/self/net/fib_trie` with a `HashSet` that discards the `ifindex` parameter entirely — the `ifindex` argument is ignored and all non-loopback IPs are collected. |
| `remove_snat` | `nftables.rs:141`, `mod.rs:196` | Never called from outside netmux. |
| `remove_dnat` | `nftables.rs:216`, `mod.rs:206` | Always a no-op placeholder (only logs `info!`). The `ServiceResource::on_delete` calls `remove_service` → `remove_dnat` — which does nothing. |
| `apply_hub_spoke` | `vnet_controller.rs:68` | Defined but never called. Hub/Spoke CRDs are registered with `CrdWatcher` but `apply_hub_spoke` is never dispatched. |
| `NetworkPolicyController::update_pod` | `np_controller.rs:104` | Never called. Pod IP changes never update NetworkPolicy sets. |
| `NetworkPolicyController::remove_pod` | `np_controller.rs:123` | Never called. Pod deletions never clean NetworkPolicy sets. |
| `IngressController::start_http` | `ingress.rs:38` | L7 HTTP listener is defined but **never spawned**. The ingress module is effectively dead code at runtime — routes are populated into `IngressState` by `CrdWatcher` on Ingress CRD apply, but no TCP listener processes them. |
| `clean_orphan_veths` | `mod.rs:234`, `veth.rs:167` | Explicitly called out in the implementation report (§2) as "defined but never called at startup". NetMux exposes the method but `main.rs` never invokes it. |

### 1.2 The `network_engine` field in `ProcessSupervisor`

```rust
// runtime.rs:98
pub network_engine: Option<Arc<dyn crate::netmux::network::NetworkEngine + Send + Sync>>,
```

Field exists but is always initialized to `None` (`runtime.rs:123`). The implementation report says it was "added for triggering service sync on pod IP assignment" but the field is never read or used — service sync is done directly through `ReconcileContext::net` in `PodResource::reconcile`. **Dead field — should be removed.**

### 1.3 `ResourceStore` unused field

The `netmux/network.rs` trait defines `dns_port()`, `sync_service()`, `remove_service()`, `sync_services_for_labels()`, `compute_endpoints()`, `compute_endpointslices()`. The `CrdWatcher` never uses the `NetworkEngine` trait — it creates `VNetController`/`NetworkPolicyController` fresh each time. The `store` field in `CrdWatcher` is only used for Ingress dispatch (line 57-58 of `crd_component.rs`), not for VNet/NSG/NetworkPolicy.

---

## 2. Performance Issues

### 2.1 Shelling out to `nft` at startup (CRITICAL)

```rust
// nftables.rs:112-115
let _ = std::process::Command::new("nft")
    .args(["flush", "chain", "ip", NAT_TABLE, "prerouting"]).status();
let _ = std::process::Command::new("nft")
    .args(["flush", "chain", "ip", NAT_TABLE, "output"]).status();
```

- **Blocking call** in async context (`std::process::Command` is synchronous, blocks the tokio thread).
- **Violates plan contract** ("No shelling out to ip, iptables, nft, or any host binary").
- **Startup flush kills all DNAT rules** but is not atomic — there's a window where prerouting is flushed but output isn't (or vice versa).
- **Fix:** Should use `rustables::Batch` with `MsgType::Del` on a wildcard rule handle, or delete the chain and recreate it atomically.

### 2.2 Duplicate DNAT rules accumulating (CRITICAL)

`add_dnat` (`nftables.rs:158`) always calls `Batch::add` with `MsgType::Add`. The reconciler runs every 2 seconds and calls `sync_service_proxies` → `add_dnat` every cycle. Since `remove_dnat` is a no-op, DNAT rules **accumulate without bound**:

- After 5 minutes with a 2s interval: ~150 rule pairs per service
- After 30 minutes: ~900 rule pairs per service
- nftables rule count grows linearly with uptime

**Impact:** Rule lookup latency degrades. The nftables evaluation is linear through prerouting/output chains. With 100 services × 300 rules each = 30,000 rules, per-packet latency becomes measurable.

**Fix:** Use per-service chains with atomic replacement, or track rule handles and delete old rules before adding new ones.

### 2.3 `resolve_backend_pods` iterates ALL pods for EVERY service

```rust
// service.rs:134
let pod_trackers = store.get_by_kind("Pod").await;
```

Every `sync_service_proxies` call iterates the entire pod list. With `N` services and `M` pods, every service sync is O(M), making a full reconciliation O(N×M). With 100 pods and 50 services, that's 5,000 iterations per cycle (every 2 seconds).

**Fix:** Maintain an index of `(namespace, selector) → [pod_tracker]` or use the store's built-in filtering.

### 2.4 `tokio::spawn` for every pod reconciliation

```rust
// pod.rs:30
tokio::spawn(async move {
    if let Err(e) = ctx.process_tracker.start_pod(&resource).await {
```

Every `Pending` pod spawns a background task for `start_pod`. With 100 pods all transitioning to `Pending` simultaneously (e.g., after startup or bulk create), this spawns 100 concurrent `start_pod` tasks. No semaphore or throttle.

### 2.5 Blocking filesystem writes in async context

- `nftables.rs:112` — `std::process::Command::new("nft")` blocks the async thread
- `netlink.rs:434-437` — `std::fs::write("/proc/sys/...")` is synchronous
- `veth.rs:145` — `std::fs::read_dir("/sys/class/net")` is synchronous
- `dns.rs:50` — `std::fs::read_to_string("/etc/resolv.conf")` is synchronous

These are brief (microseconds) but violate DoD §3.1.

### 2.6 Aggressive reconciler interval

`reconciler.rs:23`: `Duration::from_secs(2)` — every 2 seconds, the reconciler iterates all resources and calls `sync_service_proxies` for each service. Combined with the duplicate DNAT issue above, this makes the accumulation problem 30× worse than a 60s interval would be.

---

## 3. Code Quality

### 3.1 `eprintln!` in production paths (DoD §5 violation)

- `cri/runtime.rs:569`: `eprintln!("z8s: root namespace setup failed: {}", e);`
- `cri/runtime.rs:663`: `eprintln!("z8s: execvpe(...) failed: {}...", e);`
- `cri/runtime.rs:839`: `eprintln!("z8s: namespace setup failed: {:#}", e);`
- `cri/rootfs.rs:517`: `eprintln!("z8s: WARNING: FILESYSTEM ISOLATION UNAVAILABLE...");`

All of these should be `tracing::error!`. The `eprintln!` output bypasses structured logging and won't appear in log files captured by the test suite.

### 3.2 `unwrap()` / `expect()` in production paths (DoD §6)

- `nftables.rs:44`: `writer.lock().expect("lock poisoned")` — all Mutex lock calls use expect. This is acceptable per DoD as the comment explains, but the pattern is repeated ~20 times across the codebase.
- `main.rs:96-97`: `CgroupManager::new().expect("cgroup manager init failed twice")` — a `panic!` on init failure. The first attempt logs a warning, the second panics. This is intentional (plan §15: "z8s refuses to start" for critical components), but the error message is misleading ("_twice_" is an implementation artifact).
- `service.rs:92`: `cluster_ip.parse().unwrap_or(std::net::Ipv4Addr::new(10, 96, 0, 1))` — silent fallback to a hardcoded IP if ClusterIP parsing fails. This could mask manifest errors.

### 3.3 Function length

| Function | Lines | Problem |
|---|---|---|
| `NftEngine::init` | 78 | Creates all tables, chains, rules, flushes — should be split: `create_tables()`, `add_baseline_rules()`, `flush_stale_rules()` |
| `NetworkManager::sync_service_proxies` | 79 | ClusterIP and NodePort logic intertwined. Each port iterates backends, parses named ports, calls add_dnat. |
| `ProcessSupervisor::start_pod_from_spec` | ~125 | Multi-step pipeline with placeholder management, cgroup setup, container loop — exceeds 60 lines by 2× |
| `ProcessSupervisor::spawn_root_ns_container` | ~207 | Fork + netns + veth attach + log capture + probe spawn — needs refactoring |

### 3.4 Magic numbers / hardcoded values

- `nftables.rs:52-53,58-59,66-69` — hook priorities `-100`, `0` — should be named constants
- `nftables.rs:` — `libc::NFPROTO_IPV4 as u8` — should be `const NFPROTO_IPV4: u8 = 2;`
- `cluster.rs:58`: `b[0] = true` — hardcoded block index 0 for self-assignment, no named constant
- `dns.rs:6`: `const TTL: u32 = 30` — fine, but `MAX_UDP: 4096` is fine too

### 3.5 Missing doc comments on public items

- `netlink.rs:56` — `netlink_socket()`: doc comment is minimal ("Open a netlink socket with explicit bind" — acceptable)
- `netlink.rs:88` — `recv_nlmsg()`: OK
- `vnet_controller.rs:21,55,68` — `apply_nsg`, `apply_vnet`, `apply_hub_spoke`: all have basic doc comments

Most public items in netmux have doc comments (implementation report fixed this). No violations found.

### 3.6 `#[allow(unused)]` or `#[cfg(never)]`

None found in the netmux tree. The dead code is genuinely dead (no annotations masking it).

---

## 4. Debugging Artifacts

### 4.1 `tracing::info!` that should be `tracing::debug!` or `trace!`

These are `info!` level messages added during debugging that produce excessive log output in production:

- `service.rs:57`: `"sync_service_proxies {}: empty selector, skipping"` — fires once per headless/external-name service
- `service.rs:60`: `"sync_service_proxies {}: selector={:?}"` — fires on every sync (every 2s per service)
- `service.rs:91`: `"resolve_backend_pods for {}: found {} backends"` — per-service, every sync
- `service.rs:93`: `"Service {} → ClusterIP {} — adding DNAT with {} backends"` — per-service, every sync (and the DNAT is added even if it already exists — see duplicate rules above)
- `nftables.rs:136`: `"nftables: added MASQUERADE for VNet '{}' (CIDR {})"` — once per VNet, acceptable
- `nftables.rs:211`: `"nftables: DNAT {}:{} -> {} backends"` — per-service, every sync

A 24/7 system with 50 services would log ~21,600 lines/day just from these two DNAT messages.

### 4.2 `ServiceResource::on_apply` logging

```rust
// service.rs:459
tracing::info!("ServiceResource::on_apply for ...");
```

This fires on every Service CRD apply. Useful during debugging but should be `debug!` in production.

### 4.3 DNS server query logging

```rust
// dns.rs:201
debug!("DNS query: {} type={}", name, qtype);
```

This is at `debug!` level — appropriate. No change needed.

### 4.4 `rollback_veth` function effectiveness

```rust
// mod.rs:249-255
fn rollback_veth(&self, pod_uid: &str, pod_ip: &Ipv4Addr, host_ifindex: u32) {
```

Called when `bring_up_veth` or `add_pod_host_route` fails. Issues:
- If `attach_pod` itself fails at `allocate_ip`, rollback is not called (because no IP was allocated yet — correct).
- If `create_pod_veth` succeeds but `bring_up_veth` fails, the veth pair exists in the kernel but is never cleaned up by `rollback_veth` → **stale veth leak** on partial failure.
- The `host_ifindex` passed to `rollback_veth` is from `create_pod_veth` — correct.

### 4.5 `IngressState` — shared state via `NetMux`

```rust
// mod.rs:30
pub ingress_state: Arc<crate::netmux::ingress::IngressState>,
```

The `IngressState` is populated by `CrdWatcher` via `IngressController::apply_ingress` (`crd_component.rs:56-60`). However, `IngressController::start_http` is **never called** — the L7 TCP listener is not spawned anywhere in `main.rs`. The ingress state is populated but never consumed. The `IngressState::routes` `RwLock` is only written to, never read at runtime (the `handle_connection` function that reads it is never invoked). **The entire ingress module is dead code at runtime.**

---

## 5. Plan Deviations

### 5.1 "No shelling out to ip, iptables, nft, or any host binary" — VIOLATED

Plan §2, line 6: > "No shelling out to `ip`, `iptables`, `nft`, or any host binary"

`nftables.rs:112-115` shells out to `nft flush chain` via `std::process::Command`. This is a direct violation of a core contract. The implementation report acknowledges this as a known issue (§4 "Current Limitations").

### 5.2 "rtnetlink operations via minimal libc FFI (~50 lines)" — EXCEEDED

Plan §2, line 9: > "rtnetlink operations (`RTM_NEWLINK`, `NEWADDR`, `NEWROUTE`) via minimal `libc` FFI (~50 lines)"

`netlink.rs` is 511 lines. Even excluding blank lines, comments, and constants, the actual FFI operations are ~200+ lines. The "~50 lines" estimate was optimistic by ~10×.

### 5.3 Per-service nftables chains for DNAT — NOT IMPLEMENTED

Plan §6.3 specifies:

```
chain clusterip-nginx {
    ip daddr 10.96.0.3 tcp dport 80 dnat to numgen inc mod 2 map { ... }
}
chain prerouting {
    jump clusterip-nginx
}
```

The current implementation adds flat DNAT rules directly to `prerouting` and `output` chains in `add_dnat` (`nftables.rs:182-207`). Each backend gets its own rule (not a `numgen` map). No per-service chains exist. This means:
- Remove is hard (no handle, no chain to delete)
- Rule count scales with backends × services
- `numgen` round-robin is not used — shuffle at add time provides static ordering

### 5.4 `numgen` round-robin — NOT IMPLEMENTED

Plan §6.3 specifies `numgen inc mod N map` for true kernel round-robin. Current implementation just shuffles the backend list once at `add_dnat` time and adds N sequential rules. The shuffle is deterministic per-add but doesn't distribute across connections. Implementation report acknowledges this (§3).

### 5.5 Rule-handle based deletion vs startup flush — NOT IMPLEMENTED

Plan assumes atomic replacement via `Batch`. Current approach uses startup `nft flush chain` (shelling out) and never removes rules during runtime. Implementation report (§1 "Current Limitations"): "Each reconciler cycle adds another set of DNAT rules without removing old ones." The per-service chain approach from §6.3 would solve this (delete + recreate the chain atomically), but it was not implemented.

### 5.6 `clean_orphan_veths` never called at startup

Plan §15: "clean_orphan_veths at startup removes orphans before creates." The function exists but **no code path calls it**. References:
- Implementation report (§2): "clean_orphan_veths is defined but never called at startup"
- `main.rs`: no call to `netmux.clean_orphan_veths()`
- `mod.rs:234`: method exists but is never invoked

### 5.7 VNet/Subnet per-VNet CIDR allocation not implemented

Plan §3.1 specifies per-VNet /20 sub-allocation from the cluster pod CIDR. The current `IpPool` (`pool.rs`) is a flat single pool — there is no per-VNet allocation. All pods share one global CIDR pool.

---

## 6. Summary

### Must-fix before production (P0)

| Issue | Impact | Effort |
|---|---|---|
| **Duplicate DNAT rules (accumulation)** | nftables rule count grows without bound → latency degrades over time. After 24h at 2s interval: ~43,000 duplicate rules per service. | Medium — implement per-service chains with atomic replace |
| **`nft flush chain` shelling out** | Blocks async thread, violates core "no host binary" contract, not atomic | Small — use `rustables` to delete+recreate chains in one batch |
| **`remove_dnat` is a no-op** | DNAT rules never cleaned up on service deletion. Dead backends accumulate indefinitely. | Medium — same fix as per-service chain approach |
| **Ingress listener never spawned** | L7 ingress is completely non-functional despite the code existing. | Small — spawn `start_http` in `main.rs` or remove the dead code |

### Should-fix (P1)

| Issue | Impact | Effort |
|---|---|---|
| `resolve_backend_pods` O(N×M) | Full reconciliation cost grows linearly with both pods and services | Medium — add selector index |
| `tokio::spawn` unbounded for pod starts | 100 concurrent pod starts on restart could overwhelm the system | Small — add semaphore |
| `network_engine` field dead code | Misleading, unused field in `ProcessSupervisor` | Trivial — remove |
| `rollback_veth` partial failure leak | Stale veth if `bring_up_veth` fails | Small — fix error path |
| `eprintln!` in production | Bypasses structured logging, breaks log capture | Small — replace with `tracing::error!` |
| `clean_orphan_veths` never called | Stale veths persist across crashes | Trivial — add call in `main.rs` |
| NetworkPolicy `update_pod`/`remove_pod` never called | NP sets never populated at runtime — NetworkPolicy is dead code | Medium — wire into pod lifecycle |

### Nice-to-have (P2)

| Issue | Impact | Effort |
|---|---|---|
| Per-VNet /20 sub-allocation from plan §3.1 | All pods share one flat pool — no VNet boundary enforcement | Medium |
| `numgen` round-robin from plan §6.3 | Shuffle is not real round-robin | Medium (blocked on rustables) |
| `hub_spoke` dead code | Hub/Spoke CRD watcher is registered but `apply_hub_spoke` never called | Small |
| Log levels: `info!` → `debug!` for per-cycle messages | Excessive log volume | Trivial |
| 2s reconciler interval | Too aggressive for steady state; combine with event-driven triggers | Small — use watch channels |
| `list_iface_addrs` dead code | Function with ignored parameter | Trivial — remove |
| `routing.rs` dead code | 4 of 5 functions never called | Trivial — remove |
| `delete_veth_by_index` dead code | Never called | Trivial — remove |
| `remove_snat` unused | Never called outside netmux, but is a useful public API | Low |
