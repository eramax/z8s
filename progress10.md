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

### T3: runtime — Container Lifecycle ✅

**runtime/src/lib.rs** — RuntimeProvider trait (async, object-safe)
**runtime/src/spec.rs** — ContainerSpec, ContainerConfig, ResolvedVolume, ContainerConfigBuilder
**runtime/src/health.rs** — HealthChecker, ProbeConfig, ProbeAction, HealthStatus
**runtime/src/cgroup.rs** — CgroupManager (cgroups v2), apply_limits()
**runtime/src/rootfs.rs** — Filesystem isolation (Pivot/Chroot/Degraded)
**runtime/src/image.rs** — ImageManager (OCI pull via oci-distribution 0.11, layer cache, overlay)
**runtime/src/exec.rs** — Container exec (namespace entry, PTY support)
**runtime/src/supervisor.rs** — ContainerSupervisor (spawn lifecycle orchestration)

**Tests:** 23 core + 22 store + 45 runtime = 90+ total, all passing

---

### T4: network — Veth, nftables, DNS, IPAM ✅

**8 source files**, functional core / imperative shell. No `rustables` / `libc` / `nix` —
hand-rolled nftables + RTNETLINK encoding over `rustix` + `nix` sockets.

**network/src/ipam.rs** — IpPool (BTreeSet free-list with `expand()`), Ipv4Cidr
parse/normalize, subnet allocation, gateway/broadcast reservation; Ipv6Pool from /64 prefix

**network/src/model.rs** — declarative model:
- `NftFamily/Chain/Hook/Policy` enums
- `NftExpr` (Meta/Cmp/Payload/Lookup/Immediate/Nat/Masquerade/Bitwise/Numgen/Conntrack/verdicts)
- `NftRule` + pure rule builders: `match_cidr`, `match_l4proto`, `match_dport`, `dnat_to`,
  `clusterip_dnat_rule`, `nodeport_dnat_rule`, `masquerade_rule`, `nsg_filter_rule`,
  `established_related_rule`
- `NftTable/Chain/Set/Counter`, `RouteSpec`, `NetmuxState`, `NetlinkOp`

**network/src/syscalls.rs** — nftables wire encoding over `NETLINK_NETFILTER`:
- `NlaBuf` attribute builder (nested + padding), batch envelopes
- Correct `NFTA_LIST_ELEM`/`EXPR_NAME`/`EXPR_DATA` + typed `DATA_VALUE`/`DATA_VERDICT`
- `NlSocket::send()` (single op) + `NlSocket::send_batch()` (multi-op batch)
- All attribute type numbers verified against kernel UAPI (v6.1)

**network/src/rtnetlink.rs** — `RouteSocket` over `NETLINK_ROUTE`:
- veth pair (`IFLA_NET_NS_PID` so peer lands in pod netns directly)
- get_ifindex, set_up, del_link, add_addr, add_route/del_route
- `NetnsGuard` (rustix `move_into_link_name_space`)
- `attach_pod`/`detach_pod`/`clean_orphan_veths`
- `configure_pod_netns()` (standalone netns reconfiguration)
- `add_local_service_cidr()` (RTN_LOCAL route for ClusterIP)
- `ensure_loopback_up()`, `list_veth_interfaces()`, `conntrack_available()`

**network/src/engine.rs** — `Netmux` facade + pure `reconcile(desired, current)`:
- table/chain/rule/set/route diff with automatic `Del*` cleanup
- Chain diff: delete+recreate when rules change (no kernel handle tracking needed)
- `ReconcileReport` for observability (per-resource-type op counts)
- `attach_pod`/`detach_pod` hot path, `NetmuxBuilder`

**network/src/plan.rs** — pure `plan(snapshot, PlanConfig) -> NetmuxState`:
- `z8s_nat_{node}` table: ClusterIP/NodePort DNAT + numgen LB, pod-CIDR masquerade
- `z8s_filter_{node}` table: established/related rule, input/output chains,
  nsg-rules chain (with default-deny), catch-all chain for pod CIDR
