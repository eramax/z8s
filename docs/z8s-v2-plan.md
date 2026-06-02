# z8s v2 — Master Refactoring & Modernization Plan

> **Goal**: Transform z8s from a prototype into a production-grade, multi-node cloud infrastructure platform — faster than k3s, leaner than s6, with true network/storage/compute isolation and cloud-native features (VNets, subnets, NSGs, load balancers, public IPs).

---

## 1. Executive Summary

### What z8s Is Today

z8s is a 19,451-line Rust monolith that combines a PID 1 init system with a kubectl-compatible API server. It runs OCI containers via chroot/pivot_root with per-pod network namespaces, cgroups v2 resource limits, and a multi-node gossip protocol over WebSocket.

### What z8s Must Become

| Dimension | Current (v1) | Target (v2) |
|---|---|---|
| **CLOC** | 19,451 | ≤ 5,000 |
| **Multi-node scheduling** | 5× slower than single-node | ≥ 2× faster than single-node |
| **Networking** | Host TCP proxy, messy nft chains | VNet/Subnet/NSG with kernel-native DNAT, pod-to-pod L3 routing |
| **Storage** | Full rootfs copy per container | OverlayFS (copy-on-write), block device PVs |
| **Isolation** | Partial (chroot, optional PID ns) | Full (pivot_root + PID + user + IPC + net + cgroup ns) |
| **I/O model** | epoll (tokio default) | io_uring on hot paths (image unpack, log streaming) |
| **IPv6** | None | Dual-stack with public IPv6 from host /64 |
| **Crash resilience** | In-memory state lost on restart | WAL-backed state with snapshot recovery |
| **RBAC** | None | Namespace-scoped roles + service accounts |
| **PID 1** | Basic signal forwarding | Full init with capability-aware process supervision |

### Guiding Principles

1. **Every line must earn its place.** If a pattern repeats 3×, extract it. If a struct field is `Option<serde_json::Value>`, it's untyped — remove or type it.
2. **Zero-copy where possible.** Borrow over clone, `&str` over `String`, slices over `Vec`.
3. **Pipeline everything.** Every resource lifecycle is: Parse → Validate → Store → Reconcile → Actuate. One pipeline, many resource types.
4. **Kernel is the fast path.** nftables DNAT replaces userspace TCP proxy. OverlayFS replaces `cp -r`. io_uring replaces epoll for file I/O.
5. **Decouple by interface.** Every subsystem talks through a trait. Swap implementations without touching callers.

---

## 2. Current State Audit

### 2.1 Code Size Breakdown

| Subsystem | Files | Lines | % | Verdict |
|---|---|---|---|---|
| `types.rs` | 1 | **2,545** | 13% | ❌ Hand-rolled K8s types, 60% unused `Option<Value>` fields |
| `cri/runtime.rs` | 1 | **1,259** | 6.5% | ❌ Three spawn paths, massive duplication |
| `cri/rootfs.rs` | 1 | **855** | 4.4% | ⚠️ Complex but necessary — refactor not rewrite |
| `cri/exec.rs` | 1 | **840** | 4.3% | ⚠️ WebSocket protocol correct but verbose |
| `api/server.rs` | 1 | **947** | 4.9% | ❌ 400 lines duplicated test setup |
| `api/handlers/*` | 23 | **~3,200** | 16% | ❌ Identical CRUD boilerplate × 23 |
| `spec_builder.rs` | 1 | **587** | 3% | ❌ Enormous function building ContainerSpec |
| `netmux/*` | 8 | **~1,600** | 8% | ⚠️ Raw netlink bytes — correct but fragile |
| `store/*` | 9 | **~1,300** | 7% | ⚠️ Gossip works but dedup is weak |
| `main.rs` | 1 | **583** | 3% | ❌ Lock mgmt 200 lines → should be 30 |
| `node.rs` | 1 | **402** | 2% | ⚠️ Tightly coupled — no DI |
| Everything else | — | **~5,000** | 26% | Mixed |

### 2.2 Critical Architectural Problems

**P1: `types.rs` is a 2,854-line god file.** Hand-implements every K8s type. Most structs carry 20+ `Option<serde_json::Value>` fields never read. The `AnyResource` enum has 20+ variants with repetitive match arms across 15+ call sites. **Fix**: Generic `Resource<Spec, Status>` wrapper.

