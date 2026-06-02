# z8s v3 — Accurate Modernization Blueprint

> **Date:** 2026-06-02
> **Status:** Ground-truth-based modernization plan
> **Goal:** Reduce CLOC to ≤5,000, improve multi-node scheduling, add OverlayFS, IPv6, public IP assignment, and polish the existing architecture.

---

## 1. What Already Exists (Ground Truth)

After reading every source file, here's what's already implemented:

| Feature | Status | Location | Notes |
|---|---|---|---|
| VNet/Subnet/NSG/RouteTable CRDs | ✅ Implemented | `types.rs:2100-2220`, `components/network/` | VNetSpec has cidr, internet_access, role. SubnetSpec has vnet + cidr. NsgSpec has target_vnets + rules with action/srcCIDRs/dstCIDRs/priority. |
| Kernel DNAT (ClusterIP/NodePort) | ✅ Implemented | `netmux/nftables.rs:253-320` | `add_dnat()` and `add_nodeport_dnat()` with per-service nftables chains. |
| NetworkPolicy via nftables sets | ✅ Implemented | `netmux/np_controller.rs` | Dynamic nftables sets for pod/namespace selectors. NOT a TCP proxy. |
| Leader election | ✅ Implemented | `store/leases.rs` | Epoch+expiry lease in redb. `run_lease_loop`, `renew_lease`. |
| Scheduler (single-pass snapshot) | ✅ Implemented | `scheduler/scheduler.rs` | `node_load_snapshot()` does ONE pass over pods + nodes. NOT O(n²). Assigns least-loaded. |
| Anti-entropy gossip | ⚠️ Placeholder | `store/anti_entropy.rs` | Hash comparison loop exists but doesn't request missing keys. |
| Host process execution | ✅ Implemented | `cri/runtime.rs` (`is_native`) | Containers with `is_native: true` run without OCI isolation. |
| Ingress (L7 HTTP) | ✅ Implemented | `netmux/ingress.rs` | Host-based routing via TCP listener on port 80. |
| Service proxy (old TCP proxy) | ✅ Retired | `retired/network/service_proxy.rs` | Replaced by kernel DNAT. |
| OCI image pull | ✅ Implemented | `cri/image.rs` | Pulls from Docker Hub, unpacks layers, caches. |
| pivot_root / chroot / userns | ✅ Implemented | `cri/rootfs.rs` | Full RootfsIsolation enum: Pivot, Chroot, Degraded. |
| Landlock + capability drop | ✅ Implemented | `cri/rootfs.rs` | `apply_landlock()`, `drop_capabilities()`. |
| Reconciler pipeline | ✅ Implemented | `components/mod.rs` | `ComponentRegistry` + `ReconciliationPipeline` + `PipelineStage` trait. |
| Gossip (term-based dedup) | ✅ Implemented | `store/gossip.rs` | Per-resource broadcast with dedup. NOT batched. |
| MemoryBackend + RedbBackend | ✅ Implemented | `store/memory.rs`, `store/db.rs` | Full `StoreBackend` trait. |

---

## 2. What Actually Needs Work

### 2.1 Code Size: 19,451 → 5,000

The CLOC reduction is real. Here's what drives it:

| Subsystem | Current LOC | Target LOC | Technique |
|---|---|---|---|
| `types.rs` | 2,853 | ~500 | Generic `Resource<Spec, Status>` + drop unused `Option<Value>` fields |
| `api/handlers/*.rs` (23 files) | ~3,200 | ~300 | Generic `CrudHandler<R>` |
| `api/server.rs` (incl. helpers) | 240 | ~120 | Extract helpers, remove `chrono` usage |
| `cri/runtime.rs` | 1,415 | ~500 | Unify 3 spawn paths into strategy pattern |
| `cri/rootfs.rs` | ~600 | ~400 | Factor shared mount logic |
| `cri/image.rs` | ~300 | ~200 | OverlayFS replaces `copy_dir` |
| `netmux/mod.rs` | 586 | ~350 | Already clean, minor cleanup |
| `netmux/nftables.rs` | 479 | ~400 | Add numgen round-robin for LB |
| `scheduler/scheduler.rs` | 195 | ~150 | Minor cleanup |
| `components/*.rs` | ~1,050 | ~300 | Merge into pipeline |
| `store/gossip.rs` | 136 | ~100 | Batch protocol |
| `main.rs` | 739 | ~80 | Extract lock helpers |
| Everything else | ~4,600 | ~1,500 | Cleanup |
| **Total** | **~19,451** | **~5,000** | |