- VNet isolation + pools, NetworkPolicy pod-selector IP sets + lookup rules
- Remote pod `/32` routes via peer gateways
- DNS records: `svc.ns.svc.cluster.local`, `kubernetes.default.svc.cluster.local`
- RouteTable resources → state.routes, Subnet resources → state.ip_pools

**network/src/dns.rs** — compact tokio UDP DNS server:
- Hand-rolled RFC 1035 codec (pure `parse_query`/`build_response`)
- Hot-swappable `DnsZone` behind `RwLock`
- A-record answers + NXDOMAIN for cluster domain + upstream forward

**network/src/lib.rs** — re-exports, `NetworkEngine` trait, pod-state helpers,
sysctl helpers (`enable_ip_forward`, `enable_rp_filter`, `enable_arp_announce`, `harden_sysctl`)

**Build:** `cargo build -p network` clean (no warnings), `cargo clippy` clean.
**Tests:** 85 unit + 59 integration = 144 total, all passing.

**Wire encoding verified against kernel UAPI (v6.1):**
- Chain attributes: TABLE=1, NAME=3, HOOK=4(nested), POLICY=5, TYPE=7
- Set attributes: TABLE=1, NAME=2, FLAGS=3, KEY_TYPE=4, KEY_LEN=5, DATA_LEN=7, ELEMENTS=13
- Rule attributes: TABLE=1, CHAIN=2, HANDLE=3, EXPRESSIONS=4
- Object attributes: TABLE=1, NAME=2, TYPE=3, DATA=4
- NAT attributes: TYPE=1, FAMILY=2, ADDR_MIN=3, ADDR_MAX=4, PROTO_MIN=5, PROTO_MAX=6
- Verdict values: NF_DROP=0, NF_ACCEPT=1, NFT_JUMP=-3, NFT_GOTO=-4, NFT_RETURN=-5
- Strings: NOT NUL-terminated (matching rustables wire format)
- Batch: single sendmsg per batch (BATCH_BEGIN + ops + BATCH_END)
- Conntrack: CT_DREG=1, CT_KEY=2 (NFT_CT_STATE=3)

**Key design decisions:**
- No `rustables` / `nix` / `libc` — hand-rolled nftables + RTNETLINK encoding
- Functional core (`reconcile`, `plan`, rule builders, DNS codec all pure) +
  imperative shell (`Netmux::apply`, `RouteSocket`, `DnsServer::serve`)
- `plan(snapshot, cfg)` patches DB structs straight into the engine; the diff
  emits the minimal ops and cleans up removed/stale resources automatically
- Multi-node fabric: per-node tables, remote pod `/32` routes via peer gateways
- veth created with `IFLA_NET_NS_PID`; `NetnsGuard` restores host netns on drop

---

## Not Started

### T5: sync — Gossip, anti-entropy, vector clocks
### T6: controller — Reconcile loop (assign + reconcile)
### T7: api — HTTP handlers, auth, catalog
### T8: z8s CLI binary
### T9: z8s-node binary

---

## Known Gaps (see NETWORK-GAPS.md for full details)

**Not yet implementable (missing core types):**
- L7 HTTP Ingress (IngressSpec is empty stub)
- Namespace selector in NetworkPolicy (NetworkPolicyIngressRule is empty stub)
- IP block with except in NetworkPolicy
- matchExpressions operators (In, NotIn, Exists, DoesNotExist)
- Dynamic runtime set membership updates (requires controller integration)
- CNAME records / ExternalName services (DNS only has A records)
- DNS compression pointer support
- DNS AAAA/ANY query handling
- DNS upstream auto-detection from /etc/resolv.conf
- IpPool::expand() for non-adjacent ranges

## Files
- Plan: `REFACTOR-PLAN.md` (architecture, types, DB design, events)
- Gaps: `NETWORK-GAPS.md` (remaining network features)
- Performance: `PERFORMANCE-PLAN.md` (bottlenecks, optimizations)
- Old code: `src2/` (reference)
- New code: `src/` (in progress)
