# Module Plan: NetMux (Network Engine)

> Part of [00-overview.md](./00-overview.md) · **Critical path** · Current ~2,650 LOC (`netmux/*` + `components/network/*`)

NetMux is the **single data-plane authority** for z8s: IPAM, veth/pod networking, nftables NAT/filter, in-cluster DNS, L7 ingress, and network policy. Everything else (API, scheduler, CRI) **configures** NetMux; it does not improvise routes or rules on the side.

---

## 1. Design goals

| Goal | Meaning |
|------|---------|
| **One model** | Desired state in memory → diff → apply. No parallel imperative trackers (`jump_track`, per-service ad hoc calls). |
| **One reconcile** | Network changes run from a single loop keyed off store snapshot — never from per-pod `sync_services_for_labels`. |
| **Kernel-first** | ClusterIP / NodePort / pod egress via **nftables + netlink**; no userspace TCP proxy on the hot path. |
| **Safe by default** | Only touch `z8s_nat_{node}` / `z8s_filter_{node}`; never flush foreign rulesets ([incident.md](../incident.md)). |
| **Testable** | Planner is pure Rust; appliers are mockable; golden tests without root. |
| **Node-local** | Each peer runs NetMux for its pods + service frontends; cross-node via routes/gossip, not shared mutable state. |

**Out of scope (for now):** cloud LB integration (Azure/AWS annotation labels), CNI plugins, full IPv6 production path, eBPF replacement of nftables.

---

## 2. Current architecture (as-built)

```
                    ┌─────────────────────────────────────┐
                    │  components/network/*               │
                    │  Service, VNet, NSG, NP, Ingress…   │
                    │  (on_apply → direct NetMux calls)     │
                    └──────────────────┬──────────────────┘
                                       │ imperative calls
                    ┌──────────────────▼──────────────────┐
                    │  NetMux (mod.rs)                     │
                    │  IPAM · attach_pod · apply_*_rules   │
                    └──────┬───────────────┬────────────────┘
                           │               │
              ┌────────────▼───┐   ┌───────▼────────┐
              │  netlink.rs    │   │  nftables.rs   │
              │  veth,routes   │   │  DNAT, NSG, NP │
              └────────────────┘   └────────────────┘
                           │
              ┌────────────▼────────┐
              │  dns · ingress · np │
              └─────────────────────┘
```

### Module inventory

| File | LOC | Responsibility |
|------|-----|----------------|
| `mod.rs` | 777 | `NetMux` facade, `NetworkState`/`NftRule` types, pod attach/detach, duplicate `apply_*` entry points |
| `nftables.rs` | 479 | Table init, ClusterIP/NodePort DNAT chains, NSG forward, sets for NetworkPolicy |
| `netlink.rs` | 530 | RTNETLINK: veth, addresses, routes, sysctl |
| `dns.rs` | 422 | Authoritative DNS, `cluster.local` |
| `pool.rs` | 189 | `IpPool` / `Ipv4Cidr` allocators |
| `np_controller.rs` | 230 | NetworkPolicy ↔ nft sets |
| `ingress.rs` | 155 | HTTP reverse proxy (L7) |
| `ipv6.rs` | 96 | Stub / partial |
| `network.rs` | 35 | `NetworkEngine`, `PodResolver` traits |
| `components/network/service.rs` | 387 | Backend resolution, calls `nft.add_dnat`, legacy proxy `HashSet` |

### What already works well

- **Per-node nft tables** — `z8s_nat_{node}`, `z8s_filter_{node}` avoid clobbering k3s/kube tables.
- **`NetNsGuard`** — RAII restore of host netns after `setns` in pod setup.
- **`spawn_blocking`** for `Batch::send()` — keeps tokio runtime responsive.
- **Pod attach pipeline** — allocate IP → veth → host route → configure pod netns (gateway, default route).
- **ClusterIP DNAT** — `add_dnat` + jump chains in prerouting/output (when `NetworkManager` drives it).
- **Declarative structs** — `NftRule`, `NftAction`, `VethSpec`, `RouteSpec`, `NetworkState` exist but are **not** the source of truth yet.

### Technical debt (why it feels “messy”)