**P2: Three redundant container spawn paths.** `spawn_container_from_config` / `spawn_root_ns_container` / `spawn_userns_container` share 70% identical code (pipe setup, env merge, probe spawn, log tasks, cgroup). **Fix**: Single `ContainerSpawner` with `Strategy` enum.

**P3: Multi-node scheduling is O(pods²).** `scheduler_tick()` does full table scan; `node_load_snapshot()` does another. On 2 nodes with N pods, each tick is O(2N) in store plus O(N) individual WebSocket sends. **Fix**: In-memory index `HashMap<NodeName, PodCount>`, batched gossip.

**P4: No real pod-to-pod networking.** Services use userspace TCP proxy (`tokio::io::copy_bidirectional`). Pod IPs route through host nftables DNAT making the proxy redundant overhead. **Fix**: Pure nftables DNAT for ClusterIP/NodePort. Eliminate proxy.

**P5: Image unpacking copies entire rootfs per container.** 200MB nginx × 3 replicas = 600MB redundant I/O. **Fix**: OverlayFS with shared read-only lower layer.

**P6: 23 API handler files with identical CRUD boilerplate.** Every handler implements list/get/create/update/patch/delete identically. **Fix**: Generic `CrudHandler<R: ResourceSpec>`.

### 2.3 Dependency Audit

| Crate | Verdict | Action |
|---|---|---|
| `tokio`, `axum`, `serde*`, `nix`, `tracing`, `anyhow`, `redb`, `rustables`, `caps`, `landlock` | ✅ Keep | Core stack |
| `oci-distribution`, `flate2`, `tar`, `base64` | ✅ Keep | Image pipeline |
| `chrono` | ⚠️ Remove | Replace with `std::time` + 15-line RFC3339 formatter |
| `uuid` | ⚠️ Remove | Replace with `getrandom` + 12-line v4 format |
| `notify` | ⚠️ Remove | Use `nix` inotify directly (already a dep) |
| `async-trait` | ⚠️ Remove | Rust 2024 edition has native RPITIT |
| `ipnetwork` | ⚠️ Remove | Already have `parse_cidr` in config.rs |
| `tokio-tungstenite` | ⚠️ Remove | Use `axum` built-in WS (already used server-side) |
| `futures-util` | ⚠️ Minimize | Only need `StreamExt` |

**Net**: Remove 6 crates → smaller binary, fewer transitive deps.

---

## 3. Target Architecture

### 3.1 Module Hierarchy (v2)

```
src/
├── main.rs                  CLI dispatch + daemon management (~80 lines)
├── node.rs                  Node lifecycle + DI container (~100 lines)
├── config.rs                CLI parsing + global config (~120 lines)
├── init.rs                  PID 1 signal handling (~50 lines)
│
├── resource/                ── Generic resource framework ──
│   ├── mod.rs               Resource<S,T> wrapper, AnyResource enum
│   ├── meta.rs              ObjectMeta, ListMeta, Time, Quantity
│   ├── registry.rs          Type registry: kind → (de)serializer
│   └── store.rs             StoreBackend trait + Memory + Redb
│
├── api/                     ── HTTP layer ──
│   ├── server.rs            Axum router + middleware (~60 lines)
│   ├── crud.rs              Generic CRUD handler (all 6 ops from 1 trait)
│   ├── watch.rs             Watch stream (SSE-based)
│   ├── exec.rs              kubectl exec WebSocket
│   ├── proto.rs             Protobuf decoder
│   └── discovery.rs         /api, /apis, /version, /healthz
│
├── compute/                 ── Container runtime ──
│   ├── spawner.rs           Unified container spawn pipeline
│   ├── rootfs.rs            Filesystem isolation (pivot_root/chroot)
│   ├── image.rs             OCI pull + OverlayFS mount
│   ├── cgroup.rs            cgroups v2 resource limits
│   ├── health.rs            Probe runner
│   └── lifecycle.rs         Restart policy + process tracking
│
├── network/                 ── Network plane ──
│   ├── netlink.rs           Raw netlink socket operations
│   ├── nft.rs               nftables engine (DNAT, SNAT, NSG)
│   ├── veth.rs              veth pair lifecycle
│   ├── vnet.rs              VNet/Subnet IPAM
│   ├── dns.rs               In-cluster DNS server
│   ├── ipv6.rs              IPv6 public IP assignment from host /64
│   └── policy.rs            NetworkPolicy + NSG enforcement
│
├── storage/                 ── Persistent storage ──
│   ├── overlay.rs           OverlayFS mount/unmount
│   ├── provision.rs         PV/PVC binding + loop provisioner
│   └── volumes.rs           Volume mount resolution
│
├── cluster/                 ── Multi-node coordination ──
│   ├── gossip.rs            Batched gossip protocol
│   ├── scheduler.rs         O(1) index-based pod scheduling
│   ├── leader.rs            Lease-based leader election
│   └── sync.rs              Anti-entropy reconciliation
│
└── reconcile/               ── Control plane ──
    ├── mod.rs               Reconciler loop + notify-driven wake
    ├── pipeline.rs          Resource lifecycle pipeline
    ├── deployment.rs        Deployment → Pod reconciliation
    └── service.rs           Service → nftables DNAT reconciliation
```

