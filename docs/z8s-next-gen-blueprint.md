# z8s Next-Gen Blueprint: From Prototype to Production-Grade Cloud Platform

> **Date:** 2026-06-02  
> **Status:** Comprehensive modernization plan  
> **Goal:** Transform z8s into a mature, production-grade cloud infrastructure platform that exceeds k3s and s6 in performance, isolation, and features while reducing code complexity by 75%.

---

## Part 1: Executive Vision & Strategic Foundation

### 1.1 The z8s Opportunity

**Current Reality (v1):**
- 19,451 lines of Rust monolith code
- Prototype-quality with significant technical debt
- Multi-node scheduling is 5× slower than single-node (should be 2× faster)
- Networking is a mess of userspace proxies and incomplete kernel integration
- Storage uses full rootfs copies (200MB nginx × 3 replicas = 600MB wasted I/O)
- Partial isolation (chroot, optional PID namespace)
- No real cloud features (VNet, subnets, NSGs, load balancers, public IPs)
- Scheduler is O(pods²) instead of O(nodes)
- 23 API handler files with identical CRUD boilerplate
- Three redundant container spawn paths sharing 70% code

**Vision (v2):**
- ≤ 5,000 lines of clean, composable Rust code
- Production-grade reliability with zero crashes
- Multi-node scheduling ≥ 2× faster than single-node
- Kernel-native networking with VNet/Subnet/NSG/LoadBalancer/Public IPv6
- OverlayFS for instant container startup (mount is O(1))
- Full isolation: pivot_root + PID + user + IPC + net + cgroup namespaces
- io_uring for 2-4× throughput on image unpack and log streaming
- O(1) index-based scheduling with batched gossip
- PID 1 capable with multi-core support and capability-aware process supervision
- Complete cloud infrastructure supporting Azure/AWS-style workloads

### 1.2 Core Architectural Principles

**P1: Type-Driven Design**
Every resource, operation, and state transition is encoded in the type system. If it compiles, it's correct. Runtime errors are bugs.

**P2: Zero-Copy Where Possible**
Borrow over clone, `&str` over `String`, slices over `Vec`. Every allocation is a potential performance bottleneck.

**P3: Pipeline Everything**
Every resource lifecycle follows: Parse → Validate → Store → Reconcile → Actuate. One pipeline, many resource types. No special cases.

**P4: Kernel is the Fast Path**
nftables DNAT replaces userspace TCP proxy. OverlayFS replaces `cp -r`. io_uring replaces epoll for file I/O. Userspace is for control plane only.

**P5: Decouple by Interface**
Every subsystem communicates through traits. Implementations can be swapped without touching callers. Dependency injection is mandatory.

**P6: Fail Fast, Recover Gracefully**
Invalid state is detected at compile time or at system boundaries. Runtime errors trigger recovery, not crashes. Every error path is tested.

**P7: Measure Everything**
If you can't measure it, you can't improve it. Every critical path has metrics. Every metric has a target. Every target has a test.

### 1.3 Success Metrics

| Metric | Current | Target | Measurement Method |
|--------|---------|--------|-------------------|
| **Code Size (CLOC)** | 19,451 | ≤ 5,000 | `cloc --include-lang=Rust src/` |
| **Binary Size** | ~15MB | ≤ 8MB (static, stripped) | `ls -la target/release/z8s` |
| **Pod Startup (cached)** | ~2s | < 200ms | Timer from apply → Running |
| **Multi-node Scheduling** | 5× slower | ≥ 2× faster | Pods/sec across N nodes |
| **Scheduling Latency** | ~500ms | < 50ms per pod | Timer from Pending → Assigned |
| **Gossip Convergence** | ~2s | < 500ms (3 nodes) | Timer from write → all nodes agree |
| **NodePort Latency** | ~200μs | < 10μs | Direct pod IP vs NodePort comparison |
| **Image Unpack (200MB)** | ~2s | ~500ms | io_uring vs epoll benchmark |
| **Test Coverage** | ~40% | ≥ 80% | `cargo tarpaulin` |
| **Zero `unwrap()` in prod** | 47 | 0 | `grep -r 'unwrap()' src/ --include='*.rs' \| grep -v test` |
| **Zero `unsafe` without SAFETY** | 12 | 0 | Manual audit |

### 1.4 Guiding Constraints

**C1: No New Dependencies Without Justification**
Every new crate must solve a problem that can't be solved with existing deps or stdlib. DashMap → use `RwLock<HashMap>` instead. MessagePack → stick with JSON for simplicity.

**C2: No Breaking Changes to kubectl Compatibility**
z8s must remain 100% compatible with standard kubectl commands. API surfaces are frozen.

**C3: No Feature Without Isolation**
Every new feature must work in isolated environments (user namespaces, chroot, netns). No feature requires root unless absolutely necessary.

**C4: No Synchronous I/O on Hot Paths**
All I/O on critical paths (image pull, container spawn, gossip) must be async. Blocking calls are bugs.

**C5: No Global State Without Atomic Operations**
Global state is accessed via atomics or locked structures. No `static mut`. No data races.

**C6: No Comments Without "Why"**
Comments explain "why", not "what". If the code isn't self-documenting, refactor it.

**C7: No Test Without Edge Cases**
Every test must cover at least one edge case. Happy path tests are necessary but insufficient.

---

## Part 2: Deep Codebase Audit

### 2.1 Code Size Inventory (Per-File Analysis)

After reading every file in `src/`, here is the precise breakdown:

| File/Module | Lines | % of Total | Category | Severity |
|---|---|---|---|---|
| `types.rs` | 2,545 | 13.1% | Type definitions | CRITICAL — god file |
| `api/server.rs` | 947 | 4.9% | HTTP server | HIGH — test bloat |
| `api/handlers/*.rs` (23 files) | ~3,200 | 16.5% | CRUD handlers | CRITICAL — boilerplate |
| `cri/runtime.rs` | 1,259 | 6.5% | Container spawn | CRITICAL — 3 paths |
| `cri/rootfs.rs` | 855 | 4.4% | Filesystem isolation | MEDIUM — complex but needed |
| `cri/exec.rs` | 840 | 4.3% | kubectl exec | MEDIUM — protocol complexity |
| `cri/image.rs` | ~600 | 3.1% | Image pull/cache | HIGH — full copy |
| `cri/cgroup.rs` | ~300 | 1.5% | Resource limits | LOW — clean |
| `netmux/mod.rs` | 465 | 2.4% | NetMux orchestrator | HIGH — mixed concerns |
| `netmux/nftables.rs` | 429 | 2.2% | nftables engine | LOW — well-structured |
| `netmux/netlink.rs` | 424 | 2.2% | Raw netlink | MEDIUM — fragile bytes |
| `netmux/dns.rs` | 362 | 1.9% | DNS resolver | LOW — works |
| `netmux/np_controller.rs` | 200 | 1.0% | TCP proxy | CRITICAL — DELETE |
| `netmux/pool.rs` | 154 | 0.8% | IP pool | LOW — clean |
| `netmux/ingress.rs` | 145 | 0.7% | Ingress L7 | LOW — thin |
| `store/*.rs` (9 files) | ~1,300 | 6.7% | Store + gossip | MEDIUM — dedup weak |
| `scheduler/*.rs` | ~500 | 2.6% | Scheduler | CRITICAL — O(n²) |
| `components/*.rs` (15 files) | ~1,400 | 7.2% | Reconcilers | HIGH — repetitive |
| `manifest/*.rs` | ~300 | 1.5% | YAML watcher | LOW — works |
| `storage/*.rs` | ~400 | 2.1% | PV/PVC/loop | MEDIUM — needs overlay |
| `main.rs` | 739 | 3.8% | CLI + daemon mgmt | HIGH — lock mgmt bloat |
| `node.rs` | 475 | 2.4% | Node lifecycle | HIGH — tightly coupled |
| `config.rs` | 327 | 1.7% | Config parsing | LOW — clean |
| `init.rs` | 59 | 0.3% | PID 1 signal handler | LOW — clean |
| **Total** | **~19,451** | **100%** | | |

### 2.2 Critical Problems — Detailed Analysis

#### P1: `types.rs` — The 2,545-Line God File

**Problem:** Hand-rolls every Kubernetes type. Most structs carry 20+ `Option<serde_json::Value>` fields that are never read. The `AnyResource` enum has 20+ variants with repetitive match arms across 15+ call sites.

**Evidence:**
```rust
// types.rs:217-250 — PodSpec has 14 Option<Value> fields that are never accessed
pub struct PodSpec {
    pub containers: Option<Vec<Container>>,
    pub init_containers: Option<Vec<Container>>,
    pub volumes: Option<Vec<Volume>>,
    // ... 20 more fields, most Option<Value>
}

// types.rs:1800-2200 — AnyResource enum with 20 variants
pub enum AnyResource {
    Pod(Pod), Service(Service), Deployment(Deployment),
    Namespace(Namespace), ConfigMap(ConfigMap), Secret(Secret),
    Node(Node), Ingress(Ingress), NetworkPolicy(NetworkPolicy),
    VNet(VNet), Subnet(Subnet), Nsg(Nsg), RouteTable(RouteTable),
    Pv(Pv), Pvc(Pvc), // ... 5 more
}
```

**Impact:** Every time a new resource type is added, 15+ match arms must be updated. Adding a field to one type means copy-pasting boilerplate. This is the single biggest source of code duplication.

**Fix:** Generic `Resource<Spec, Status>` wrapper with `ResourceSpec` trait. Reduces from 2,545 to ~400 lines.

#### P2: Three Redundant Container Spawn Paths

**Problem:** `cri/runtime.rs` has three functions sharing ~70% identical code:

| Function | Lines | When Used |
|---|---|---|
| `spawn_container_from_config` | ~200 | Non-root, user ns, chroot |
| `spawn_root_ns_container` | ~350 | Root, double-fork, PID ns, pivot_root |
| `spawn_userns_container` | ~200 | Non-root, user ns (newer path) |