| # | Issue | Impact |
|---|--------|--------|
| D1 | **Dual control planes** — `NetworkManager::sync_service` and `NetMux::apply_nsg` / `apply_vnet` called from many components | Drift, duplicate store walks, hard to reason about final nft state |
| D2 | **`jump_track` / `nodeport_jump_track`** in `NftEngine` | Actual rules ≠ declared state; leaks on partial failure |
| D3 | **`apply_rule` DNAT branch is a no-op** (debug log only) | Declarative path incomplete; all DNAT still imperative |
| D4 | **`sync_services_for_labels` from pod reconcile** | O(pods × services) per 2s tick ([05-scheduler.md](./05-scheduler.md)) |
| D5 | **`netlink.rs` monolith** | Hard to unit test; mixed sync I/O and policy |
| D6 | **NSG `reset_nsg_rules` + full replay** | Flash window; not diff-based |
| D7 | **RouteTable `apply_route_table_rules`** | Logs only; no kernel routes |
| D8 | **Service CIDR vs pod CIDR** | Service ClusterIP allocation not unified in planner |
| D9 | **Cross-node** | Remote pod IP unreachable without manual routes |

---

## 3. Target architecture

### 3.1 Layered model

```
┌─────────────────────────────────────────────────────────────────────────┐
│  L4  Control plane (outside netmux)                                      │
│      API store · scheduler · CRI attach_pod hook                          │
│      → emits store snapshot, never calls nft/netlink directly             │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ StoreSnapshot (read-only)
┌───────────────────────────────────▼─────────────────────────────────────┐
│  L3  Planner (pure, sync, no IO)                                         │
│      network/planner.rs — VNet, Subnet, NSG, EdgeService, NP, endpoints  │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ NetworkState (desired)
┌───────────────────────────────────▼─────────────────────────────────────┐
│  L2  Reconciler                                                          │
│      network/reconciler.rs — diff(current, desired) → ApplyPlan          │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │ ApplyPlan
┌───────────────────────────────────▼─────────────────────────────────────┐
│  L1  Appliers (IO, async, spawn_blocking where needed)                   │
│      applier/netlink.rs · applier/nft.rs · applier/dns.rs · applier/l7.rs │
└───────────────────────────────────┬─────────────────────────────────────┘
                                    │
┌───────────────────────────────────▼─────────────────────────────────────┐
│  L0  Linux kernel — netlink, nftables, network namespaces                 │
└───────────────────────────────────────────────────────────────────────────┘
```

**Public facade** — thin `NetMux` struct:

```rust
pub struct NetMux {
    runtime: NetMuxRuntime,  // holds current + desired, appliers, node_id
}

impl NetMux {
    pub async fn reconcile(&self, snap: &StoreSnapshot) -> Result<ReconcileReport>;
    pub fn attach_pod(&self, req: PodAttachRequest) -> Result<PodNetwork>;  // sync, CRI hot path
    pub fn detach_pod(&self, req: PodDetachRequest) -> Result<()>;
}
```

Components **do not** call `nft.add_dnat` or `apply_nsg` directly after migration.

### 3.2 Core types (`netmux/state.rs`)

All nftables intent lives in one structure — **no side vectors**.

```rust
/// Stable identity for idempotent nft upsert/delete.
#[derive(Clone, Hash, Eq, PartialEq)]
pub struct RuleKey {
    pub table: TableId,       // Nat | Filter
    pub chain: String,
    pub handle: String,       // logical name, e.g. "svc-0a000001-0050"
}

#[derive(Default)]
pub struct NetworkState {
    pub generation: u64,
    pub ipam: IpamState,
    pub links: IndexMap<PodId, VethSpec>,
    pub routes: Vec<RouteSpec>,
    pub nft: IndexMap<RuleKey, NftRule>,
    pub nft_sets: IndexMap<String, NftSetSpec>,   // policy IP sets
    pub dns: Vec<DnsRecord>,
    pub l7_routes: Vec<IngressRoute>,             // HTTP only
}

pub struct IpamState {
    pub pod_pool: PoolSnapshot,
    pub service_pool: PoolSnapshot,
    pub subnet_pools: HashMap<SubnetId, PoolSnapshot>,
    pub allocations: HashMap<PodId, Ipv4Addr>,
}
```

`NftRule` fields (already started in `mod.rs`) — enforce in planner, compile in applier:

| Field | Use |
|-------|-----|
| `name` / `chain` | `RuleKey` |
| `action` | `Accept` \| `Drop` \| `DNAT` \| `SNAT` \| `Masquerade` \| `Jump` |
| `source`, `dest` | CIDR or match |
| `protocol`, `dport`, `sport` | Match |
| `action::DNAT` | `{ dest_ip, dest_port }` — **must** be rendered in applier (fix D3) |