### 3.2 Dependency Flow

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

**Key rule**: No circular deps. `resource/` is the leaf. `api/` never imports `compute/` directly.

### 3.3 Generic Resource Framework

The single biggest code reduction:

```rust
#[derive(Serialize, Deserialize)]
pub struct Resource<S, T = ()> {
    pub api_version: &'static str,
    pub kind: &'static str,
    pub metadata: ObjectMeta,
    pub spec: Option<S>,
    pub status: Option<T>,
}

pub trait ResourceSpec: Serialize + DeserializeOwned + Clone + Send + Sync {
    const API_VERSION: &'static str;
    const KIND: &'static str;
    const PLURAL: &'static str;
    const NAMESPACED: bool;
    type Status: Serialize + DeserializeOwned + Default;
}

// Pod: 8 lines instead of 60
impl ResourceSpec for PodSpec {
    const API_VERSION: &'static str = "v1";
    const KIND: &'static str = "Pod";
    const PLURAL: &'static str = "pods";
    const NAMESPACED: bool = true;
    type Status = PodStatus;
}
```

Eliminates: 20-variant `AnyResource` match arms (~400 lines), per-type boilerplate (~1,500 lines), per-type handlers (~2,000 lines).

---

## 4. Code Reduction Strategy

### 4.1 Line Budget

| Subsystem | Current | Target | Technique |
|---|---|---|---|
| Type definitions | 2,545 | **400** | Generic `Resource<S,T>`, drop unused fields |
| API handlers | 3,200 | **300** | Generic `CrudHandler<R>` |
| API server + infra | 947 | **200** | Remove dup test setup |
| Container runtime | 2,099 | **600** | Unified spawn pipeline |
| Rootfs isolation | 855 | **400** | Factor shared mount logic |
| Exec | 840 | **500** | Keep (protocol complexity) |
| Networking | 1,600 | **700** | Remove TCP proxy, clean netlink |
| Store/gossip | 1,300 | **400** | Batch gossip, simplify Redb |
| Scheduler | 500 | **200** | Index-based O(1) |
| Components | 1,400 | **300** | Merge into reconcile |
| CLI/config/init | 1,262 | **300** | flock helper |
| Spec builder | 587 | **200** | Derive from ResourceSpec |
| Storage | 400 | **200** | OverlayFS replaces copy |
| Tests | ~1,000 | **500** | Shared test harness |
| **Total** | **19,451** | **~4,800** | **75% reduction** |

---

## 5. Performance Optimization Plan

### 5.1 io_uring Integration

**Where it matters**: Image layer unpacking and log file tailing are I/O-bound hot paths.

```
Current:  tokio::fs::read() → epoll → syscall → copy to userspace → write()
Target:   io_uring submit(READ, WRITE) → zero syscall overhead → kernel does copy
```

**Implementation**: Use `tokio-uring` crate (same API surface as `tokio::fs`) behind a feature flag:

```rust
// compute/image.rs — layer unpacking
#[cfg(feature = "io_uring")]
async fn unpack_layer(data: &[u8], target: &Path) -> Result<()> {
    let ring = IoUring::new(256)?;
    // Submit READ + WRITE ops in a single batch
    // Kernel handles the copy chain without returning to userspace
}

#[cfg(not(feature = "io_uring"))]
async fn unpack_layer(data: &[u8], target: &Path) -> Result<()> {
    // Existing tar::Archive extraction
}
```