**Shared code (duplicated 3×):**
- Pipe pair creation (`nix::unistd::pipe()`)
- Environment variable merging (OCI config + pod spec + ConfigMap)
- Log buffer task spawning
- Health probe scheduling
- cgroup assignment
- `RunningContainer` registration

**Impact:** Bug fixes must be applied 3×. New features (e.g., new probe type) require 3 changes. 750 lines of duplicated code.

**Fix:** Single `Spawner` with `IsolationStrategy` enum. One 200-line function replaces three 750-line functions.

#### P3: Multi-Node Scheduling is O(pods²)

**Problem:** In `scheduler/scheduler.rs`:
```rust
pub async fn scheduler_tick(store: &dyn StoreBackend, node_name: &str) {
    let pods = store.get_by_kind("Pod").await;         // O(all pods)
    for tracker in pods {
        let snapshot = node_load_snapshot(store).await; // O(all pods) AGAIN
        // Assign to least-loaded node
    }
}
```

**Analysis:** For N unscheduled pods across M nodes, each tick does:
- 1 full table scan: O(total pods)
- N snapshot computations: O(N × total pods)
- Total: O(N × total pods) per tick

With 10 pods on 2 nodes, this is O(20) per tick. With 100 pods, it's O(10,000). The 5× slowdown for 2 nodes comes from the gossip overhead multiplying the O(N²) work.

**Fix:** In-memory `DashMap<NodeName, NodeLoad>` maintained incrementally. O(nodes) per scheduling decision.

#### P4: Userspace TCP Proxy Instead of Kernel DNAT

**Problem:** `netmux/np_controller.rs` implements a full TCP proxy:
```rust
// Each Service connection spawns a tokio task
let (mut client_read, mut client_write) = tcp_stream.split();
let (mut backend_read, mut backend_write) = backend_stream.split();
let _ = tokio::io::copy_bidirectional(
    &mut client_read, &mut backend_write,
    &mut backend_read, &mut client_write,
).await;
```

**Impact:** Each connection consumes a tokio task + 2 socket buffers. Under load: 2× memory, ~200μs latency per request.

**But:** The nftables DNAT rules already exist in `nftables.rs:302-323`. The proxy is redundant overhead.

**Fix:** Delete `np_controller.rs`. Use pure kernel DNAT. 200 lines removed, 200μs latency eliminated.

#### P5: Full Rootfs Copy Per Container

**Problem:** In `cri/image.rs:275-291`:
```rust
fn copy_cache_to_container(cache_path: &str, container_rootfs: &str) -> Result<String> {
    let _ = std::fs::remove_dir_all(container_rootfs);
    Self::copy_dir(Path::new(cache_path), Path::new(container_rootfs))?;  // FULL COPY
    Ok(container_rootfs.to_string())
}
```

**Impact:** 200MB nginx × 3 replicas = 600MB redundant I/O + 6 seconds startup. With 10 replicas: 20 seconds + 2GB wasted disk.

**Fix:** OverlayFS mount is O(1). Shared read-only lower layer. Instant startup regardless of replica count.

#### P6: 23 API Handler Files with Identical CRUD

**Problem:** Every resource type has its own handler file implementing list/get/create/update/patch/delete identically:
```rust
// api/handlers/pod.rs, service.rs, deployment.rs, ... (×23)
pub async fn list_pods(state: State<AppState>, ...) -> Json<Value> { ... }
pub async fn get_pod(state: State<AppState>, ...) -> Json<Value> { ... }
pub async fn create_pod(state: State<AppState>, ...) -> Json<Value> { ... }
// ... same pattern 23 times
```

**Fix:** Generic `CrudHandler<R: ResourceSpec>` with router registration:
```rust
.route("/api/v1/pods", get(CrudHandler::<PodSpec>::list).post(CrudHandler::<PodSpec>::create))
```

23 files → 1 file. ~3,200 lines → ~300 lines.

#### P7: Gossip Serialization Overhead

**Problem:** In `store/gossip.rs:74-96`:
```rust
pub async fn broadcast_write(&self, resource: &AnyResource) {
    let json = serde_json::to_vec(resource)?;     // ALLOC #1
    let msg = serde_json::to_string(&msg)?;        // ALLOC #2
    let bytes = msg.as_bytes().to_vec();            // ALLOC #3
    for peer in &self.peers {
        peer.send(bytes.clone()).await;             // N sends
    }
}
```

3 allocations + N sends per resource write. For a deployment scale-up of 10 pods: 30 allocations + 10N sends.

**Fix:** Batch + single serialization. O(1) sends per tick instead of O(N). 90% bandwidth reduction.

#### P8: `main.rs` Lock Management Bloat

**Problem:** 200 lines of lock management code (acquire, read, scan, stale detection, D-state handling) that should be 30 lines with a helper abstraction.

**Fix:** Extract `LockManager` struct with clean API. `main.rs` goes from 739 to ~80 lines.

### 2.3 Dependency Audit

| Crate | Lines Used | Verdict | Action |
|---|---|---|---|
| `tokio` | Core runtime | Keep | — |
| `axum` | HTTP server | Keep | — |
| `serde`, `serde_json` | Serialization | Keep | — |
| `nix` | System calls | Keep | — |
| `tracing` | Logging | Keep | — |
| `anyhow` | Error handling | Keep | — |
| `redb` | Persistence | Keep | — |
| `rustables` | nftables | Keep | — |
| `caps` | Capabilities | Keep | — |
| `landlock` | LSM | Keep | — |
| `oci-distribution` | Image pull | Keep | — |
| `flate2`, `tar`, `base64` | Image unpack | Keep | — |
| `chrono` | Timestamps | **Remove** | Replace with `std::time` + 15-line RFC3339 formatter |
| `uuid` | ID generation | **Remove** | Replace with `getrandom` + 12-line v4 format |
| `notify` | File watching | **Remove** | Use `nix` inotify directly (already a dep) |
| `async-trait` | Async traits | **Remove** | Rust 2024 edition has native RPITIT |
| `ipnetwork` | CIDR parsing | **Remove** | Already have `parse_cidr` in config.rs |
| `tokio-tungstenite` | WS client | **Remove** | Use `axum` built-in WS |
| `futures-util` | Stream utils | **Minimize** | Only need `StreamExt` |

**Net removal:** 6 crates → ~200KB binary reduction, fewer transitive deps.

### 2.4 Architecture Dependency Graph (Current — Has Cycles)

```
main.rs ──→ node.rs ──→ api/ ──→ components/ ──→ cri/
                │            │          │            │
                │            │          │            ↓
                │            │          └──── store/ ←──┘
                │            │                    │
                ├──→ netmux/ ←──────────────────┘
                │            │
                ├──→ scheduler/ ──→ store/
                │
                └──→ storage/ ──→ store/
```

**Problems:**
- `components/` imports `cri/`, `netmux/`, `store/` — too many dependencies
- `netmux/` is imported by both `components/` and `cri/` — circular risk
- `store/` is imported by everything — it's the leaf but treated as a utility
- `node.rs` is a 475-line orchestrator with no DI — every dependency is hardcoded

### 2.5 Architecture Dependency Graph (Target — Clean DAG)

```
api/ ──→ resource/ ←── reconcile/
              ↑              │
              │         ┌────┼────┐
              │         ↓    ↓    ↓
           cluster/  compute/ network/ storage/
                        │       │
                        ↓       ↓
                    Linux Kernel (fork, netlink, nftables, overlayfs)
```

**Key rules:**
- `resource/` is the leaf — no imports from other z8s modules
- `api/` never imports `compute/` directly — goes through `reconcile/`
- `cluster/` never imports `network/` or `compute/` — only `resource/`
- No circular dependencies. Period.

---

## Part 3: Target Architecture — Detailed Design

### 3.1 Module Hierarchy (v2)

```
src/
├── main.rs                  CLI dispatch + daemon management (~80 lines)
├── node.rs                  Node lifecycle + DI container (~100 lines)
├── config.rs                CLI parsing + global config (~120 lines)
├── init.rs                  PID 1 signal handling + multi-core (~60 lines)
│
├── resource/                ── Generic resource framework ──
│   ├── mod.rs               Resource<S,T> wrapper, AnyResource enum (~80 lines)
│   ├── meta.rs              ObjectMeta, ListMeta, Time, Quantity (~100 lines)
│   ├── registry.rs          Type registry: kind → (de)serializer (~60 lines)
│   └── store.rs             StoreBackend trait + Memory + Redb backends (~200 lines)
│
├── api/                     ── HTTP layer ──
│   ├── server.rs            Axum router + middleware (~60 lines)
│   ├── crud.rs              Generic CRUD handler (all 6 ops from 1 trait) (~150 lines)
│   ├── watch.rs             Watch stream (SSE-based) (~80 lines)
│   ├── exec.rs              kubectl exec WebSocket (~400 lines)
│   ├── proto.rs             Protobuf decoder (~100 lines)
│   ├── auth.rs              RBAC middleware (~120 lines)
│   └── discovery.rs         /api, /apis, /version, /healthz (~50 lines)
│
├── compute/                 ── Container runtime ──
│   ├── spawner.rs           Unified container spawn pipeline (~250 lines)
│   ├── rootfs.rs            Filesystem isolation (pivot_root/chroot) (~350 lines)
│   ├── image.rs             OCI pull + OverlayFS mount (~300 lines)
│   ├── cgroup.rs            cgroups v2 resource limits (~150 lines)
│   ├── health.rs            Probe runner (~120 lines)
│   ├── lifecycle.rs         Restart policy + process tracking (~180 lines)
│   └── caps.rs              Capability management (~80 lines)
│
├── network/                 ── Network plane ──
│   ├── netlink.rs           Raw netlink socket operations (~250 lines)
│   ├── nft.rs               nftables engine (DNAT, SNAT, NSG) (~300 lines)
│   ├── veth.rs              veth pair lifecycle (~150 lines)
│   ├── vnet.rs              VNet/Subnet IPAM (~200 lines)
│   ├── dns.rs               In-cluster DNS server (~250 lines)
│   ├── ipv6.rs              IPv6 public IP assignment from host /64 (~120 lines)
│   ├── policy.rs            NetworkPolicy + NSG enforcement (~150 lines)
│   ├── lb.rs                LoadBalancer implementation (~180 lines)
│   └── pool.rs              IP address pool (CIDR allocation) (~100 lines)
│
├── storage/                 ── Persistent storage ──
│   ├── overlay.rs           OverlayFS mount/unmount (~120 lines)
│   ├── provision.rs         PV/PVC binding + loop provisioner (~150 lines)
│   └── volumes.rs           Volume mount resolution (~100 lines)
│
├── cluster/                 ── Multi-node coordination ──
│   ├── gossip.rs            Batched gossip protocol (~120 lines)
│   ├── scheduler.rs         O(1) index-based pod scheduling (~150 lines)
│   ├── leader.rs            Lease-based leader election (~100 lines)
│   └── sync.rs              Anti-entropy reconciliation (~80 lines)
│
└── reconcile/               ── Control plane ──
    ├── mod.rs               Reconciler loop + notify-driven wake (~100 lines)
    ├── pipeline.rs          Resource lifecycle pipeline (~120 lines)
    ├── deployment.rs        Deployment → Pod reconciliation (~150 lines)
    └── service.rs           Service → nftables DNAT reconciliation (~120 lines)
```

