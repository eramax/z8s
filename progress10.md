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
- mount, umount2, chroot, chdir
- unshare, sethostname (rustix::system), setns
- fork (inline asm, x86_64 + aarch64)
- kill, pipe2, dup2, read, write, open, set_cloexec
- flock_exclusive, flock_unlock
- write_uid_map, write_gid_map, write_setgroups

**Warnings fixed:**
- helpers.rs: removed unnecessary `mut`
- network.rs: fixed snake_case naming (srcCIDRs → src_cidrs, etc.)
- redb.rs: fixed unused variable, fixed redb API compatibility

## In Progress

### T3: runtime — Container Lifecycle
- Container spawn (fork, namespaces, pipes)
- Image pull (OCI registry)
- Rootfs (chroot, pivot_root, overlayfs)
- Exec (kubectl exec PTY + pipe)
- Health probes (exec, httpGet, tcpSocket)
- Cgroups v2

## Not Started

### T4: network — Veth, nftables, DNS, IPAM
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