**Expected gain**: 2-4× throughput on image unpack (eliminates 2 context switches per 4KB block).

### 5.2 Zero-Copy Gossip Protocol

**Current bottleneck** (from `store/gossip.rs:74-96`):
```
broadcast_write() → serde_json::to_vec(resource)  // ALLOC #1
                  → serde_json::to_string(&msg)    // ALLOC #2
                  → json.as_bytes().to_vec()        // ALLOC #3
                  → for peer in peers { tx.send() } // N sends
```

That's 3 allocations + N sends per resource write. For a deployment scale-up of 10 pods, that's 30 allocations + 10N sends.

**Fix**: Batch + pre-serialize:
```rust
// cluster/gossip.rs — batched protocol
pub async fn flush_batch(&self) {
    let batch: Vec<GossipEntry> = self.pending.drain(..).collect();
    if batch.is_empty() { return; }
    // Single serialization, single send per peer
    let frame = rmp_serde::to_vec(&batch)?;  // MessagePack: 40% smaller than JSON
    for peer in &self.peers {
        peer.send(Message::Binary(frame.clone())).await;
    }
}
```

**Expected gain**: O(1) sends per tick instead of O(N). 90% reduction in gossip bandwidth.

### 5.3 Lock-Free Process Tracking

**Current** (`scheduler/process.rs`): `ProcessTracker` uses `Arc<Mutex<HashMap<String, RunningContainer>>>`. Every pod status query locks the entire map.

**Fix**: Replace with `DashMap` (concurrent HashMap) or `Arc<RwLock>`:
```rust
pub struct ProcessTracker {
    running: DashMap<CompactString, RunningContainer>,
    // or: Arc<RwLock<HashMap<...>>> for read-heavy workloads
}
```

**Expected gain**: Eliminates lock contention during concurrent pod status queries (kubectl get pods × N terminals).

### 5.4 Eliminating the TCP Proxy

**Current flow** (`netmux/np_controller.rs` + `components/network/service.rs`):
```
Client → NodePort → tokio::net::TcpListener → copy_bidirectional → Pod
```

Each connection consumes a tokio task + 2 socket buffers. Under load, this is 2× the memory and adds ~200μs latency per request.

**Target flow**:
```
Client → NodePort → nftables DNAT → Pod (kernel fast path, zero userspace)
```

The nftables rules already exist in `nftables.rs:302-323` (`add_nodeport_dnat`). The TCP proxy is redundant — remove it and rely purely on kernel DNAT.

**Expected gain**: ~200μs latency reduction per connection, ~50% memory reduction for proxy tasks.

---

## 6. Networking Modernization

### 6.1 Current State Analysis

The networking stack across `netmux/` has 5 files totaling ~1,600 lines:

| File | Lines | Responsibility | Issue |
|---|---|---|---|
| `mod.rs` | 465 | NetMux orchestrator, IP pools, veth lifecycle | Mixes IPAM + veth + routing |
| `nftables.rs` | 429 | NftEngine with DNAT/SNAT/NSG rules | Well-structured, keep |
| `netlink.rs` | 424 | Raw netlink for interface/routing ops | Raw byte offsets, fragile |
| `dns.rs` | 362 | In-cluster CoreDNS-compatible resolver | Works, needs SRV records |
| `np_controller.rs` | 200 | NodePort TCP proxy | **DELETE** — replaced by nft DNAT |
| `pool.rs` | 154 | IP address pool (CIDR allocation) | Clean, keep |
| `ingress.rs` | 145 | Ingress → L7 routing | Thin, keep |
| `network.rs` | ~60 | `NetworkEngine` trait | Clean |

### 6.2 VNet/Subnet Model

**New CRD hierarchy**:
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
      nsg: web-nsg          # link to NSG
    - name: db-tier
      cidr: "10.200.1.0/24"
      nsg: db-nsg

# NSG: firewall rules for a subnet
apiVersion: z8s.io/v1
kind: NetworkSecurityGroup
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

**Implementation**: When a pod is assigned to a subnet:
1. Allocate IP from subnet's pool (existing `pool.rs` logic)
2. Create veth pair + assign to pod netns (existing `netlink.rs`)
3. Apply NSG rules via nftables (existing `nftables.rs:341-367`)
4. Add SNAT for outbound traffic (existing `nftables.rs:161-176`)