**Total:** ~4,800 lines (vs. current 19,451)

### 3.2 Generic Resource Framework — The Foundation

The single biggest code reduction comes from replacing 2,545 lines of hand-rolled K8s types with a generic framework.

**Core Types:**

```rust
// resource/mod.rs — 80 lines
#[derive(Serialize, Deserialize)]
pub struct Resource<S, T = ()> {
    pub api_version: &'static str,
    pub kind: &'static str,
    pub metadata: ObjectMeta,
    pub spec: Option<S>,
    pub status: Option<T>,
}

pub trait ResourceSpec: Serialize + DeserializeOwned + Clone + Send + Sync + 'static {
    const API_VERSION: &'static str;
    const KIND: &'static str;
    const PLURAL: &'static str;
    const NAMESPACED: bool;
    type Status: Serialize + DeserializeOwned + Default + Clone;
}

// Type-erased wrapper for heterogeneous collections
#[derive(Clone)]
pub enum AnyResource {
    Pod(Resource<PodSpec, PodStatus>),
    Service(Resource<ServiceSpec, ServiceStatus>),
    // ... 15 more
}
```

**Resource Implementations — 8 lines each instead of 60:**

```rust
// Example: Pod (8 lines instead of 60)
impl ResourceSpec for PodSpec {
    const API_VERSION: &'static str = "v1";
    const KIND: &'static str = "Pod";
    const PLURAL: &'static str = "pods";
    const NAMESPACED: bool = true;
    type Status = PodStatus;
}

// Example: Service (8 lines instead of 60)
impl ResourceSpec for ServiceSpec {
    const API_VERSION: &'static str = "v1";
    const KIND: &'static str = "Service";
    const PLURAL: &'static str = "services";
    const NAMESPACED: bool = true;
    type Status = ServiceStatus;
}
```

**Code Reduction:** 2,545 → 400 lines (84% reduction)

### 3.3 Generic CRUD Handler — Eliminating Boilerplate

Replace 23 handler files with one generic implementation.

**Generic Handler:**

```rust
// api/crud.rs — 150 lines
pub struct CrudHandler<R: ResourceSpec> {
    _phantom: PhantomData<R>,
}

impl<R: ResourceSpec> CrudHandler<R> {
    pub async fn list(state: State<AppState>, params: ListParams) -> Json<Value> {
        let resources = state.store.get_by_kind(R::KIND).await;
        let filtered = apply_list_params(resources, params);
        Json(build_list_response::<R>(filtered))
    }

    pub async fn get(state: State<AppState>, Path(name): Path<String>) -> Json<Value> {
        let resource = state.store.get(R::KIND, &name).await;
        Json(build_get_response::<R>(resource))
    }

    pub async fn create(state: State<AppState>, body: Json<Value>) -> Result<Json<Value>> {
        let resource: Resource<R> = serde_json::from_value(body.0)?;
        validate_resource(&resource)?;
        state.store.apply(resource.clone()).await?;
        Ok(Json(build_create_response(resource)))
    }

    pub async fn update(
        state: State<AppState>,
        Path(name): Path<String>,
        body: Json<Value>,
    ) -> Result<Json<Value>> {
        let mut resource: Resource<R> = serde_json::from_value(body.0)?;
        resource.metadata.name = Some(name);
        validate_resource(&resource)?;
        state.store.apply(resource.clone()).await?;
        Ok(Json(build_update_response(resource)))
    }

    pub async fn patch(
        state: State<AppState>,
        Path(name): Path<String>,
        body: Json<Value>,
    ) -> Result<Json<Value>> {
        let existing = state.store.get(R::KIND, &name).await
            .ok_or_else(|| anyhow!("not found"))?;
        let patched = merge_patch(existing, body.0)?;
        state.store.apply(patched.clone()).await?;
        Ok(Json(build_patch_response(patched)))
    }

    pub async fn delete(state: State<AppState>, Path(name): Path<String>) -> Json<Value> {
        state.store.delete(R::KIND, &name).await;
        Json(build_delete_response())
    }
}
```

**Router Registration — 1 line per resource:**

```rust
// api/server.rs — 60 lines
let router = Router::new()
    // Core resources
    .route("/api/v1/pods", get(CrudHandler::<PodSpec>::list).post(CrudHandler::<PodSpec>::create))
    .route("/api/v1/pods/{name}", get(CrudHandler::<PodSpec>::get)
        .put(CrudHandler::<PodSpec>::update)
        .patch(CrudHandler::<PodSpec>::patch)
        .delete(CrudHandler::<PodSpec>::delete))
    
    .route("/api/v1/services", get(CrudHandler::<ServiceSpec>::list).post(CrudHandler::<ServiceSpec>::create))
    .route("/api/v1/services/{name}", get(CrudHandler::<ServiceSpec>::get).delete(CrudHandler::<ServiceSpec>::delete))
    
    .route("/apis/apps/v1/deployments", get(CrudHandler::<DeploymentSpec>::list).post(CrudHandler::<DeploymentSpec>::create))
    // ... 15 more resources, 1 line each
    
    // Custom resources (VNet, Subnet, NSG)
    .route("/apis/z8s.io/v1/vnets", get(CrudHandler::<VNetSpec>::list).post(CrudHandler::<VNetSpec>::create))
    .route("/apis/z8s.io/v1/subnets", get(CrudHandler::<SubnetSpec>::list).post(CrudHandler::<SubnetSpec>::create))
    .route("/apis/z8s.io/v1/nsgs", get(CrudHandler::<NsgSpec>::list).post(CrudHandler::<NsgSpec>::create));
```

**Code Reduction:** 3,200 → 300 lines (91% reduction)

### 3.4 Unified Container Spawner — One Pipeline

Replace three redundant spawn paths with a single unified pipeline.

**Isolation Strategy:**

```rust
// compute/spawner.rs — 250 lines
pub enum IsolationStrategy {
    /// Root mode: double-fork with PID namespace, pivot_root
    Full { pid_ns: bool },
    /// Non-root: user namespace + chroot
    UserNs,
    /// Degraded: host mounts, no isolation (development only)
    Degraded,
}

pub struct SpawnRequest {
    pub container_id: String,
    pub image_ref: String,
    pub entrypoint: Vec<String>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub volumes: Vec<ResolvedVolume>,
    pub resources: Option<ResourceRequirements>,
    pub probes: ProbeConfig,
    pub ports: Vec<ContainerPort>,
    pub isolate_net: bool,
    pub hostname: String,
    pub run_as_user: Option<u32>,
    pub capabilities: Option<Capabilities>,
}

pub struct Spawner {
    image_mgr: Arc<ImageManager>,
    cgroup_mgr: Arc<CgroupManager>,
    netmux: Arc<NetMux>,
}

impl Spawner {
    /// Single entry point for all container launches.
    pub async fn spawn(&self, req: SpawnRequest) -> Result<RunningContainer> {
        // 1. Resolve image → rootfs (OverlayFS or copy fallback)
        let rootfs = self.image_mgr.prepare_rootfs(&req.image_ref, &req.container_id).await?;

        // 2. Determine isolation strategy
        let strategy = self.detect_strategy();

        // 3. Create sync pipes (shared across all strategies)
        let (sync_r, sync_w) = pipe()?;
        let (ack_r, ack_w) = pipe()?;

        // 4. Fork + isolate (strategy-specific)
        let child_pid = match strategy {
            IsolationStrategy::Full { pid_ns } => {
                self.fork_full(&req, &rootfs, pid_ns, sync_w, ack_r)?
            }
            IsolationStrategy::UserNs => {
                self.fork_userns(&req, &rootfs, sync_w, ack_r)?
            }
            IsolationStrategy::Degraded => {
                self.fork_degraded(&req, &rootfs)?
            }
        };

        // 5. Parent: wait for sync, write userns maps, send ack (shared)
        wait_for_sync(&sync_r)?;
        if matches!(strategy, IsolationStrategy::UserNs) {
            write_userns_maps(child_pid, req.run_as_user, None)?;
        }
        send_ack(&ack_w)?;

        // 6. Network setup (shared)
        if req.isolate_net {
            self.netmux.configure_pod_netns(child_pid, &req.container_id).await?;
        }

        // 7. cgroup assignment (shared)
        self.cgroup_mgr.assign(child_pid, &req.container_id, &req.resources)?;

        // 8. Apply capabilities (shared)
        if let Some(ref caps) = req.capabilities {
            apply_capabilities(child_pid, caps)?;
        }

        // 9. Spawn log + probe tasks (shared)
        let log_buffer = spawn_log_task(child_pid);
        spawn_probes(&req.probes, child_pid);

        // 10. Register
        Ok(RunningContainer {
            pid: Some(child_pid),
            log_buffer,
            rootfs,
            strategy,
        })
    }

    fn detect_strategy(&self) -> IsolationStrategy {
        if crate::cri::rootfs::is_root() {
            IsolationStrategy::Full { pid_ns: true }
        } else if crate::cri::rootfs::userns_available() {
            IsolationStrategy::UserNs
        } else {
            warn!("No isolation available — running in degraded mode");
            IsolationStrategy::Degraded
        }
    }
}
```

