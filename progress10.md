# z8s Refactoring Progress

## Status: In Progress

## Completed

### T1: Workspace Structure ✅
- Renamed `src` → `src2` (old code visible for reference)
- Created workspace with 8 crates: `core`, `runtime`, `network`, `sync`, `controller`, `api`, `z8s`, `z8s-node`
- Created stub Cargo.toml + lib.rs for all crates

### T2: core/types + core/store + core/syscall ✅

**core/types/** — All Kubernetes-compatible resource types
- `meta.rs` — ObjectMeta, Time, OwnerReference, ManagedFieldsEntry
- `resource.rs` — Resource trait, AnyResource enum (21 variants), ResourceRecord, ResourceStatus, Phase, Condition, ContainerStatus
- `compute.rs` — Pod, PodSpec, ContainerSpec, Deployment, DeploymentSpec, Volume, VolumeMount, Probe, SecurityContext, EnvVar
- `network.rs` — Service, ServiceSpec, EndpointSlice, VNet, Subnet, NSG, NetworkPolicy, Ingress, RouteTable
- `storage.rs` — PersistentVolume, PersistentVolumeClaim, StorageClass, ConfigMap, Secret
- `control.rs` — Namespace, Node, NodeStatus, Event, ServiceAccount, Role, RoleBinding
- `event.rs` — EventRecord, EventType, reasons constants
- `helpers.rs` — Quantity, IntOrString, time utilities (now_rfc3339, now_epoch_ms, parse_rfc3339_secs)

**core/store/** — Persistent storage backend
- `backend.rs` — StoreBackend trait (write_spec, assign_node, write_status, get, get_by_kind, get_by_node, get_unassigned, get_needing_reconcile, delete, apply_batch, snapshot, record_event, get_events, get_events_by_kind, get_recent_events, prune_events)
- `redb.rs` — RedbBackend with in-memory index for O(1) lookups
- `memory.rs` — MemoryBackend for testing
- `hub.rs` — StoreEventHub (reactive event distribution via broadcast)
- `ops.rs` — StoreOp, StoreEvent, StoreChange
- `snapshot.rs` — StoreSnapshot (point-in-time view)

**core/syscall.rs** — Direct Linux syscalls via rustix 1.1.4
- mount, umount2, chroot, chdir, pivot_root — filesystem operations
- unshare, sethostname, setns — namespace operations
- fork — inline asm (x86_64 + aarch64)
- execve — via rustix::runtime
- kill, pipe2 — process/signal/pipe management
- dup2_stdout/stderr/stdin — raw syscall 33 (rustix dup2 needs &mut OwnedFd)
- ioctl — raw syscall 16 (rustix only exposes specific ioctls)
- setuid, setgid — raw syscalls 105/106 (not in rustix 1.1.4)
- mknod — raw syscall 133 (not in rustix 1.1.4)
- flock_exclusive, flock_unlock
- write_uid_map, write_gid_map, write_setgroups

**Package rename:** `core` → `z8s_core` (prevents shadowing Rust's `core` crate which breaks `async_trait`)

**Warnings fixed:**
- helpers.rs: removed unnecessary `mut`
- network.rs: fixed snake_case naming (srcCIDRs → src_cidrs, etc.)
- redb.rs: fixed unused variable, fixed redb API compatibility

### T3: runtime — Container Lifecycle ✅

**runtime/src/lib.rs** — RuntimeProvider trait (async, object-safe)

**runtime/src/spec.rs** — ContainerSpec, ContainerConfig, ResolvedVolume, ContainerConfigBuilder (fluent builder)

**runtime/src/health.rs** — HealthChecker, ProbeConfig, ProbeAction (Exec/HTTPGet/TCPSocket), HealthStatus

**runtime/src/cgroup.rs** — CgroupManager (cgroups v2: memory.max, memory.low, cpu.max, pids.max, cgroup.procs), apply_limits()

**runtime/src/rootfs.rs** — Filesystem isolation (Pivot/Chroot/Degraded):
- prepare_rootfs, setup_container_rootfs, child_enter_ns_fork
- drop_capabilities (OCI default set), apply_landlock (LSM)
- resolve_exec_path, build_container_argv, wrap_dynamic_linker
- bind_mount_volumes
- mount propagation via MountPropagationFlags (DOWNSTREAM/PRIVATE)

**runtime/src/image.rs** — ImageManager (OCI pull via oci-distribution 0.11, layer cache, overlay mount, OCI config save/read/guess)

**runtime/src/exec.rs** — Container exec (build_command with namespace entry, set_winsize via ioctl, PTY support)

**runtime/src/supervisor.rs** — ContainerSupervisor (orchestrates spawn lifecycle):
- spawn_isolated (double-fork root / single-fork userns)
- write_userns_maps (newuidmap/subid/direct fallback)
- RunningContainer tracking, log collection, probe management
- Pure functions: merge_env, is_pid_alive, spawn_container_probes

**Overlay (no fallback):**
- Removed copy_dir fallback — overlay mount now required
- If mount fails, error propagates (no degraded mode)
- Tests use `sudo` with CAP_SYS_ADMIN
- Overlay only works on ext4 paths (`/home/abb`), not on container overlay root (`/tmp`)

**Tests:**
- 23 core unit tests + 22 store tests + 45 runtime tests = 90+ total
- Integration tests: alpine, ubuntu, python, postgres, nginx, http-echo, busybox
- All tests passing (image tests require network, ignored in CI)

**Key design decisions:**
- Renamed `core` → `z8s_core` to avoid Rust `core` shadowing
- All syscalls use rustix 1.1.4 except: fork, ioctl, setuid, setgid, mknod, dup2 (raw asm for missing APIs)
- Mount propagation uses `MountPropagationFlags` + `mount_change()` (not `MountFlags`)
- `execve` via `rustix::runtime::execve` (returns Errno, not Result)
- Signal names: `Signal::KILL`/`Signal::TERM` (not SIGKILL/SIGTERM)
- `LinkNameSpaceType::User`/`Mount`/`Network`/`ProcessID` (PascalCase)
- No `libc` crate — zero external C dependencies

## Not Started

### T4: network — Veth, nftables, DNS, IPAM ✅

**12 modules**, clean split between pure `planner` and side-effecting `sync`:

**network/src/ipam.rs** — IpPool (BTreeSet free-list), Ipv4Cidr parse/normalize,
subnet allocation, gateway/broadcast reservation, expand-to-adjacent

**network/src/ipv6.rs** — Ipv6Pool from /64 prefix, IID allocation with release/reuse

**network/src/netlink.rs** — Raw RTNETLINK (no `libc` dep, `core::ffi::c_void` alias):
socket, veth create, link up/down, addr add/del, route add/del, setns helpers

**network/src/veth.rs** — veth pair management: `veth-<uid8>` / `zeth-<uid8>` naming,
host route, gateway IP, default route, netns guard, orphan cleanup

**network/src/nft.rs** — NftEngine (rustables 0.8.7): SNAT, ClusterIP DNAT,
NodePort DNAT, NSG allow/deny, set membership, catch-all chain

**network/src/dns.rs** — UDP DNS server with DnsSnapshot (in-memory cache),
service name parser, A/CNAME answers, upstream forwarding

**network/src/ingress.rs** — L7 HTTP listener (Host: header → Service backend),
IngressState with host→(svc,port) routing

**network/src/np_controller.rs** — NetworkPolicy controller: nftables sets with
pod-selector membership, `update_pod` / `remove_pod` for live sync

**network/src/planner.rs** — Pure desired-state planner from StoreSnapshot:
ClusterIP/NodePort DNAT, DNS records, NSG rules, remote pod routes

**network/src/sync.rs** — Reconciler entry: `reconcile_network(snap, nft, dns,
ingress, npc, node_name, gateway, store)` applies planner output

**network/src/state.rs** — RuleKey (stable identity for idempotent ops), TableId,
chain naming

**network/src/rule.rs** — Declarative NftAction (Accept/Drop/DNAT/SNAT/Masq/Jump)
and NftRule with builder

**network/src/lib.rs** — `NetMux` facade, `NetworkEngine` trait, `PodResolver` trait,
`attach_pod` / `detach_pod` / `configure_pod_netns` / `init_nft`

**Build:** `cargo check -p network` clean. **Tests:** 77/77 unit tests pass
(16 ipam + 4 ipv6 + 3 netlink + 4 veth + 4 nft + 12 dns + 5 ingress +
6 np_controller + 7 planner + 2 sync + 4 lib + 4 state + 2 rule).

**Key design decisions:**
- No `nix` / `libc` deps — direct rustix + raw syscall extern where needed
- veth created with `IFLA_NET_NS_PID` so peer lands in pod netns directly
- `NetNsGuard` ensures host netns is restored on scope exit
- DNS cache (DnsSnapshot) avoids store reads per query (PERFORMANCE-PLAN §13)
- Declarative `NftRule` data type — composable, serializable, diffable
- Planner is pure (no IO) — trivially testable, deterministic
- `unsafe extern "C"` for send/recv (Rust 2024 requirement)
- 4-bucket `#[cfg(test)] mod tests` pattern in every file

### T5: sync — Gossip, anti-entropy, vector clocks
### T6: controller — Reconcile loop (assign + reconcile)
### T7: api — HTTP handlers, auth, catalog
### T8: z8s CLI binary
### T9: z8s-node binary

## Files
- Plan: `REFACTOR-PLAN.md` (architecture, types, DB design, events)
- Performance: `PERFORMANCE-PLAN.md` (bottlenecks, optimizations)
- Old code: `src2/` (reference)
- New code: `src/` (in progress)