### 3.3 Planner (`netmux/planner.rs`)

Single function, deterministic, documented inputs:

```rust
pub struct NetworkPlanner {
    pub node_name: String,
    pub pod_cidr: Ipv4Cidr,
    pub service_cidr: Ipv4Cidr,
}

impl NetworkPlanner {
    /// Build desired state for THIS node only.
    pub fn plan(&self, snap: &StoreSnapshot) -> NetworkState;
}
```

**Planning rules (ordered pipeline):**

| Stage | Input | Output in `NetworkState` |
|-------|--------|---------------------------|
| `plan_default_vnet` | config + `VNet/default`, `Subnet/default` | baseline SNAT, forward policy |
| `plan_vnets_subnets` | VNet, Subnet CRDs | per-VNet pools, isolation |
| `plan_nsg` | NSG CRDs attached to subnet/vnet | ordered filter rules → `nft` |
| `plan_routes` | RouteTable CRDs | `routes` + static nft if needed |
| `plan_pods` | local running/assigned pods | `links`, IPAM allocations |
| `plan_edgeservices` | EdgeService (+ Service alias) | ClusterIP, NodePort DNAT rules |
| `plan_networkpolicy` | NetworkPolicy | `nft_sets` + filter rules |
| `plan_dns` | EdgeService, pods, endpoints | `dns` A/AAAA/CNAME |
| `plan_l7` | Ingress / Gateway exposure | `l7_routes` |

**EdgeService exposure** ([01-api.md](./01-api.md)):

| `spec.exposure.mode` | Planner output |
|----------------------|----------------|
| `ClusterIP` | DNAT: `service_ip:port` → backends `(pod_ip, target_port)*` |
| `NodePort` | DNAT: `:nodePort` → same backends |
| `LoadBalancer` | bind host VIP (from pool) + ClusterIP path |
| `Gateway` | L7 routes only; TLS termination optional later |

**Backend selection** (moved from `service.rs`):

```rust
fn resolve_backends(snap: &StoreSnapshot, selector: &Labels, ns: &str, port: u16, local_node: &str)
    -> Vec<(Ipv4Addr, u16)>;
```

- Only pods with `assigned_node == local_node` and phase Running.
- Round-robin via nft **or** multiple DNAT rules (document choice; prefer nft load balancing when available).

**Default VNet** — every pod without `z8s.io/vnet` uses `default`:

| Object | Purpose |
|--------|---------|
| `VNet/default` | `pod_cidr`, `internet_access: true` |
| `Subnet/default` | pod IP pool |
| `NSG/default` | allow egress, deny unsolicited ingress |
| `RouteTable/default` | SNAT masquerade for pod CIDR |

### 3.4 Diff & reconcile (`netmux/reconciler.rs`)

```rust
pub struct StateDiff {
    pub nft_upsert: Vec<NftRule>,
    pub nft_delete: Vec<RuleKey>,
    pub sets_upsert: Vec<(String, Vec<Ipv4Addr>)>,
    pub sets_delete: Vec<String>,
    pub link_ops: Vec<LinkOp>,      // CreateVeth | DeleteVeth | SetUp
    pub route_ops: Vec<RouteOp>,
    pub dns_replace: Option<Vec<DnsRecord>>,
}

impl NetMuxRuntime {
    pub fn diff(&self, desired: &NetworkState) -> StateDiff;
    pub async fn apply(&mut self, plan: StateDiff) -> Result<ReconcileReport>;
}
```

**Apply order** (invariants):

1. **IPAM** — reserve/release (in-memory; pod attach already holds locks)
2. **Netlink** — links before routes; routes before NAT that depends on them
3. **Nft sets** — before rules referencing sets
4. **Nft rules** — **delete** obsolete keys, then **upsert** new (batch per table)
5. **DNS** — atomic replace of record set
6. **L7** — reload ingress router config

On failure: return `ReconcileReport { partial, errors }`; keep `current` unchanged for failed slice; retry with backoff.

### 3.5 Nft applier (`netmux/applier/nft.rs`)

Responsibilities only:

- Own `NftEngine` with **no** `jump_track` — jumps derived from `RuleKey` set diff.
- `compile(rule: &NftRule) -> Vec<rustables::Rule>` — single place for DNAT/SNAT/match builders.
- `apply_batch(deletes, adds)` — one `spawn_blocking` per table when possible.
- Table lifecycle: `init()`, `cleanup()` — delete **only** owned tables.