**Code Reduction:** 1,259 → 250 lines (80% reduction)

### 3.5 Networking Modernization — VNet/Subnet/NSG/LoadBalancer

#### 3.5.1 VNet Model

```yaml
# VNet: a /16-/20 overlay network
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: production
spec:
  cidr: "10.200.0.0/16"
  subnets:
    - name: web-tier
      cidr: "10.200.0.0/24"
      nsg: web-nsg
    - name: db-tier
      cidr: "10.200.1.0/24"
      nsg: db-nsg
```

**Implementation:**

```rust
// network/vnet.rs — 200 lines
pub struct VNetManager {
    vnets: DashMap<String, VNetState>,
    pools: DashMap<String, IpPool>,
    nft: Arc<NftEngine>,
}

struct VNetState {
    cidr: Ipv4Net,
    subnets: Vec<Subnet>,
    nft_rules: Vec<NftRule>,
}

impl VNetManager {
    pub fn apply_vnet(&self, vnet: &VNet) -> Result<()> {
        let state = VNetState {
            cidr: vnet.spec.cidr.parse()?,
            subnets: vnet.spec.subnets.clone(),
            nft_rules: Vec::new(),
        };

        // Create subnet pools
        for subnet in &state.subnets {
            let pool = IpPool::new(subnet.cidr.parse()?);
            self.pools.insert(subnet.name.clone(), pool);
        }

        // Generate nftables rules for VNet isolation
        let rules = self.generate_vnet_rules(&state);
        self.nft.apply_rules(&rules)?;

        self.vnets.insert(vnet.metadata.name.clone(), state);
        Ok(())
    }

    fn generate_vnet_rules(&self, state: &VNetState) -> Vec<NftRule> {
        let mut rules = Vec::new();

        // SNAT for outbound traffic from VNet
        rules.push(NftRule {
            chain: "postrouting",
            action: Action::SNAT,
            source: Some(state.cidr.into()),
            dest: None,
            protocol: Protocol::Any,
            ports: None,
        });

        // Inter-subnet routing rules
        for subnet in &state.subnets {
            for peer in &state.subnets {
                if subnet.name != peer.name {
                    rules.push(NftRule {
                        chain: "forward",
                        action: Action::Accept,
                        source: Some(subnet.cidr.into()),
                        dest: Some(peer.cidr.into()),
                        protocol: Protocol::Any,
                        ports: None,
                    });
                }
            }
        }

        rules
    }

    pub fn allocate_pod_ip(&self, subnet_name: &str, pod_id: &str) -> Result<Ipv4Addr> {
        let pool = self.pools.get(subnet_name)
            .ok_or_else(|| anyhow!("Subnet {} not found", subnet_name))?;
        pool.allocate(pod_id)
    }
}
```

#### 3.5.2 NSG (Network Security Group)

```yaml
# NSG: firewall rules for a subnet
apiVersion: z8s.io/v1
kind: Nsg
metadata:
  name: web-nsg
spec:
  rules:
    - direction: inbound
      action: allow
      protocol: tcp
      ports: [80, 443]
      source: "0.0.0.0/0"
    - direction: inbound
      action: deny
      protocol: "*"
      source: "0.0.0.0/0"
```

**Implementation:**

```rust
// network/policy.rs — 150 lines
pub struct NsgManager {
    nsgs: DashMap<String, NsgState>,
    nft: Arc<NftEngine>,
}

struct NsgState {
    rules: Vec<NsgRule>,
    nft_set: String,
}

impl NsgManager {
    pub fn apply_nsg(&self, nsg: &Nsg) -> Result<()> {
        let mut rules = Vec::new();

        for rule in &nsg.spec.rules {
            let nft_rule = match rule.direction {
                Direction::Inbound => NftRule {
                    chain: "input",
                    action: rule.action.into(),
                    source: Some(rule.source.parse()?),
                    dest: None,
                    protocol: rule.protocol.clone(),
                    ports: rule.ports.clone(),
                },
                Direction::Outbound => NftRule {
                    chain: "output",
                    action: rule.action.into(),
                    source: None,
                    dest: Some(rule.source.parse()?),
                    protocol: rule.protocol.clone(),
                    ports: rule.ports.clone(),
                },
            };
            rules.push(nft_rule);
        }

        self.nft.apply_rules(&rules)?;
        self.nsgs.insert(nsg.metadata.name.clone(), NsgState {
            rules: nsg.spec.rules.clone(),
            nft_set: format!("nsg-{}", nsg.metadata.name),
        });

        Ok(())
    }
}
```

#### 3.5.3 LoadBalancer

```yaml
apiVersion: v1
kind: Service
metadata:
  name: web-lb
spec:
  type: LoadBalancer
  ports:
    - port: 80
      targetPort: 8080
  selector:
    app: web
```

**Implementation:**

```rust
// network/lb.rs — 180 lines
pub struct LoadBalancerManager {
    lbs: DashMap<String, LbState>,
    nft: Arc<NftEngine>,
    used_ports: RwLock<BTreeSet<u16>>,
}

struct LbState {
    service: Service,
    external_ip: IpAddr,
    node_port: u16,
    backends: Vec<Backend>,
}

impl LoadBalancerManager {
    pub fn apply_lb(&self, svc: &Service) -> Result<()> {
        let external_ip = self.allocate_external_ip(svc)?;
        let node_port = self.allocate_node_port()?;
        let backends = self.resolve_backends(svc)?;

        // Create nftables DNAT rules with round-robin
        self.nft.apply_rules(&[
            NftRule {
                chain: "prerouting",
                action: Action::DNAT,
                source: None,
                dest: Some(IpNetwork::from(external_ip)),
                protocol: Protocol::TCP,
                ports: Some((svc.spec.ports[0].port, svc.spec.ports[0].port)),
                extra: Some(format!("daddr {} map @backends_{}", external_ip, svc.metadata.name)),
            },
        ])?;

        self.lbs.insert(svc.metadata.name.clone(), LbState {
            service: svc.clone(),
            external_ip,
            node_port,
            backends,
        });

        Ok(())
    }

    fn allocate_external_ip(&self, svc: &Service) -> Result<IpAddr> {
        // For bare metal: use node IP with NodePort
        // For cloud: allocate from external pool (future)
        let node_ip = crate::config::get().node_ip.parse()?;
        Ok(node_ip)
    }

    fn allocate_node_port(&self) -> Result<u16> {
        let port_range = 30000..32767;
        let mut used = self.used_ports.write();
        for port in port_range {
            if !used.contains(&port) {
                used.insert(port);
                return Ok(port);
            }
        }
        Err(anyhow!("No available NodePort"))
    }
}
```

#### 3.5.4 IPv6 Public IP Assignment

```rust
// network/ipv6.rs — 120 lines
pub struct Ipv6Pool {
    prefix: Ipv6Net,
    next: AtomicU64,
    available: RwLock<BTreeSet<u64>>,
}

impl Ipv6Pool {
    pub fn new(prefix: &str) -> Result<Self> {
        let prefix: Ipv6Net = prefix.parse()?;
        Ok(Self {
            prefix,
            next: AtomicU64::new(1),
            available: RwLock::new(BTreeSet::new()),
        })
    }

    pub fn allocate(&self) -> Ipv6Addr {
        // Check available pool first
        if let Some(iid) = self.available.write().pop_first() {
            return self.iid_to_addr(iid);
        }

        // Allocate new
        let iid = self.next.fetch_add(1, Ordering::Relaxed);
        self.iid_to_addr(iid)
    }

    pub fn release(&self, addr: Ipv6Addr) {
        let iid = self.addr_to_iid(addr);
        self.available.write().insert(iid);
    }

    fn iid_to_addr(&self, iid: u64) -> Ipv6Addr {
        let mut octets = self.prefix.network().octets();
        octets[8..16].copy_from_slice(&iid.to_be_bytes());
        Ipv6Addr::from(octets)
    }

    fn addr_to_iid(&self, addr: Ipv6Addr) -> u64 {
        let octets = addr.octets();
        u64::from_be_bytes(octets[8..16].try_into().unwrap())
    }
}
```

### 3.6 Default VNet — All Pods Get a Network

Every pod lives in a VNet. If no VNet is specified, the pod is assigned to the `default` VNet. This ensures unified network isolation design.

```rust
// network/vnet.rs — default VNet initialization
impl VNetManager {
    pub fn ensure_default_vnet(&self, default_cidr: &str) -> Result<()> {
        if !self.vnets.contains_key("default") {
            let default_vnet = Resource {
                api_version: "z8s.io/v1",
                kind: "VNet",
                metadata: ObjectMeta {
                    name: Some("default".to_string()),
                    annotations: Some({
                        let mut m = BTreeMap::new();
                        m.insert("z8s.io/default".to_string(), "true".to_string());
                        m
                    }),
                    ..Default::default()
                },
                spec: Some(VNetSpec {
                    cidr: default_cidr.to_string(),
                    subnets: vec![SubnetSpec {
                        name: "default".to_string(),
                        cidr: default_cidr.to_string(),
                        nsg: None,
                    }],
                }),
                status: None,
            };
            self.apply_vnet_typed(&default_vnet)?;
        }
        Ok(())
    }

    pub fn get_vnet_for_pod(&self, pod: &Resource<PodSpec, PodStatus>) -> String {
        pod.metadata.annotations
            .as_ref()
            .and_then(|a| a.get("z8s.io/vnet"))
            .cloned()
            .unwrap_or_else(|| "default".to_string())
    }
}
```