Most of the kernel plumbing already exists — the new code is orchestration (~200 lines).

### 6.3 IPv6 Dual-Stack

**Design**: Host provides a /64 prefix. Each pod gets a /128 from it.

```rust
// network/ipv6.rs
pub struct Ipv6Pool {
    prefix: Ipv6Net,     // e.g., 2001:db8::/64
    next: AtomicU64,     // interface ID counter
}

impl Ipv6Pool {
    pub fn allocate(&self) -> Ipv6Addr {
        let iid = self.next.fetch_add(1, Ordering::Relaxed);
        let mut octets = self.prefix.network().octets();
        octets[8..16].copy_from_slice(&iid.to_be_bytes());
        Ipv6Addr::from(octets)
    }
}
```

**Kernel setup**: Add IPv6 address to veth via netlink `RTM_NEWADDR` (extend existing `netlink.rs`).

### 6.4 Load Balancer Implementation

```yaml
apiVersion: v1
kind: Service
metadata:
  name: web
spec:
  type: LoadBalancer
  ports:
    - port: 80
      targetPort: 8080
```

**Implementation**: LoadBalancer services get a "public" IP from a configurable external pool + nftables DNAT rules. On bare metal, this maps to the node IP with a dedicated port range (30000-32767, same as NodePort but with a stable external IP alias).

---

## 7. Scheduler Redesign

### 7.1 Current Bottleneck Analysis

From `scheduler/scheduler.rs`:

```rust
pub async fn scheduler_tick(store: &dyn StoreBackend, node_name: &str) {
    let pods = store.get_by_kind("Pod").await;    // O(all resources) scan
    for tracker in pods {
        // Check if pod needs scheduling...
        let snapshot = node_load_snapshot(store).await;  // ANOTHER O(all) scan
        // Assign to least-loaded node
    }
}
```

**Problem**: For N pods, this is O(N²) — each unscheduled pod triggers a full snapshot.

### 7.2 Index-Based O(1) Scheduling

```rust
// cluster/scheduler.rs
pub struct Scheduler {
    /// Maintained incrementally on apply/delete events
    node_loads: DashMap<CompactString, NodeLoad>,
    /// Pending pods awaiting scheduling (channel-driven)
    pending_rx: mpsc::Receiver<PodRef>,
}

struct NodeLoad {
    pod_count: u32,
    cpu_millis: u64,
    memory_bytes: u64,
    last_heartbeat: Instant,
}

impl Scheduler {
    /// Called when a pod is applied with no assigned_node
    pub fn enqueue(&self, pod: PodRef) {
        self.pending_tx.send(pod).ok();
    }

    /// Scheduling loop — wakes on channel receive
    pub async fn run(&self) {
        while let Some(pod) = self.pending_rx.recv().await {
            let target = self.least_loaded_node();  // O(nodes), not O(pods)
            self.assign(pod, target).await;
        }
    }

    fn least_loaded_node(&self) -> &str {
        self.node_loads
            .iter()
            .filter(|n| n.last_heartbeat.elapsed() < HEARTBEAT_TIMEOUT)
            .min_by_key(|n| n.pod_count)
            .map(|n| n.key().as_str())
            .unwrap_or(LOCAL_NODE)
    }

    /// Called on apply/delete to maintain the index
    pub fn update_load(&self, node: &str, delta: i32) {
        self.node_loads.entry(node.into())
            .or_default()
            .pod_count = (pod_count as i32 + delta).max(0) as u32;
    }
}
```

**Complexity**: O(nodes) per scheduling decision instead of O(pods). For 100 pods across 3 nodes, that's 3 comparisons instead of 100.

### 7.3 Leader Election Hardening

**Current issue** (from conversation history): Multiple nodes can believe they're the leader because each reads its own local `redb` instance.

**Fix**: The lease record includes an epoch + expiry. A node is leader IFF:
1. Its `holder` field matches its own node name
2. `expires_at_ms > now()`
3. The epoch matches what it wrote (CAS semantics)