**Chain layout** (documented constant):

```text
table z8s_nat_{node}
  chain prerouting  (hook) → jumps to svc-* / np-*
  chain output      (hook) → jumps to svc-*
  chain postrouting (hook) → masquerade for pod CIDR
  chain svc-{ip}-{port}     → DNAT backends
  chain np-{port}           → NodePort DNAT

table z8s_filter_{node}
  chain forward     (hook) → jump nsg-rules → established → policy
  chain nsg-rules           → NSG allow/deny
  chain input/output        → policy, optional
```

### 3.6 Netlink applier (`netmux/applier/netlink.rs`)

Split from today’s 530-line file:

| Submodule | Functions |
|-----------|-----------|
| `link.rs` | `VethBuilder`, create/delete, `set_link_up` |
| `addr.rs` | `assign_ip`, `assign_gateway` |
| `route.rs` | host `/32` pod routes, default route in pod ns |
| `ns.rs` | `NetNsGuard`, `enter_netns(pid)` |
| `sysctl.rs` | `ip_forward`, harden ([`harden_sysctl`](../../src/netmux/netlink.rs)) |

**Pod attach** (CRI hot path — stays synchronous, minimal):

```rust
pub struct PodAttachRequest {
    pub pod_uid: String,
    pub container_pid: u32,
    pub vnet: String,           // default
    pub subnet: Option<String>,
}

pub struct PodNetwork {
    pub pod_ip: Ipv4Addr,
    pub host_veth_ifindex: u32,
    pub peer_ifindex: u32,
}
```

`attach_pod` uses IPAM + netlink only — **no full reconcile**. After attach, mark `network_generation` dirty so async reconcile picks up policy/DNAT.

### 3.7 DNS (`netmux/applier/dns.rs`)

- Input: `NetworkState.dns` from planner.
- Single writer task; replace records atomically.
- Names: `<svc>.<ns>.svc.cluster.local`, headless → pod A records.
- Upstream forwarder for external names (existing behavior, cleaned).

### 3.8 L7 ingress (`netmux/applier/l7.rs`)

- Keep userspace HTTP proxy **only** for `Gateway` / Ingress paths.
- Config from `l7_routes`; reload without restart when diff changes listeners.
- Not used for ClusterIP TCP.

### 3.9 Network policy

- Planner emits `nft_sets` + filter rules (replace `np_controller` ad hoc calls).
- Label selector → pod IPs on **this node** only.
- Default deny within policy scope: implement in planner as explicit drop rules after allows.

---

## 4. Integration with the rest of z8s

### 4.1 Scheduler-driven reconcile (only caller)

NetMux is invoked **only** from the scheduler orchestrator ([05-scheduler.md](./05-scheduler.md)):

```rust
// scheduler/reconcile.rs — Task::SyncNetwork
let snap = self.engines.store.snapshot().await?;
self.engines.net.reconcile(&snap).await?;
```

Store changes (API, gossip) → `scheduler_notify` → coalesced `SyncNetwork` task. No network ticker inside netmux; no component `on_apply` → netmux.

**Delete** `NetworkManager`, `sync_services_for_labels`, and all `components/network/*` direct NetMux calls.

### 4.2 API / components

| Writer | Role |
|--------|------|
| API | Persist EdgeService / VNet / NSG / … to **store** only |
| Scheduler | Read snapshot → `NetMux::reconcile` |

Components for network kinds become **validation-only** or are removed ([05-scheduler.md](./05-scheduler.md)).

### 4.3 CRI

- `cri/runtime` calls `netmux.attach_pod` / `detach_pod` only.
- Inject `resolv.conf` from DNS applier endpoint (existing `config::set_dns_server`).

### 4.4 Multi-node (phase N)

| Mechanism | Behavior |
|-----------|----------|
| Pod IP on node | Local veth + host route (unchanged) |
| Remote pod IP | Gossip `PodStatus.podIP` + `nodeName` → planner adds `route_ops`: `pod_ip/32 via peer_gateway` |
| ClusterIP on every node | Same DNAT rules; backends filtered to local pods; remote traffic via routes |
| Overlay (later) | VXLAN interface in `link_ops` — planner stage `plan_overlay` |

No cloud-provider LB labels in this phase.