### 3.7 Scheduler Redesign — O(nodes) Instead of O(pods²)

**Current bottleneck:**
```rust
// O(pods²): For N unscheduled pods, each triggers a full snapshot
pub async fn scheduler_tick(store: &dyn StoreBackend, node_name: &str) {
    let pods = store.get_by_kind("Pod").await;         // O(total pods)
    for _ in pods {
        let snapshot = node_load_snapshot(store).await; // O(total pods) AGAIN
    }
}
```

**Target: Index-based O(nodes) scheduling:**

```rust
// cluster/scheduler.rs — 150 lines
pub struct Scheduler {
    node_loads: Arc<RwLock<HashMap<String, NodeLoad>>>,
    pending_tx: mpsc::Sender<PodRef>,
    pending_rx: mpsc::Receiver<PodRef>,
}

struct NodeLoad {
    pod_count: u32,
    cpu_millis: u64,
    memory_bytes: u64,
    last_heartbeat: Instant,
}

impl Scheduler {
    pub fn new() -> (Self, mpsc::Sender<PodRef>) {
        let (pending_tx, pending_rx) = mpsc::channel(1024);
        let sched = Self {
            node_loads: Arc::new(RwLock::new(HashMap::new())),
            pending_tx: pending_tx.clone(),
            pending_rx,
        };
        (sched, pending_tx)
    }

    /// Channel-driven scheduling loop — wakes only when pods need scheduling
    pub async fn run(&mut self, store: Arc<dyn StoreBackend>) {
        while let Some(pod_ref) = self.pending_rx.recv().await {
            let target = self.least_loaded_node();
            if let Err(e) = self.assign_pod(&store, &pod_ref, target).await {
                tracing::error!("Scheduling failed for {}: {}", pod_ref.name, e);
            }
        }
    }

    fn least_loaded_node(&self) -> String {
        let loads = self.node_loads.read();
        loads
            .iter()
            .filter(|(_, n)| n.last_heartbeat.elapsed() < Duration::from_secs(30))
            .min_by_key(|(_, n)| n.pod_count)
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "local".to_string())
    }

    /// Called on apply/delete to maintain the index — O(1)
    pub fn update_load(&self, node: &str, delta: i32) {
        let mut loads = self.node_loads.write();
        let entry = loads.entry(node.to_string()).or_default();
        entry.pod_count = (entry.pod_count as i32 + delta).max(0) as u32;
    }

    async fn assign_pod(&self, store: &dyn StoreBackend, pod: &PodRef, target: String) -> Result<()> {
        let mut resource = store.get("Pod", &pod.name).await
            .ok_or_else(|| anyhow!("pod disappeared"))?;
        // Set assigned_node in status
        // Store the update
        // Broadcast via gossip
        self.update_load(&target, 1);
        Ok(())
    }
}

impl Default for NodeLoad {
    fn default() -> Self {
        Self {
            pod_count: 0,
            cpu_millis: 0,
            memory_bytes: 0,
            last_heartbeat: Instant::now(),
        }
    }
}
```

**Why this fixes the 5× slowdown:** The current scheduler does O(N²) work per tick. With 2 nodes, gossip multiplies the cost. The new scheduler does O(1) work per pod (channel receive) + O(nodes) for selection. For 100 pods on 2 nodes: 100 × 2 = 200 operations instead of 100 × 100 = 10,000.

### 3.8 Batched Gossip Protocol

```rust
// cluster/gossip.rs — 120 lines
pub struct BatchedGossip {
    pending: Vec<GossipEntry>,
    peers: Vec<PeerHandle>,
    batch_interval: Duration,
}

impl BatchedGossip {
    pub async fn run(&mut self) {
        let mut ticker = tokio::time::interval(self.batch_interval);
        loop {
            ticker.tick().await;
            if self.pending.is_empty() { continue; }

            let batch: Vec<GossipEntry> = self.pending.drain(..).collect();

            // Single serialization
            let frame = serde_json::to_vec(&batch).unwrap_or_default();

            // Single send per peer (clone is cheap — Arc<[u8]> internally)
            for peer in &self.peers {
                let _ = peer.send_bytes(frame.clone()).await;
            }
        }
    }

    pub fn queue(&mut self, entry: GossipEntry) {
        self.pending.push(entry);
    }
}
```

**Expected gain:** O(1) sends per tick instead of O(N). 90% bandwidth reduction.

### 3.9 OverlayFS — O(1) Container Startup

```rust
// storage/overlay.rs — 120 lines
pub struct OverlayMount {
    lower: PathBuf,       // image cache (shared, read-only)
    upper: PathBuf,       // container-specific writes
    work: PathBuf,        // overlayfs workdir (kernel requirement)
    merged: PathBuf,      // final rootfs mount point
}

impl OverlayMount {
    pub fn mount(image_cache: &Path, container_id: &str) -> Result<Self> {
        let base = format!("/var/lib/z8s/overlay/{}", container_id);
        let upper = PathBuf::from(format!("{}/upper", base));
        let work = PathBuf::from(format!("{}/work", base));
        let merged = PathBuf::from(format!("{}/merged", base));

        std::fs::create_dir_all(&upper)?;
        std::fs::create_dir_all(&work)?;
        std::fs::create_dir_all(&merged)?;

        let opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            image_cache.display(), upper.display(), work.display()
        );

        nix::mount::mount(
            Some("overlay"), &merged, Some("overlay"),
            nix::mount::MsFlags::empty(), Some(opts.as_str()),
        )?;

        Ok(Self { lower: image_cache.to_path_buf(), upper, work, merged })
    }

    pub fn unmount(&self) -> Result<()> {
        nix::mount::umount2(&self.merged, nix::mount::MntFlags::MNT_DETACH)?;
        std::fs::remove_dir_all(self.upper.parent().unwrap())?;
        Ok(())
    }

    pub fn rootfs_path(&self) -> &Path { &self.merged }
}
```

**Gains:**
- Instant container startup (mount is O(1))
- Shared base layer: N replicas share 1× image_size disk
- 200MB nginx × 10 replicas = 200MB instead of 2GB

### 3.10 PID 1 — Multi-Core Init Process

```rust
// init.rs — 60 lines (enhanced)
pub struct InitHandler {
    signalfd: SignalFd,
}

impl InitHandler {
    pub fn new() -> Result<Self> {
        prctl::set_child_subreaper(true).ok();

        let mut mask = nix::sys::signal::SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        mask.add(Signal::SIGCHLD);  // NEW: reap ALL zombies
        mask.add(Signal::SIGHUP);
        mask.thread_block()?;

        let sigfd = SignalFd::new(&mask)?;
        Ok(Self { signalfd: sigfd })
    }

    pub async fn run(&self, shutdown: &tokio::sync::watch::Sender<bool>) -> Result<()> {
        let async_fd = AsyncFd::new(self.signalfd.as_raw_fd())?;
        loop {
            let _ = async_fd.readable().await?;
            loop {
                match self.signalfd.read_signal() {
                    Ok(Some(siginfo)) => {
                        let signo = siginfo.ssi_signo as i32;
                        match Signal::try_from(signo) {
                            Ok(Signal::SIGCHLD) => {
                                // Reap ALL zombies — not just ours
                                while let Ok(WaitStatus::Exited(pid, code)) =
                                    waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG))
                                {
                                    info!("Reaped zombie PID {} (exit {})", pid, code);
                                }
                            }
                            Ok(Signal::SIGTERM) | Ok(Signal::SIGINT) => {
                                info!("Shutdown signal received");
                                let _ = shutdown.send(true);
                                let _ = nix::sys::signal::kill(
                                    Pid::from_raw(-1), Signal::SIGTERM);
                                return Ok(());
                            }
                            Ok(Signal::SIGHUP) => {
                                info!("SIGHUP — reloading config");
                            }
                            _ => {}
                        }
                    }
                    Ok(None) => break,
                    Err(e) => { error!("SignalFd error: {}", e); break; }
                }
            }
        }
    }
}
```

**Multi-core note:** PID 1 does NOT pin to CPU 0. Tokio's work-stealing runtime handles multi-core automatically. The init handler only processes signals — it should be lightweight and non-blocking.

### 3.11 RBAC — Lightweight Role System

z8s needs basic RBAC for multi-tenant scenarios. The implementation is intentionally simple — namespace-scoped roles with service accounts.

```rust
// api/auth.rs — 120 lines
pub struct RbacManager {
    roles: Arc<RwLock<HashMap<String, Role>>>,
    bindings: Arc<RwLock<HashMap<String, RoleBinding>>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Role {
    pub name: String,
    pub namespace: String,
    pub rules: Vec<PolicyRule>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    pub api_groups: Vec<String>,
    pub resources: Vec<String>,
    pub verbs: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoleBinding {
    pub name: String,
    pub namespace: String,
    pub subjects: Vec<Subject>,
    pub role_ref: RoleRef,
}

impl RbacManager {
    pub fn authorize(&self, user: &str, namespace: &str, resource: &str, verb: &str) -> bool {
        let bindings = self.bindings.read();
        let roles = self.roles.read();

        for binding in bindings.values() {
            if binding.namespace != namespace { continue; }
            if !binding.subjects.iter().any(|s| s.name == user) { continue; }

            if let Some(role) = roles.get(&binding.role_ref.name) {
                for rule in &role.rules {
                    if rule.resources.contains(&resource.to_string())
                        && rule.verbs.contains(&verb.to_string())
                    {
                        return true;
                    }
                }
            }
        }
        false
    }
}
```

**RBAC CRDs:**

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
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: pod-manager-binding
  namespace: default