```rust
// cluster/leader.rs
pub async fn try_acquire(db: &RedbBackend, node: &str) -> bool {
    let lease = db.read_lease().await;
    match lease {
        Some(l) if l.holder == node => true,  // already leader
        Some(l) if l.expires_at_ms > now_ms() => false,  // someone else holds it
        _ => {
            // Expired or missing — try to acquire
            let mut new_lease = lease.unwrap_or_default();
            RedbBackend::write_lease_epoch(&mut new_lease, node);
            db.write_lease(&new_lease).await.is_ok()
        }
    }
}
```

The `IS_SCHEDULER_LEADER` atomic flag in `config.rs` is set only after successful acquisition AND gossip confirmation.

---

## 8. Storage & OverlayFS

### 8.1 Current Problem

From `cri/image.rs:275-291` (`copy_cache_to_container`):
```rust
fn copy_cache_to_container(cache_path: &str, container_rootfs: &str, ...) -> Result<String> {
    let _ = std::fs::remove_dir_all(container_rootfs);   // blow away old rootfs
    Self::copy_dir(Path::new(cache_path), Path::new(container_rootfs))?;  // FULL COPY
    // ...
}
```

`copy_dir` is a recursive walk that copies every file. For nginx (200MB), this takes ~2 seconds and 200MB disk. With 10 replicas: 20 seconds startup + 2GB wasted disk.

### 8.2 OverlayFS Design

```
Image Cache (lower, read-only)          Container Layer (upper, read-write)
┌──────────────────────┐                ┌────────────────────┐
│  /usr/sbin/nginx     │                │  /var/log/nginx/   │ ← container writes
│  /etc/nginx/nginx.cf │                │  /tmp/session.123  │
│  /lib/x86_64-linux-  │                │  (whiteouts)       │
│    gnu/libc.so.6     │                └────────────────────┘
└──────────────────────┘                          ↓
            ↓                              overlayfs mount
    ┌───────────────────────────────────────────────┐
    │  Container rootfs (merged view)               │
    │  /usr/sbin/nginx  (from lower)                │
    │  /var/log/nginx/  (from upper)                │
    └───────────────────────────────────────────────┘
```

### 8.3 Implementation

```rust
// storage/overlay.rs
pub struct OverlayMount {
    lower: PathBuf,      // image cache (shared, read-only)
    upper: PathBuf,      // container-specific writes
    work: PathBuf,       // overlayfs workdir (required by kernel)
    merged: PathBuf,     // final rootfs mount point
}

impl OverlayMount {
    pub fn mount(image_cache: &Path, container_id: &str) -> Result<Self> {
        let base = format!("/var/lib/z8s/overlay/{}", container_id);
        let upper = PathBuf::from(format!("{}/upper", base));
        let work = PathBuf::from(format!("{}/work", base));
        let merged = PathBuf::from(format!("{}/merged", base));

        fs::create_dir_all(&upper)?;
        fs::create_dir_all(&work)?;
        fs::create_dir_all(&merged)?;

        let opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            image_cache.display(), upper.display(), work.display()
        );

        mount(
            Some("overlay"), &merged, Some("overlay"),
            MsFlags::empty(), Some(opts.as_str())
        )?;

        Ok(Self { lower: image_cache.to_path_buf(), upper, work, merged })
    }

    pub fn unmount(&self) -> Result<()> {
        umount2(&self.merged, MntFlags::MNT_DETACH)?;
        fs::remove_dir_all(self.upper.parent().unwrap())?;
        Ok(())
    }

    pub fn rootfs_path(&self) -> &Path { &self.merged }
}
```

**Gains**: Instant container startup (mount is O(1)), shared base layer saves N×image_size disk.

**Fallback**: When not running as root (no overlayfs), fall back to existing `copy_dir` — degraded mode already exists.

### 8.4 PV/PVC with Loop Devices

The existing `storage/loop_prov.rs` (159 lines) creates loop-backed block devices for PersistentVolumes. This is correct and stays. The only change: integrate OverlayFS mount into the volume resolution path so a PVC can be overlay-mounted as a container layer.

---

## 9. Container Runtime Unification

### 9.1 Current: Three Spawn Paths

`cri/runtime.rs` has three functions that share ~70% code:

| Function | Lines | Used When |
|---|---|---|
| `spawn_container_from_config` | ~200 | Non-root, user namespace, chroot |
| `spawn_root_ns_container` | ~350 | Root, double-fork, PID namespace, pivot_root |
| `spawn_userns_container` | ~200 | Non-root, user namespace (newer path) |