---

## 5. Safety & operations

### 5.1 Hard rules (from production incidents)

1. **Never** `nft flush ruleset` or delete tables other than `z8s_*_{node}`.
2. **Never** modify `KUBE-*`, `FLANNEL-*`, or other CNI chains on shared hosts.
3. `cleanup()` on shutdown: drop only owned tables; idempotent.
4. D-state / stuck netlink: applier timeouts; do not block PID1 shutdown ([node.rs](../../src/node.rs) watchdog).

### 5.2 Observability

```rust
pub struct ReconcileReport {
    pub generation: u64,
    pub duration_ms: u64,
    pub nft_rules: usize,
    pub routes: usize,
    pub warnings: Vec<String>,
}
```

- `tracing` spans: `netmux.reconcile`, `netmux.attach_pod`.
- Debug endpoint (future): `GET /debug/netmux/state` → JSON desired vs applied hash.

### 5.3 Configuration

| Flag / config | Default |
|---------------|---------|
| `pod_cidr` | `10.42.0.0/16` |
| `service_cidr` | `10.96.0.0/16` |
| `node_name` | hostname (table suffix) |
| `--net-reconcile-interval` | `1s` |
| `--net-dry-run` | log diff only |

---

## 6. Directory layout (target)

```
netmux/
├── mod.rs              # NetMux public API (~80 LOC)
├── state.rs            # NetworkState, RuleKey, IpamState (~140 LOC)
├── planner.rs          # pure plan(snap) (~200 LOC)
├── reconciler.rs       # diff + orchestration (~100 LOC)
├── pool.rs             # IpPool (keep, minor cleanup)
├── applier/
│   ├── mod.rs
│   ├── nft.rs          # compile + batch (~220 LOC)
│   ├── netlink.rs      # link, route, ns (~200 LOC)
│   ├── dns.rs          # (~180 LOC)
│   └── l7.rs           # ingress (~100 LOC)
└── network.rs          # traits for tests
```

Remove or fold: monolithic `nftables.rs`, `np_controller.rs`, fat `mod.rs` methods, `components/network/service.rs` logic.

**LOC target:** ~950 netmux + ~40 component glue ≈ **under 1,000** for core (L7 + ipv6 extra).

---

## 7. Implementation phases

| Phase | Deliverable | Success criteria |
|-------|-------------|------------------|
| **N0** | `state.rs` + `RuleKey`; freeze chain naming doc | Review signed off |
| **N1** | `planner` for pods + default VNet + SNAT; dry-run log diff | Golden tests pass |
| **N2** | `applier/nft` with DNAT compile; delete `jump_track` | ClusterIP curl works |
| **N3** | `applier/netlink` split; `attach_pod` unchanged behavior | Existing pod tests green |
| **N4** | EdgeService in planner; remove `sync_service` hot path | No userspace TCP proxy |
| **N5** | NSG + NetworkPolicy via planner diff | policy integration tests |
| **N6** | DNS from `NetworkState`; drop duplicate store walks | `nslookup` from pod |
| **N7** | RouteTable kernel routes | traceroute matches spec |
| **N8** | Multi-node routes via gossip | 2-node pod↔pod ping |
| **N9** | Delete dead code (`apply_rule` no-op path, old controllers) | `wc -l` ≤ target |

---

## 8. Testing strategy

| Layer | Method |
|-------|--------|
| Planner | Golden files: `tests/net/fixtures/*.yaml` → `NetworkState` JSON |
| Diff | Property: `diff(a,a) == empty`; `apply(diff(a,b))` then `current == b` (mock applier) |
| Nft compile | Snapshot rule text per `RuleKey` (optional `nft list ruleset` in VM test) |
| Integration | `tests/test-nginx-deploy.sh`, NodePort curl, NetworkPolicy deny |
| Bench | 200 EdgeServices, incremental reconcile &lt; 300ms |

---

## 9. Future (not planned now)

- **Cloud LB annotations** (`service.beta.kubernetes.io/azure-*`, `aws-*`) — defer until EdgeService `exposure.cloud` sub-spec is designed; no stub code in netmux until then.
- **eBPF / Cilium-style datapath** — only if nftables becomes bottleneck (profile first).
- **IPv6** — dual-stack planner stage after IPv4 stable.

---

*Previous: [03-storage.md](./03-storage.md) · Next: [05-scheduler.md](./05-scheduler.md)*