subjects:
  - kind: ServiceAccount
    name: deploy-bot
    namespace: default
roleRef:
  kind: Role
  name: pod-manager
  apiGroup: rbac.authorization.k8s.io
```

### 3.12 Capability-Aware Process Supervision

```rust
// compute/caps.rs — 80 lines
use caps::{Capability, CapSet, CapsHashSet};

pub struct CapabilityManager;

impl CapabilityManager {
    /// Apply capability additions/drops to a container process
    pub fn apply(pid: u32, caps_config: &crate::types::Capabilities) -> Result<()> {
        let pid = nix::unistd::Pid::from_raw(pid as i32);

        // Drop capabilities
        if let Some(ref drop_caps) = caps_config.drop {
            let mut current = caps::read(None, CapSet::Effective)?;
            for cap_name in drop_caps {
                if let Some(cap) = parse_capability(cap_name) {
                    current.remove(&cap);
                }
            }
            caps::set(None, CapSet::Effective, &current)?;
            caps::set(None, CapSet::Permitted, &current)?;
        }

        // Add capabilities
        if let Some(ref add_caps) = caps_config.add {
            let mut current = caps::read(None, CapSet::Effective)?;
            for cap_name in add_caps {
                if let Some(cap) = parse_capability(cap_name) {
                    current.insert(cap);
                }
            }
            caps::set(None, CapSet::Effective, &current)?;
        }

        Ok(())
    }
}

fn parse_capability(name: &str) -> Option<Capability> {
    match name.to_uppercase().as_str() {
        "NET_ADMIN" => Some(Capability::CAP_NET_ADMIN),
        "NET_RAW" => Some(Capability::CAP_NET_RAW),
        "SYS_ADMIN" => Some(Capability::CAP_SYS_ADMIN),
        "SYS_PTRACE" => Some(Capability::CAP_SYS_PTRACE),
        "NET_BIND_SERVICE" => Some(Capability::CAP_NET_BIND_SERVICE),
        "DAC_OVERRIDE" => Some(Capability::CAP_DAC_OVERRIDE),
        "SETUID" => Some(Capability::CAP_SETUID),
        "SETGID" => Some(Capability::CAP_SETGID),
        "CHOWN" => Some(Capability::CAP_CHOWN),
        "KILL" => Some(Capability::CAP_KILL),
        _ => {
            tracing::warn!("Unknown capability: {}", name);
            None
        }
    }
}
```

### 3.13 Clean nftables Chain Architecture

```
z8s_table (ip)
├── prerouting     → DNAT for NodePort, LoadBalancer, public IPs
├── input          → NSG rules for host-facing traffic
├── forward        → NSG rules for container traffic + inter-VNet routing
├── postrouting    → SNAT for outbound, MASQUERADE for pod→external
├── output         → NSG outbound rules
└── chains per VNet
    ├── vnet-{name}-input
    ├── vnet-{name}-forward
    └── vnet-{name}-postrouting
```

**Pipeline pattern:**

```rust
// network/nft.rs — 300 lines
pub struct NftPipeline {
    table: Table,
    chains: HashMap<String, Chain>,
}

impl NftPipeline {
    /// Atomic batch apply — all rules committed in one netlink transaction
    pub fn apply_atomic(&mut self, rules: &[NftRule]) -> Result<()> {
        let mut batch = nft_batch::new()?;

        for rule in rules {
            let chain = self.chains.entry(rule.chain.to_string())
                .or_insert_with(|| self.table.create_chain(&rule.chain));

            match rule.action {
                Action::DNAT => batch.add_dnat(chain, rule),
                Action::SNAT => batch.add_snat(chain, rule),
                Action::Accept => batch.add_accept(chain, rule),
                Action::Drop => batch.add_drop(chain, rule),
                Action::Reject => batch.add_reject(chain, rule),
            }
        }

        batch.commit()?;
        Ok(())
    }

    /// Full cleanup — safe to call on shutdown
    pub fn cleanup(&self) -> Result<()> {
        for chain in self.chains.values() {
            chain.flush()?;
        }
        self.table.delete()
    }
}
```

### 3.14 io_uring Integration (Feature-Gated)

```rust
// compute/image.rs
#[cfg(feature = "io_uring")]
async fn unpack_layer_uring(data: &[u8], target: &Path) -> Result<()> {
    use tokio_uring::fs::File;
    use tokio_uring::buf::IoBuf;

    let file = File::create(target.join("layer.tar")).await?;
    let buf = data.to_vec().into_inner();  // Owned buf for io_uring
    let (result, buf) = file.write_all_at(buf, 0).await;
    result?;
    file.sync_all().await?;

    // Extract tar
    let tar_path = target.join("layer.tar");
    let tar_file = std::fs::File::open(&tar_path)?;
    let mut archive = tar::Archive::new(tar_file);
    archive.unpack(target)?;
    std::fs::remove_file(tar_path)?;

    Ok(())
}

#[cfg(not(feature = "io_uring"))]
async fn unpack_layer(data: &[u8], target: &Path) -> Result<()> {
    // Existing tokio::fs path
    let tar_path = target.join("layer.tar");
    tokio::fs::write(&tar_path, data).await?;
    let tar_file = std::fs::File::open(&tar_path)?;
    let mut archive = tar::Archive::new(tar_file);
    archive.unpack(target)?;
    tokio::fs::remove_file(tar_path).await?;
    Ok(())
}
```

**Expected gain:** 2-4× throughput on image unpack (eliminates 2 context switches per 4KB block).

### 3.15 Host Process Supervision

As PID 1, z8s can run normal host processes (DHCP, SSHD, etc.) with capability enforcement:

```rust
// compute/lifecycle.rs — host process management
impl ProcessSupervisor {
    /// Spawn a host process with specific capabilities
    pub fn spawn_host_process(
        &self,
        cmd: &str,
        args: &[&str],
        caps: &[Capability],
        working_dir: Option<&str>,
        env: &[(&str, &str)],
    ) -> Result<u32> {
        use nix::unistd::{fork, ForkResult, execvp};
        use nix::unistd::chdir;

        match unsafe { fork()? } {
            ForkResult::Child => {
                // Set working directory
                if let Some(dir) = working_dir {
                    chdir(dir)?;
                }

                // Set environment
                for (k, v) in env {
                    std::env::set_var(k, v);
                }

                // Apply capabilities
                CapabilityManager::apply_raw(caps)?;

                // Exec
                let c_cmd = std::ffi::CString::new(cmd)?;
                let c_args: Vec<std::ffi::CString> = std::iter::once(c_cmd.clone())
                    .chain(args.iter().map(|a| std::ffi::CString::new(*a).unwrap()))
                    .collect();
                execvp(&c_cmd, &c_args)?;
                unreachable!()
            }
            ForkResult::Parent { child } => {
                let pid = child.as_raw() as u32;
                info!("Spawned host process '{}' with PID {}", cmd, pid);
                Ok(pid)
            }
        }
    }
}
```

---

## Part 4: Line Budget & Code Reduction Summary

### 4.1 Before/After Comparison

| Subsystem | Current LOC | Target LOC | Reduction | Technique |
|---|---|---|---|---|
| Type definitions | 2,545 | **400** | 84% | Generic `Resource<S,T>`, drop unused fields |
| API handlers (23 files) | 3,200 | **300** | 91% | Generic `CrudHandler<R>` |
| API server + infra | 947 | **200** | 79% | Remove test bloat, extract middleware |
| Container runtime | 1,259 | **250** | 80% | Unified spawn pipeline |
| Rootfs isolation | 855 | **350** | 59% | Factor shared mount logic |
| Exec | 840 | **400** | 52% | Keep protocol complexity |
| Image management | 600 | **300** | 50% | OverlayFS, shared code |
| Networking (netmux) | ~1,600 | **700** | 56% | Delete TCP proxy, clean netlink |
| Store + gossip | 1,300 | **400** | 69% | Batch gossip, simplify |
| Scheduler | 500 | **150** | 70% | Index-based O(nodes) |
| Components/reconcilers | 1,400 | **300** | 79% | Merge into pipeline |
| CLI/config/init | 1,125 | **260** | 77% | Lock helper, clean config |
| Spec builder | 587 | **100** | 83% | Derive from ResourceSpec |
| Storage | 400 | **200** | 50% | OverlayFS replaces copy |
| Tests | ~1,000 | **500** | 50% | Shared test harness |
| **Total** | **19,451** | **~4,810** | **75%** | |

### 4.2 Dependency Changes

| Action | Crates | Impact |
|---|---|---|
| Remove | `chrono`, `uuid`, `notify`, `ipnetwork`, `async-trait`, `tokio-tungstenite` | ~200KB binary, fewer deps |
| Add (optional) | `tokio-uring` (feature-gated) | 2-4× I/O throughput |
| Keep | `tokio`, `axum`, `serde*`, `nix`, `tracing`, `anyhow`, `redb`, `rustables`, `caps`, `landlock` | Core stack |
| Consider | `dashmap` | Lock-free concurrent map (or use `RwLock<HashMap>`) |

---

## Part 5: Implementation Phases

### Phase 0: Foundation (Generic Resource Framework)
**Goal:** Establish the generic resource framework without breaking existing functionality.

| Task | Files | Risk |
|---|---|---|
| Create `resource/mod.rs` with `Resource<S,T>` + `ResourceSpec` trait | New | Low |
| Create `resource/meta.rs` — extract `ObjectMeta`, `ListMeta`, `Time` from `types.rs` | New + types.rs | Low |
| Create `resource/store.rs` — move `StoreBackend` trait + `MemoryBackend` | Move | Low |
| Create `api/crud.rs` — generic CRUD handler | New | Medium |
| Migrate ONE resource (ConfigMap) end-to-end | Multiple | Medium |

**Validation:** `cargo test` passes. `kubectl get configmaps` works via generic handler.

### Phase 1: Type System Collapse
**Goal:** Migrate all resources to generic framework. Delete the god file.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Migrate Pod, Service, Deployment to `Resource<S,T>` | ~800 | ~120 |
| Migrate Namespace, ConfigMap, Secret, Node | ~400 | ~60 |
| Migrate CRDs (VNet, Subnet, NSG, RouteTable) | ~200 | ~40 |
| Delete per-type API handlers → `CrudHandler<R>` registrations | ~2,500 | ~150 |
| Delete `AnyResource` enum → type-erased `Box<dyn Any>` | ~400 | ~50 |

**Validation:** Full test suite passes. `kubectl get pods,svc,deploy,ns` all work.

### Phase 2: Runtime Unification
**Goal:** Merge three spawn paths into one `Spawner`. Integrate OverlayFS.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Create `compute/spawner.rs` with unified pipeline | — | ~250 |
| Delete `spawn_container_from_config`, `spawn_userns_container` | ~400 | — |
| Refactor `spawn_root_ns_container` → `Spawner::fork_full` | ~350 | ~100 |
| Integrate OverlayFS into image preparation | ~100 | ~80 |
| Extract shared log/probe/cgroup into helpers | ~200 | ~80 |

**Validation:** Pod lifecycle tests pass in root and non-root modes. Container startup < 200ms.

### Phase 3: Network Modernization
**Goal:** Clean networking. Remove TCP proxy. Add VNet/NSG/IPv6/LoadBalancer.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Delete `np_controller.rs` (TCP proxy) | ~200 | — |
| Verify pure nftables DNAT for NodePort + ClusterIP | — | ~30 |
| Refactor `netmux/mod.rs` — separate IPAM from veth | ~465 | ~300 |
| Add `network/vnet.rs` — VNet/Subnet orchestration | — | ~200 |
| Add `network/policy.rs` — NSG enforcement | — | ~150 |
| Add `network/ipv6.rs` — IPv6 pool + netlink | — | ~120 |
| Add `network/lb.rs` — LoadBalancer | — | ~180 |
| Ensure default VNet for all pods | — | ~50 |

**Validation:** `curl NodePort` works. Pod-to-pod connectivity. NSG blocks/allows traffic. IPv6 public IP assigned.

### Phase 4: Scheduler & Cluster
**Goal:** O(1) scheduling. Hardened leader election. Batched gossip.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Rewrite `scheduler.rs` with index-based Scheduler | ~160 | ~150 |
| Add `cluster/leader.rs` with epoch+expiry CAS | — | ~100 |
| Rewrite gossip to batched protocol | ~116 | ~120 |
| Add anti-entropy sync | — | ~80 |

**Validation:** Multi-node test: 50 pods, even distribution, leader failover. Scheduling latency < 50ms.

### Phase 5: Security & Isolation
**Goal:** Full isolation stack. RBAC. Capability management.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Implement capability management (`compute/caps.rs`) | — | ~80 |
| Add RBAC manager (`api/auth.rs`) | — | ~120 |
| Integrate Landlock for filesystem restriction | — | ~50 |
| Add seccomp profile support | — | ~80 |
| Default VNet enforcement | — | ~40 |
| Host process supervision | — | ~60 |

**Validation:** Container isolation tests. RBAC denies unauthorized access. Capabilities dropped correctly.

### Phase 6: Performance & Polish
**Goal:** io_uring, dependency removal, binary optimization, final CLOC audit.

| Task | Impact |
|---|---|
| Add `io_uring` feature flag for image unpack | 2-4× throughput |
| Remove `chrono`, `uuid`, `notify`, `ipnetwork`, `async-trait` | ~200KB binary |
| Collapse test setup into shared `TestCluster` builder | ~400 lines removed |
| CLOC audit — verify ≤ 5,000 | Quality gate |
| Benchmark suite: scheduling, startup, gossip, NodePort | Performance gate |
| Zero `unwrap()` audit in production code | Quality gate |
| Zero `unsafe` without `// SAFETY:` audit | Quality gate |