### 2.2 Types System (2,853 → ~500 lines)

The `AnyResource` enum with 20+ variants and `Option<serde_json::Value>` fields is the #1 source of code bloat. 

**Problem:** Every resource type has its own struct with many `Option<Value>` fields that are never read. Adding a field requires updating 15+ match arms.

**Fix:** Generic wrapper that preserves kubectl JSON compatibility:

```rust
#[derive(Serialize, Deserialize)]
pub struct Resource<S> {
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    pub kind: String,
    pub metadata: ObjectMeta,
    pub spec: Option<S>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<serde_json::Value>,
}
```

This keeps `kubectl get pods` working because JSON format is identical. The `AnyResource` enum shrinks from 20 variants to just a few, or can be replaced with `Box<dyn Any>`.

### 2.3 Generic CRUD Handler (23 files → ~300 lines)

Currently 23 handler files each implementing list/get/create/update/patch/delete identically.

**Fix:** One generic handler + router registration:

```rust
pub struct CrudHandler<S: Serialize + for<'de> Deserialize<'de>> {
    _phantom: PhantomData<S>,
}

impl<S: Serialize + for<'de> Deserialize<'de>> CrudHandler<S> {
    pub async fn list(state: State<AppState>) -> Json<Value> { ... }
    pub async fn get(state: State<AppState>, Path(name): Path<String>) -> Json<Value> { ... }
    pub async fn create(state: State<AppState>, body: Json<Value>) -> Result<Json<Value>> { ... }
    pub async fn update(state: State<AppState>, Path(name): Path<String>, body: Json<Value>) -> Json<Value> { ... }
    pub async fn patch(state: State<AppState>, Path(name): Path<String>, body: Json<Value>) -> Json<Value> { ... }
    pub async fn delete(state: State<AppState>, Path(name): Path<String>) -> Json<Value> { ... }
}
```

Router becomes:
```rust
.route("/api/v1/pods", get(CrudHandler::<PodSpec>::list).post(CrudHandler::<PodSpec>::create))
.route("/api/v1/pods/{name}", get(CrudHandler::<PodSpec>::get).delete(CrudHandler::<PodSpec>::delete))
// ... one line per resource
```

### 2.4 OverlayFS (Critical Performance)

**Current:** `cri/image.rs` calls `copy_dir()` — full recursive copy of image layers. 200MB nginx × 3 replicas = 600MB.

**Fix:** OverlayFS mount with shared lower layer:

```rust
// storage/overlay.rs
pub fn mount_overlay(lower: &Path, upper: &Path, work: &Path, merged: &Path) -> Result<()> {
    let opts = format!("lowerdir={},upperdir={},workdir={}", lower.display(), upper.display(), work.display());
    nix::mount::mount(Some("overlay"), merged, Some("overlay"), MsFlags::empty(), Some(opts.as_str()))?;
    Ok(())
}
```

Gains: Instant container startup. 200MB nginx × 10 replicas = 200MB instead of 2GB.

### 2.5 IPv6 + Public IP Assignment

The host provides a /64 prefix. Each pod gets a /128 from it.

```rust
pub struct Ipv6Pool { prefix: Ipv6Net, next: AtomicU64 }
impl Ipv6Pool {
    pub fn allocate(&self) -> Ipv6Addr {
        let iid = self.next.fetch_add(1, Ordering::Relaxed);
        let mut octets = self.prefix.network().octets();
        octets[8..16].copy_from_slice(&iid.to_be_bytes());
        Ipv6Addr::from(octets)
    }
}
```

Netlink setup: Add IPv6 address to veth via `RTM_NEWADDR`. Extend `netmux/netlink.rs`.

### 2.6 LoadBalancer with Public IP

The Service CRD already has `type: LoadBalancer`. Current implementation maps it to NodePort. Enhancement:

```yaml
apiVersion: v1
kind: Service
metadata:
  annotations:
    z8s.io/public-ip: "auto"  # allocate from IPv6 pool
spec:
  type: LoadBalancer
  ports:
    - port: 80
      targetPort: 8080
```

Implementation adds: allocate public IPv6 → add nftables DNAT for that IP → route to backends.

### 2.7 Gossip Batching

**Current:** `broadcast_write()` in `store/gossip.rs` serializes and sends per-peer individually:
```rust
for peer in &self.peers {
    peer.send(json.as_bytes().to_vec()).await;
}
```

**Fix:** Batch + flush on timer:

```rust
pub struct BatchedGossip {
    pending: Vec<GossipEntry>,
    flush_interval: Duration,
}

impl BatchedGossip {
    pub fn queue(&mut self, entry: GossipEntry) { self.pending.push(entry); }
    pub async fn flush(&mut self) {
        if self.pending.is_empty() { return; }
        let batch: Vec<GossipEntry> = self.pending.drain(..).collect();
        let frame = serde_json::to_vec(&batch).unwrap_or_default();
        for peer in &self.peers { peer.send(frame.clone()).await; }
    }
}
```

### 2.8 Scheduler Improvement

The current scheduler already does a single-pass snapshot. However, it rescans the full store every 3 seconds. Improvement:

```rust
pub struct Scheduler {
    node_loads: HashMap<String, u32>,  // maintained incrementally
}

impl Scheduler {
    pub fn update_load(&mut self, node: &str, delta: i32) {
        let load = self.node_loads.entry(node.to_string()).or_default();
        *load = (*load as i32 + delta).max(0) as u32;
    }
    
    pub fn least_loaded_node(&self) -> &str {
        self.node_loads.iter().min_by_key(|(_, c)| *c).map(|(n, _)| n.as_str()).unwrap_or("local")
    }
}
```

This avoids the full store scan. Current scan is O(pods + nodes), target is O(nodes).

### 2.9 Three Spawn Paths → Unified Pipeline

`cri/runtime.rs` has three paths:
- `spawn_container_from_config` (native)
- `spawn_root_ns_container` (root, PID namespace, pivot_root)
- `spawn_userns_container` (non-root, user namespace)

**Fix:** Strategy pattern:

```rust
pub enum IsolationStrategy { Full, UserNs, Degraded }

impl ProcessSupervisor {
    pub async fn spawn_container(&self, ctx: ContainerSpawnCtx, strategy: IsolationStrategy) -> Result<RunningContainer> {
        // Shared: pipe creation, env merge, log tasks, probes
        // Strategy-specific: fork + namespace setup
    }
}
```

This unifies 1,415 lines → ~500 lines.

### 2.10 Dependency Removal

| Crate | Replacement | Lines |
|---|---|---|
| `chrono` | `std::time` + 15-line RFC3339 | Remove ~200 usages |
| `uuid` | `getrandom` + 12-line v4 | Remove ~10 usages |
| `async-trait` | Rust 2024 RPITIT | Remove from all traits |
| `ipnetwork` | `config.rs:parse_cidr()` | Already exists |
| `futures-util` | Only `StreamExt` needed | Minimize |
| `tokio-tungstenite` | `axum` WS | Client only |

### 2.11 RBAC (Namespace-Scoped)

```rust
pub struct RbacManager {
    roles: HashMap<String, Role>,
    bindings: HashMap<String, RoleBinding>,
}

impl RbacManager {
    pub fn authorize(&self, user: &str, namespace: &str, resource: &str, verb: &str) -> bool {
        // Simple loop over bindings → roles → rules
    }
}
```

Standard kubectl YAML:
```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: pod-manager
  namespace: default
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch", "create", "update", "delete"]
```

### 2.12 Default VNet

All pods get assigned to a `default` VNet if no annotation is specified:

```rust
pub fn get_vnet_for_pod(&self, pod: &AnyResource) -> String {
    pod.metadata().annotations
        .get("z8s.io/vnet")
        .cloned()
        .unwrap_or_else(|| "default".to_string())
}
```

### 2.13 PID 1 Improvements

Already implemented in `init.rs`. Enhancements:
- Add `SIGCHLD` handling (currently only SIGTERM/SIGINT/SIGHUP)
- Add `waitpid(-1, WNOHANG)` loop for zombie reaping
- Multi-core: Tokio work-stealing handles this automatically