Shared code across all three:
- Pipe pair creation (`nix::unistd::pipe()`)
- Environment variable merging (OCI config + pod spec + ConfigMap)
- Log buffer task spawning
- Health probe scheduling
- cgroup assignment
- `RunningContainer` registration

### 9.2 Unified Spawner Design

```rust
// compute/spawner.rs

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
            IsolationStrategy::Full { pid_ns } => self.fork_full(req, rootfs, pid_ns, sync_w, ack_r)?,
            IsolationStrategy::UserNs => self.fork_userns(req, rootfs, sync_w, ack_r)?,
            IsolationStrategy::Degraded => self.fork_degraded(req, rootfs)?,
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

        // 8. Spawn log + probe tasks (shared)
        let log_buffer = spawn_log_task(child_pid);
        spawn_probes(&req.probes, child_pid);

        // 9. Register
        Ok(RunningContainer { pid: Some(child_pid), log_buffer, /* ... */ })
    }
}
```

**Result**: One 200-line function replaces three 750-line functions. All shared logic (steps 1, 3, 5-9) is written once.

### 9.3 PID 1 Hardening

The current `main.rs` handles `SIGCHLD` via `signalfd` for zombie reaping. For v2:

```rust
// init.rs — PID 1 mode
pub async fn run_as_init(node: Node) -> ! {
    // Block all signals except SIGCHLD, SIGTERM, SIGINT
    let mut signals = SignalFd::new(&[SIGCHLD, SIGTERM, SIGINT])?;

    loop {
        match signals.read_signal().await? {
            SIGCHLD => {
                // Reap ALL zombies (not just ours)
                while let Ok(status) = waitpid(Pid::from_raw(-1), WNOHANG) {
                    match status {
                        WaitStatus::Exited(pid, code) => node.handle_exit(pid, code).await,
                        WaitStatus::StillAlive => break,
                        _ => {}
                    }
                }
            }
            SIGTERM | SIGINT => {
                node.graceful_shutdown().await;
                std::process::exit(0);
            }
        }
    }
}
```

**Key**: As PID 1, we are responsible for reaping ALL orphaned processes, not just our direct children. The `waitpid(-1, WNOHANG)` loop handles this.

---

## 10. Implementation Phases

### Phase 0: Foundation (Week 1)
**Goal**: Establish the generic resource framework without breaking existing functionality.

| Task | Files Touched | Risk |
|---|---|---|
| Create `resource/mod.rs` with `Resource<S,T>` + `ResourceSpec` trait | New file | Low |
| Create `resource/meta.rs` — extract `ObjectMeta`, `ListMeta`, `Time` from `types.rs` | New + `types.rs` | Low |
| Create `resource/store.rs` — move `StoreBackend` trait + `MemoryBackend` | Move from `store/` | Low |
| Create `api/crud.rs` — generic CRUD handler | New file | Medium |
| Migrate ONE resource (ConfigMap) to generic framework end-to-end | Multiple | Medium |

**Validation**: `cargo test` passes. `kubectl get configmaps` works via generic handler.

### Phase 1: Type System Collapse (Week 2)
**Goal**: Migrate all resources to the generic framework. Delete `types.rs` god file.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Migrate Pod, Service, Deployment to `Resource<S,T>` | ~800 | ~120 |
| Migrate Namespace, ConfigMap, Secret, Node | ~400 | ~60 |
| Migrate CRDs (VNet, Subnet, NSG, RouteTable) | ~200 | ~40 |
| Delete per-type API handlers, replace with `CrudHandler<R>` registrations | ~2,500 | ~150 |
| Delete `AnyResource` enum, replace with type-erased `Box<dyn Resource>` | ~400 | ~50 |

**Validation**: Full integration test suite passes. `kubectl get pods,svc,deploy,ns` all work.

### Phase 2: Runtime Unification (Week 3)
**Goal**: Merge the three spawn paths into one `Spawner`.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Create `compute/spawner.rs` with unified pipeline | — | ~250 |
| Delete `spawn_container_from_config`, `spawn_userns_container` | ~400 | — |
| Refactor `spawn_root_ns_container` into `Spawner::fork_full` | ~350 | ~100 |
| Integrate OverlayFS into image preparation | ~100 (copy_dir) | ~80 |
| Extract shared log/probe/cgroup into helper functions | ~200 | ~80 |

