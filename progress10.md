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

**7 modules**, functional core / imperative shell. Everything is expressed as
immutable structs (tables → chains → rules → sets) that a pure `reconcile`
diffs into the minimal set of kernel ops. No `rustables` / `libc` / `nix` —
only `rustix` + `neli` for socket plumbing.

**network/src/ipam.rs** — IpPool (BTreeSet free-list), Ipv4Cidr parse/normalize,
subnet allocation, gateway/broadcast reservation; Ipv6Pool from /64 prefix

**network/src/model.rs** — the declarative model: `NftFamily/Chain/Hook/Policy`,
`NftExpr` (Meta/Cmp/Payload/Lookup/Immediate/Nat/Masquerade/Bitwise/Numgen/
verdicts), `NftRule` + pure rule builders (`match_cidr`, `match_l4proto`,
`match_dport`, `dnat_to`, `clusterip_dnat_rule`, `nodeport_dnat_rule`,
`masquerade_rule`, `nsg_filter_rule`), `NftTable/Chain/Set/Counter`, `RouteSpec`,
`NetmuxState`, and `NetlinkOp` (incl. `AddRoute`/`DelRoute`)

**network/src/syscalls.rs** — nftables wire encoding over `NETLINK_NETFILTER`:
`NlaBuf` attribute builder (nested + padding), batch envelopes, correct
`NFTA_LIST_ELEM`/`EXPR_NAME`/`EXPR_DATA` + typed `DATA_VALUE`/`DATA_VERDICT`,
`NlSocket` send/ACK

**network/src/rtnetlink.rs** — `RouteSocket` over `NETLINK_ROUTE` (fresh socket
per call so it binds to the current netns): veth pair (`IFLA_NET_NS_PID` so the
peer lands in the pod netns directly), get_ifindex, set_up, del_link, add_addr,
add_route/del_route, `NetnsGuard` (rustix `move_into_link_name_space`),
`attach_pod`/`detach_pod`/`clean_orphan_veths`, `veth-<uid8>`/`zeth-<uid8>` naming

**network/src/engine.rs** — `Netmux` facade + pure `reconcile(desired, current)`:
table/chain/rule/set/route diff with automatic `Del*` cleanup, route ops routed
to `RouteSocket` vs nft ops to `NlSocket`, imperative `attach_pod`/`detach_pod`
hot path, `NetmuxBuilder`

**network/src/plan.rs** — pure `plan(snapshot, PlanConfig) -> NetmuxState` from
z8s_core DB types: per-node `z8s_nat_{node}` / `z8s_filter_{node}` tables,
ClusterIP/NodePort DNAT with cluster-wide backend resolution + numgen LB,
pod-CIDR masquerade, NSG allow/deny + whitelist default-deny, VNet isolation +
pools, NetworkPolicy pod-selector IP sets, remote pod routes via peer gateways,
DNS records

**network/src/dns.rs** — compact tokio UDP DNS server: hand-rolled RFC 1035
codec (pure `parse_query`/`build_response`), hot-swappable `DnsZone` behind
`RwLock`, A-record answers + NXDOMAIN for the cluster domain + upstream forward

**network/src/lib.rs** — re-exports, `NetworkEngine` trait, pod-state helpers

**Build:** `cargo build -p network` clean (no warnings), `cargo clippy` clean.
**Tests:** 84/84 unit tests pass.

**Key design decisions:**
- No `rustables` / `nix` / `libc` — hand-rolled nftables + RTNETLINK encoding
  over `rustix` + `neli` sockets
- Functional core (`reconcile`, `plan`, rule builders, DNS codec all pure) +
  imperative shell (`Netmux::apply`, `RouteSocket`, `DnsServer::serve`)
- `plan(snapshot, cfg)` patches DB structs straight into the engine; the diff
  emits the minimal ops and cleans up removed/stale resources automatically
- Multi-node fabric: per-node tables, remote pod `/32` routes via peer gateways
- veth created with `IFLA_NET_NS_PID`; `NetnsGuard` restores host netns on drop
- Note: `NetworkPolicyIngressRule`/`EgressRule` are empty stubs in z8s_core, so
  the planner emits pod-selector sets but no ingress/egress match rules yet

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