### 2.14 io_uring (Feature-Gated)

For image unpack (hot path):

```rust
#[cfg(feature = "io_uring")]
async fn unpack_layer_uring(data: &[u8], target: &Path) -> Result<()> {
    // Use tokio_uring for batched read/write
}

#[cfg(not(feature = "io_uring"))]
async fn unpack_layer(data: &[u8], target: &Path) -> Result<()> {
    // Existing tokio::fs path
}
```

---

## 3. Implementation Phases

### Phase 0: Generic Types + Handlers (Biggest Impact)
1. Create `resource/mod.rs` with `Resource<S>` generic
2. Create `api/crud.rs` with generic CRUD handler
3. Migrate one resource (ConfigMap) end-to-end
4. Validate: `kubectl get configmaps` works

### Phase 1: Type System Collapse
1. Migrate Pod, Service, Deployment to `Resource<S>`
2. Migrate Namespace, ConfigMap, Secret, Node
3. Migrate CRDs (VNet, Subnet, NSG, RouteTable)
4. Delete per-type API handlers → generic `CrudHandler`
5. Delete `AnyResource` enum

### Phase 2: Runtime Unification
1. Create unified spawner with strategy pattern
2. Delete `spawn_container_from_config` and `spawn_userns_container`
3. Refactor `spawn_root_ns_container` → `Spawner::spawn_full`
4. Add OverlayFS support

### Phase 3: Network Polish
1. Add IPv6 pool + netlink RTM_NEWADDR
2. Add LoadBalancer with public IP assignment
3. Add numgen round-robin for true LB in nftables
4. Complete anti-entropy gossip (request missing keys)
5. Batch gossip protocol

### Phase 4: Scheduler + Cluster
1. Add incremental load tracking (avoid full store scan)
2. Polish leader election with gossip confirmation
3. Add batch gossip for assignments

### Phase 5: Security + Polish
1. Add RBAC manager
2. Default VNet enforcement
3. SIGCHLD + zombie reaping in PID 1
4. Remove `chrono`, `uuid`, `async-trait`, `ipnetwork`
5. Collapse test setup into shared harness
6. CLOC audit — verify ≤ 5,000

---

## 4. What's NOT Needed

Based on actual code reading:

- **❌ Delete TCP proxy** — Already retired. `np_controller.rs` is NetworkPolicyController, not a proxy.
- **❌ Scheduler O(n²) fix** — Already single-pass snapshot.
- **❌ Leader election** — Already implemented with epoch+expiry.
- **❌ VNet/Subnet/NSG/RouteTable** — Already implemented.
- **❌ Host process support** — Already supported via `is_native`.
- **❌ Kernel DNAT for services** — Already implemented.
- **❌ NetworkPolicy via nftables** — Already implemented.
- **❌ Anti-entropy gossip** — Already has hash comparison loop (needs completion, not rewrite).

---

## 5. Success Metrics

| Metric | Current | Target | Method |
|---|---|---|---|
| CLOC | 19,451 | ≤ 5,000 | `cloc --include-lang=Rust src/` |
| Binary size | ~15MB | ≤ 8MB | `ls -la target/release/z8s` |
| Pod startup (cached) | ~2s | < 200ms | OverlayFS mount |
| Multi-node scheduling | Snapshot-based | Index-based O(nodes) | Incremental tracking |
| Gossip bandwidth | Per-resource | Batched 90% reduction | Timer-based flush |
| IPv6 support | None | Full dual-stack | Netlink RTM_NEWADDR |
| Test coverage | ~40% | ≥ 80% | `cargo tarpaulin` |

---

## 6. Key Insight

The existing codebase is more mature than assumed. The biggest wins are:
1. **Types system collapse** (2,853 → 500 lines)
2. **Generic CRUD handlers** (23 files → ~300 lines)
3. **OverlayFS** (instant container startup)
4. **IPv6 + public IP** (new feature)
5. **Gossip batching** (90% bandwidth reduction)

The scheduler, leader election, VNet/Subnet/NSG, and kernel DNAT are already well-implemented. Focus on what's missing, not rewriting what works.