**Validation**: Pod lifecycle tests pass in both root and non-root modes.

### Phase 3: Network Modernization (Week 4)
**Goal**: Clean networking, remove TCP proxy, add VNet/NSG orchestration.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Delete `np_controller.rs` (TCP proxy) | ~200 | — |
| Verify pure nftables DNAT path for NodePort + ClusterIP | — | ~30 (test) |
| Refactor `netmux/mod.rs` — separate IPAM from veth lifecycle | ~465 | ~300 (split) |
| Add `network/ipv6.rs` — IPv6 pool + netlink RTM_NEWADDR | — | ~80 |
| Add `network/policy.rs` — NSG enforcement via existing nft engine | — | ~60 |

**Validation**: `curl NodePort` works. Pod-to-pod connectivity via veth L3. NSG blocks/allows traffic.

### Phase 4: Scheduler & Cluster (Week 5)
**Goal**: O(1) scheduling, hardened leader election, batched gossip.

| Task | Lines Removed | Lines Added |
|---|---|---|
| Rewrite `scheduler.rs` with index-based `Scheduler` | ~160 | ~120 |
| Add `cluster/leader.rs` with epoch+expiry CAS | — | ~60 |
| Rewrite gossip to batch + MessagePack | ~116 | ~80 |
| Add anti-entropy sync (checksum → diff → apply) | — | ~60 |

**Validation**: Multi-node test: deploy 50 pods, verify even distribution, verify leader failover.

### Phase 5: Polish & Performance (Week 6)
**Goal**: io_uring integration, dependency removal, binary size optimization.

| Task | Impact |
|---|---|
| Add `io_uring` feature flag for image unpack | 2-4× image unpack throughput |
| Remove `chrono`, `uuid`, `notify`, `ipnetwork`, `async-trait` | ~200KB binary reduction |
| Collapse test setup into shared `TestCluster` builder | ~400 lines removed |
| CLOC audit — verify ≤5,000 lines | Quality gate |
| Benchmark: scheduling latency, pod startup time, gossip bandwidth | Performance gate |

### Phase Summary

```
Week 1: Foundation     → Generic framework + one migration
Week 2: Types          → All resources migrated, handlers collapsed
Week 3: Runtime        → Unified spawner + OverlayFS
Week 4: Network        → Clean net, no proxy, IPv6, NSG
Week 5: Cluster        → O(1) scheduler, batched gossip
Week 6: Polish         → io_uring, dep removal, benchmarks
```

**Total estimated effort**: 6 weeks of focused development.

---

## 11. Success Criteria

| Metric | Target | How to Measure |
|---|---|---|
| CLOC | ≤ 5,000 | `cloc --include-lang=Rust src/` |
| Binary size | ≤ 8MB (static, stripped) | `ls -la target/release/z8s` |
| Pod startup (cached image) | < 200ms | Timer from apply → Running |
| Multi-node scheduling latency | < 50ms per pod | Timer from Pending → Assigned |
| Gossip convergence (3 nodes) | < 500ms | Timer from write → all nodes agree |
| NodePort latency overhead | < 10μs | Comparison: direct pod IP vs NodePort |
| Test coverage | ≥ 80% of public API | `cargo tarpaulin` |
| Zero `unwrap()` in production | 0 | `grep -r 'unwrap()' src/ --include='*.rs' \| grep -v test \| grep -v '#\[cfg(test)\]'` |
| Zero `unsafe` without `// SAFETY:` | 0 | Manual audit |

---

## 12. Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| OverlayFS not available (non-root) | Container startup regression | Keep `copy_dir` fallback, gated by `is_root()` |
| io_uring kernel version requirement | Feature unavailable on older hosts | Feature flag, epoll fallback is default |
| Generic CRUD handler doesn't cover all kubectl edge cases | API incompatibility | Keep escape hatches for custom handler overrides |
| MessagePack gossip breaks backward compat with v1 nodes | Rolling upgrade failure | Version field in gossip handshake, fall back to JSON |
| Removing `chrono` breaks timestamp parsing | Silent data corruption | Test RFC3339 roundtrip in CI with edge cases |
| DashMap adds a new dependency | Violates no-new-deps rule | Alternative: `RwLock<HashMap>` (zero deps) |