### Phase Summary

```
Phase 0: Foundation  → Generic framework + one migration
Phase 1: Types       → All resources migrated, handlers collapsed
Phase 2: Runtime     → Unified spawner + OverlayFS
Phase 3: Network     → Clean net, VNet/NSG/IPv6/LB, no proxy
Phase 4: Cluster     → O(1) scheduler, batched gossip, leader election
Phase 5: Security    → Isolation stack, RBAC, capabilities, seccomp
Phase 6: Polish      → io_uring, dep removal, benchmarks
```

---

## Part 6: Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| OverlayFS not available (non-root) | Container startup regression | Keep `copy_dir` fallback gated by `is_root()` |
| Generic CRUD doesn't cover all kubectl edge cases | API incompatibility | Escape hatches for custom handler overrides |
| `RwLock<HashMap>` contention under load | Scheduling latency | Profile first, then consider DashMap |
| Removing `chrono` breaks timestamp edge cases | Silent data corruption | Test RFC3339 roundtrip with leap seconds, timezone edge cases |
| VNet isolation leaks between subnets | Security vulnerability | Integration tests: pod in subnet A cannot reach subnet B without NSG allow |
| Leader election split-brain | Multiple schedulers | Epoch + CAS + gossip confirmation before acting |
| nftables rule accumulation on restart | Performance degradation | Full cleanup on startup + idempotent rule application |
| PID 1 zombie reaping misses grandchildren | Zombie accumulation | `waitpid(-1, WNOHANG)` loop + child subreaper |
| D-state processes block shutdown | Hung shutdown | Watchdog thread (already implemented) with 15s timeout |

---

## Part 7: Key Design Decisions — Rationale

### D1: Why not just use Kubernetes types from `kube-rs`?

The current codebase has hand-rolled K8s types that are 80% compatible. Migrating to `kube-rs` would require rewriting every API handler and every kubectl compatibility test. Instead, we keep kubectl compatibility by implementing the exact same JSON wire format, but with generic wrappers that eliminate the boilerplate.

### D2: Why `RwLock<HashMap>` over `DashMap`?

`DashMap` adds a dependency. `RwLock<HashMap>` is in stdlib. For read-heavy workloads (status queries), `RwLock` is faster because readers don't contend. For write-heavy workloads (gossip updates), `DashMap` wins. Profile before switching.

### D3: Why not MessagePack for gossip?

MessagePack is 40% smaller than JSON, but adds a dependency and makes debugging harder. The batch protocol already reduces bandwidth by 90%. JSON is fine until profiling shows gossip is a bottleneck.

### D4: Why keep `axum` WebSocket instead of `tokio-tungstenite`?

The server already uses `axum` for HTTP. `axum` has built-in WebSocket support. Using `tokio-tungstenite` for the client side means two WebSocket implementations. Unifying on `axum` simplifies the codebase.

### D6: Why a default VNet for all pods?

Without a default VNet, pods without explicit network config get host networking — no isolation. By forcing every pod into a VNet (default or specified), we ensure consistent isolation behavior. This is a security requirement, not a convenience.

### D7: Why RBAC is simple (not full Kubernetes RBAC)?

Full Kubernetes RBAC supports ClusterRole, ClusterRoleBinding, aggregation rules, and webhook authorization. For z8s's use case (single-cluster, small teams), namespace-scoped Role + RoleBinding covers 95% of needs. ClusterRole can be added later without breaking the simple model.

---

## Part 8: Testing Strategy

### 8.1 Test Pyramid

```
         ╱╲
        ╱  ╲        Integration Tests (~50)
       ╱ E2E╲       Full cluster lifecycle, multi-node
      ╱──────╲
     ╱        ╲     Component Tests (~150)
    ╱  Unit +  ╲    Spawner, scheduler, VNet, NSG, OverlayFS
   ╱ Integration╲
  ╱──────────────╲
 ╱                ╲  Unit Tests (~300)
╱  Pure functions  ╲ Resource parsing, CIDR math, capability mapping
╱────────────────────╲
```

### 8.2 Critical Test Scenarios

| Scenario | Type | What It Catches |
|---|---|---|
| Schedule 100 pods on 3 nodes | Integration | O(n²) regression, gossip convergence |
| Container startup with OverlayFS | Component | Mount failures, permission issues |
| NSG blocks cross-subnet traffic | Integration | nftables rule correctness |
| IPv6 public IP allocation | Unit | Pool exhaustion, address math |
| Leader election failover | Integration | Split-brain, stale lease |
| kubectl get all resource types | E2E | Generic CRUD completeness |
| PID 1 zombie reaping | Component | Orphan process accumulation |
| Graceful shutdown under load | Integration | D-state handling, watchdog |
| RBAC deny unauthorized | Unit | Authorization logic |
| Image pull + unpack (io_uring vs epoll) | Benchmark | Performance regression |

### 8.3 Shared Test Harness

```rust
// tests/common/mod.rs — shared across all test files
pub struct TestCluster {
    pub store: Arc<MemoryBackend>,
    pub netmux: Arc<NetMux>,
    pub scheduler: Scheduler,
    // ... common setup
}

impl TestCluster {
    pub async fn new() -> Self { /* ... */ }
    pub async fn apply_pod(&self, name: &str, image: &str) -> Resource<PodSpec, PodStatus> { /* ... */ }
    pub async fn wait_for_status(&self, name: &str, status: &str, timeout: Duration) -> Result<()> { /* ... */ }
    pub async fn cleanup(&self) { /* ... */ }
}
```

---

## Part 9: Extended CRD Strategy — Consolidation for Simplicity

The user noted: *"service, loadbalancer, apigateway, routetable so we can combine them into same CRD to reduce development complexity, we are free to extend the standard CRDs."*

This is a key insight. Instead of creating separate CRDs for every cloud feature, z8s extends standard Kubernetes CRDs with additional fields. This means:

1. **kubectl works unchanged** — no new resource types to learn
2. **Less code** — one reconciler handles multiple concerns
3. **Familiar UX** — users already know `kind: Service`

### 9.1 Extended Service CRD

The standard `Service` CRD gets extended with z8s-specific fields:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: web
  annotations:
    # z8s extensions
    z8s.io/type: loadbalancer          # or "nodeport", "clusterip"
    z8s.io/public-ip: "auto"           # or specific IPv6 address
    z8s.io/vnet: production            # which VNet this service belongs to
    z8s.io/nsg: web-nsg                # NSG to apply
spec:
  type: LoadBalancer                    # standard K8s type
  selector:
    app: web
  ports:
    - port: 80
      targetPort: 8080
      protocol: TCP
  # z8s extension: L7 routing (replaces separate Ingress for simple cases)
  x-z8s-http:
    - host: api.example.com
      path: /v1
      rewrite: /api/v1
    - host: api.example.com
      path: /v2
      rewrite: /api/v2
  # z8s extension: health check (merged from EndpointSlice)
  x-z8s-health:
    path: /healthz
    interval: 10s
    timeout: 3s
    unhealthyThreshold: 3
```

**What this consolidates:**
- `Service` (standard) — L4 load balancing
- `LoadBalancer` (cloud) — via `type: LoadBalancer` + annotations
- `Ingress` (standard) — via `x-z8s-http` extension
- `EndpointSlice` (standard) — via `x-z8s-health` extension
- Public IP assignment — via `z8s.io/public-ip` annotation

**Reconciler logic:**
```rust
// reconcile/service.rs — single reconciler handles all service types
pub async fn reconcile_service(store: &dyn StoreBackend, svc: &Resource<ServiceSpec, ServiceStatus>) -> Result<()> {
    // 1. Standard K8s behavior: ClusterIP, NodePort, LoadBalancer
    let cluster_ip = allocate_cluster_ip(svc);
    let endpoints = resolve_endpoints(store, &svc.spec.selector).await;

    // 2. z8s extensions
    if svc.metadata.annotations.get("z8s.io/type") == Some(&"loadbalancer".to_string()) {
        // Allocate public IP (IPv6 from host /64)
        let public_ip = allocate_public_ip(svc).await?;
        // Create nftables DNAT for public IP
        apply_lb_dnat(&public_ip, &endpoints).await?;
    }

    if let Some(http_rules) = &svc.spec.x_z8s_http {
        // L7 routing via ingress controller
        apply_http_rules(http_rules, &endpoints).await?;
    }

    if let Some(nsg_name) = svc.metadata.annotations.get("z8s.io/nsg") {
        // Apply NSG to service endpoints
        apply_nsg_to_endpoints(nsg_name, &endpoints).await?;
    }

    Ok(())
}
```

### 9.2 Extended Pod CRD

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: web-pod
  annotations:
    # z8s extensions
    z8s.io/vnet: production            # VNet assignment (default if omitted)
    z8s.io/subnet: web-tier            # Subnet within VNet
    z8s.io/public-ip: "true"           # Assign public IPv6
    z8s.io/capabilities: "NET_ADMIN,NET_RAW"  # Linux capabilities
    z8s.io/host-process: "false"       # Run as host process (PID 1 mode)
spec:
  containers:
    - name: web
      image: nginx:latest
      # Standard K8s fields...
      resources:
        limits:
          cpu: "500m"
          memory: "128Mi"
  # z8s extension: host process supervision (replaces separate DaemonSet for simple cases)
  x-z8s-host-processs:
    - name: sshd
      command: ["/usr/sbin/sshd", "-D"]
      capabilities: ["NET_BIND_SERVICE"]
      restart: always
    - name: dhclient
      command: ["dhclient", "eth0"]
      capabilities: ["NET_ADMIN", "NET_RAW"]
      restart: on-failure
```

**What this consolidates:**
- `Pod` (standard) — container orchestration
- VNet/Subnet assignment — via annotations
- Public IP — via annotation
- Capabilities — via annotation
- Host processes — via `x-z8s-host-process` extension
- DaemonSet (simple cases) — via host process with `restart: always`

### 9.3 Extended Deployment CRD

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 3
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
      annotations:
        z8s.io/vnet: production
        z8s.io/subnet: web-tier
    spec:
      containers:
        - name: web
          image: nginx:latest
  # z8s extension: auto-create Service + LoadBalancer
  x-z8s-service:
    type: LoadBalancer
    ports:
      - port: 80
        targetPort: 80
    public-ip: auto
    http:
      - host: web.example.com
        path: /
```

**What this consolidates:**
- `Deployment` (standard) — replica management
- `Service` (auto-created) — via `x-z8s-service`
- `LoadBalancer` (auto-created) — via `x-z8s-service.type`
- `Ingress` (auto-created) — via `x-z8s-service.http`

### 9.4 Implementation Strategy

The extended CRD approach means:

1. **Parse standard K8s fields first** — full compatibility
2. **Check for `x-z8s-*` extensions** — optional enhancements
3. **Check for `z8s.io/*` annotations** — z8s-specific features
4. **Single reconciler handles all** — no separate controllers

```rust
// reconcile/service.rs
pub async fn reconcile_service(store: &dyn StoreBackend, svc: &Resource<ServiceSpec, ServiceStatus>) -> Result<()> {
    // Standard K8s reconciliation (always)
    let cluster_ip = allocate_cluster_ip(svc);
    let endpoints = resolve_endpoints(store, &svc.spec.selector).await;
    apply_clusterip_dnat(&cluster_ip, &endpoints).await?;

    // z8s extensions (optional)
    if svc.spec.type_ == Some("LoadBalancer".to_string()) {
        reconcile_loadbalancer(svc, &endpoints).await?;
    }

    if let Some(http) = &svc.spec.x_z8s_http {
        reconcile_http_routing(http, &endpoints).await?;
    }

    if let Some(nsg) = svc.metadata.annotations.as_ref().and_then(|a| a.get("z8s.io/nsg")) {
        reconcile_nsg(nsg, &endpoints).await?;
    }

    Ok(())
}
```

### 9.5 Benefits of Consolidation

| Aspect | Separate CRDs | Extended CRDs |
|---|---|---|
| **Code size** | 6 reconcilers × 100 lines = 600 lines | 1 reconciler × 200 lines = 200 lines |
| **User learning curve** | 6 new resource types | 0 new types (annotations are familiar) |
| **kubectl compatibility** | Custom resources need `kubectl get vnet` | Standard `kubectl get svc` works |
| **Cognitive load** | "Which CRD do I edit?" | "I edit the Service" |
| **API surface** | 6 API endpoints | 1 API endpoint |
| **Flexibility** | Each CRD is rigid | Extensions are opt-in |

### 9.6 When to Use Separate CRDs

Some features are complex enough to warrant their own CRD:

- **VNet** — has its own lifecycle, subnets, peering → separate CRD
- **Subnet** — belongs to a VNet, has its own IPAM → separate CRD
- **NSG** — reusable across multiple VNets/subnets → separate CRD
- **RouteTable** — complex routing rules, VNet-specific → separate CRD
- **Role/RoleBinding** — RBAC is a cross-cutting concern → separate CRD

The rule: **If it has its own lifecycle and is referenced by multiple resources, it gets its own CRD. If it's a property of another resource, it's an extension.**

---

## Appendix A: New CRD Definitions

### VNet
```yaml
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: production
spec:
  cidr: "10.200.0.0/16"
  subnets:
    - name: web-tier
      cidr: "10.200.0.0/24"
      nsg: web-nsg
    - name: db-tier
      cidr: "10.200.1.0/24"
      nsg: db-nsg
```

### Subnet (standalone, for peering)
```yaml
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: public-web
  annotations:
    z8s.io/vnet: production
spec:
  cidr: "2001:db8:1::/64"
  public: true
  assignPublicIp: true
```

### NSG
```yaml
apiVersion: z8s.io/v1
kind: Nsg
metadata:
  name: web-nsg
spec:
  rules:
    - direction: inbound
      action: allow
      protocol: tcp
      ports: [80, 443]
      source: "0.0.0.0/0"
    - direction: outbound
      action: allow
      protocol: tcp
      ports: [443]
      destination: "0.0.0.0/0"
    - direction: inbound
      action: deny
      protocol: "*"
      source: "0.0.0.0/0"
```

### RouteTable
```yaml
apiVersion: z8s.io/v1
kind: RouteTable
metadata:
  name: production-routes
spec:
  routes:
    - destination: "10.200.0.0/24"
      nextHop: "10.200.0.1"
      vnet: production
    - destination: "0.0.0.0/0"
      nextHop: "10.0.0.1"
      vnet: production
```

## Appendix B: Feature Flags

| Flag | Default | Description |
|---|---|---|
| `io_uring` | off | Enable io_uring for file I/O (requires Linux 5.10+) |
| `rbac` | off | Enable RBAC authorization middleware |
| `ipv6` | on | Enable IPv6 dual-stack support |
| `overlayfs` | on | Enable OverlayFS for container rootfs (requires root) |
| `landlock` | on | Enable Landlock filesystem restriction |
| `seccomp` | on | Enable seccomp syscall filtering |

## Appendix C: Performance Benchmarks (Targets)

| Benchmark | Method | Target |
|---|---|---|
| Pod startup (cached image) | `kubectl apply` → `Running` | < 200ms |
| Pod startup (cold image, 50MB) | `kubectl apply` → `Running` | < 3s |
| Schedule 1 pod | Pending → Assigned | < 50ms |
| Schedule 100 pods (3 nodes) | All Pending → Running | < 5s |
| Gossip convergence (3 nodes) | Write → all nodes see it | < 500ms |
| NodePort request latency | `curl` through NodePort | < 10μs overhead |
| Image unpack (200MB, io_uring) | Pull → extracted | < 500ms |
| Binary size (static, stripped) | `ls -la` | ≤ 8MB |
| Memory usage (idle, 10 pods) | `RSS` | < 50MB |
| Memory usage (100 pods) | `RSS` | < 200MB |

