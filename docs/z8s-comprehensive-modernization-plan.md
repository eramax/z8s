# z8s Comprehensive Modernization Plan

> **Date:** 2026-06-02
> **Goal:** Transform z8s into a production-grade, multi-node cloud infrastructure platform that surpasses k3s and s6 in performance, isolation, and cloud-native features.
> **Target:** CLOC ≤ 5,000, scheduling ≥ 2× faster with multi-node, full VNet/Subnet/NSG/LoadBalancer support, efficient OverlayFS, clean nftables management, PID 1 capable with multi-core support.

---

## Table of Contents

- [Part 1: Architecture & Code Reduction](#part-1-architecture--code-reduction)
- [Part 2: Networking & Cloud Features](#part-2-networking--cloud-features)
- [Part 3: Performance & CRI Optimization](#part-3-performance--cri-optimization)
- [Part 4: Security, Isolation & New Features](#part-4-security-isolation--new-features)
- [Part 5: Implementation Phases & Success Criteria](#part-5-implementation-phases--success-criteria)

---

# Part 1: Architecture & Code Reduction

## 1. Current State Audit

### 1.1 Code Size Analysis (19,451 LOC → 5,000 LOC target)

| File | Current LOC | Target LOC | Reduction Technique |
|------|-------------|------------|---------------------|
| `types.rs` | 2,853 | 350 | Generic `Resource<S,T>`, drop unused `Option<Value>` fields |
| `api/server.rs` | 1,031 | 120 | Remove inline handlers, extract middleware |
| `api/handlers/*` (23 files) | ~3,200 | 250 | Generic `CrudHandler<R>` |
| `cri/runtime.rs` | 1,414 | 300 | Unified `ContainerSpawner` with strategy enum |
| `cri/rootfs.rs` | 1,023 | 350 | Factor shared mount logic |
| `cri/exec.rs` | 915 | 400 | Keep protocol complexity, reduce boilerplate |
| `netmux/mod.rs` | 585 | 200 | Split IPAM from veth lifecycle |
| `netmux/nftables.rs` | 479 | 180 | Chain management pipeline |
| `netmux/netlink.rs` | 530 | 250 | Safe wrapper with builder pattern |
| `scheduler/*` | 588 | 180 | Index-based O(1) scheduling |
| `components/*` | 1,051 | 200 | Merge into reconcile pipeline |
| `store/*` | 922 | 300 | Batched gossip, simplified persistence |
| `main.rs` | 738 | 80 | Flock helper, CLI dispatch |
| `node.rs` | 474 | 100 | DI container with clean dependency flow |
| `spec_builder.rs` | 626 | 150 | Derive from `ResourceSpec` |
| `storage/*` | 495 | 150 | OverlayFS replaces copy |
| **Total** | **19,451** | **~4,800** | **75% reduction** |

### 1.2 Critical Architectural Problems

**P1: Monolithic `types.rs` (2,853 lines)**
- Hand-rolled K8s types with 60% unused `Option<Value>` fields
- `AnyResource` enum with 20+ variants, repetitive match arms across 15+ call sites
- **Fix:** Generic `Resource<S,T>` wrapper + `ResourceSpec` trait

**P2: Three redundant container spawn paths**
- `spawn_container_from_config`, `spawn_root_ns_container`, `spawn_userns_container` share 70% code
- **Fix:** Single `ContainerSpawner` with `IsolationStrategy` enum

**P3: Multi-node scheduling is O(pods²)**
- `scheduler_tick()` does full table scan; `node_load_snapshot()` does another
- **Fix:** In-memory `HashMap<NodeName, PodCount>`, channel-driven scheduling

**P4: No real pod-to-pod networking**
- Services use userspace TCP proxy (`tokio::io::copy_bidirectional`)
- **Fix:** Pure nftables DNAT for ClusterIP/NodePort, eliminate proxy

**P5: Image unpacking copies entire rootfs per container**
- 200MB nginx × 3 replicas = 600MB redundant I/O
- **Fix:** OverlayFS with shared read-only lower layer

**P6: 23 API handler files with identical CRUD boilerplate**
- **Fix:** Generic `CrudHandler<R: ResourceSpec>`

## 2. Target Module Hierarchy

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
│   ├── policy.rs            NetworkPolicy + NSG enforcement
│   └── lb.rs                LoadBalancer implementation
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

## 3. Dependency Flow (No Circular Dependencies)

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

**Key Rule:** `resource/` is the leaf. `api/` never imports `compute/` directly.

## 4. Generic Resource Framework

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

// Example: Pod (8 lines instead of 60)
impl ResourceSpec for PodSpec {
    const API_VERSION: &'static str = "v1";
    const KIND: &'static str = "Pod";
    const PLURAL: &'static str = "pods";
    const NAMESPACED: bool = true;
    type Status = PodStatus;
}
```

**Eliminates:**
- 20-variant `AnyResource` match arms (~400 lines)
- Per-type boilerplate (~1,500 lines)
- Per-type handlers (~2,000 lines)

## 5. Generic CRUD Handler

Replace 23 handler files with one generic implementation:

```rust
// api/crud.rs
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

    pub async fn create(state: State<AppState>, body: Json<Value>) -> Json<Value> {
        let resource: Resource<R> = serde_json::from_value(body.0)?;
        state.store.apply(resource.clone()).await;
        Json(build_create_response(resource))
    }

    // ... update, patch, delete with same pattern
}
```

**Router registration becomes:**
```rust
let router = Router::new()
    .route("/api/v1/pods", get(CrudHandler::<PodSpec>::list).post(CrudHandler::<PodSpec>::create))
    .route("/api/v1/pods/{name}", get(CrudHandler::<PodSpec>::get).delete(CrudHandler::<PodSpec>::delete))
    .route("/api/v1/services", get(CrudHandler::<ServiceSpec>::list).post(CrudHandler::<ServiceSpec>::create))
    // ... 20 more resources, 1 line each
```

## 6. Unified Container Spawner

```rust
pub enum IsolationStrategy {
    Full { pid_ns: bool },
    UserNs,
    Degraded,
}

pub struct Spawner {
    image_mgr: Arc<ImageManager>,
    cgroup_mgr: Arc<CgroupManager>,
    netmux: Arc<NetMux>,
}

impl Spawner {
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

        // 5-9. Shared post-fork logic (network, cgroup, logs, probes)
        // ...
    }
}
```

## 7. Clean nftables Chain Management

```rust
// network/nft.rs
pub struct NftChainManager {
    base_chain: Chain,
}

impl NftChainManager {
    /// Single entry point for all chain operations
    pub fn apply_rules(&self, rules: &[NftRule]) -> Result<()> {
        let batch = Batch::new(self.base_chain.table());
        for rule in rules {
            match rule.action {
                Action::DNAT => self.add_dnat(&mut batch, rule),
                Action::SNAT => self.add_snat(&mut batch, rule),
                Action::Accept => self.add_accept(&mut batch, rule),
                Action::Drop => self.add_drop(&mut batch, rule),
            }
        }
        batch.commit()
    }

    /// Clean up all chains on shutdown
    pub fn cleanup(&self) -> Result<()> {
        self.base_chain.table().flush()?;
        self.base_chain.table().delete()
    }
}
```

**Pipeline pattern for network lifecycle:**
```
Parse CRD → Validate → Store → Reconcile → Apply nftables rules → Verify
```

---

# Part 2: Networking & Cloud Features

## 8. VNet/Subnet/NSG Architecture

### 8.1 VNet Model

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

### 8.2 Implementation

```rust
// network/vnet.rs
pub struct VNetManager {
    vnets: DashMap<String, VNetState>,
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
            self.pools.write().insert(subnet.name.clone(), pool);
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
}
```

### 8.3 NSG Implementation

```rust
// network/policy.rs
pub struct NsgManager {
    nsgs: DashMap<String, NsgState>,
}

struct NsgState {
    rules: Vec<NsgRule>,
    nft_set: String,
}

impl NsgManager {
    pub fn apply_nsg(&self, nsg: &Nsg) -> Result<()> {
        let mut rules = Vec::new();

        for rule in &nsg.spec.rules {
            match rule.direction {
                Direction::Inbound => {
                    rules.push(NftRule {
                        chain: "input",
                        action: rule.action.into(),
                        source: Some(rule.source.parse()?),
                        dest: None,
                        protocol: rule.protocol.clone(),
                        ports: rule.ports.clone(),
                    });
                }
                Direction::Outbound => {
                    rules.push(NftRule {
                        chain: "output",
                        action: rule.action.into(),
                        source: None,
                        dest: Some(rule.source.parse()?),
                        protocol: rule.protocol.clone(),
                        ports: rule.ports.clone(),
                    });
                }
            }
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

## 9. IPv6 Dual-Stack Support

### 9.1 IPv6 Pool

```rust
// network/ipv6.rs
pub struct Ipv6Pool {
    prefix: Ipv6Net,
    next: AtomicU64,
}

impl Ipv6Pool {
    pub fn new(prefix: &str) -> Result<Self> {
        let prefix: Ipv6Net = prefix.parse()?;
        Ok(Self {
            prefix,
            next: AtomicU64::new(1),
        })
    }

    pub fn allocate(&self) -> Ipv6Addr {
        let iid = self.next.fetch_add(1, Ordering::Relaxed);
        let mut octets = self.prefix.network().octets();
        octets[8..16].copy_from_slice(&iid.to_be_bytes());
        Ipv6Addr::from(octets)
    }

    pub fn release(&self, addr: Ipv6Addr) {
        // Convert back to IID and mark as available
        let octets = addr.octets();
        let iid = u64::from_be_bytes(octets[8..16].try_into().unwrap());
        self.available.write().insert(iid);
    }
}
```

### 9.2 Netlink IPv6 Support

```rust
// network/netlink.rs
impl Netlink {
    pub fn add_ipv6_address(&self, ifindex: i32, addr: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let msg = Newaddrmsg {
            family: AF_INET6 as u8,
            prefix_len,
            index: ifindex as i32,
            address: addr.octets().to_vec(),
            ..Default::default()
        };

        self.send_recv(RTM_NEWADDR, NLM_F_CREATE | NLM_F_ACK, msg)?;
        Ok(())
    }

    pub fn add_ipv6_route(&self, dest: Ipv6Net, gateway: Option<Ipv6Addr>, ifindex: i32) -> Result<()> {
        let msg = Rtmsg {
            family: AF_INET6 as u8,
            dst_len: dest.prefix_len(),
            src_len: 0,
            table: RT_TABLE_MAIN as u8,
            protocol: RTPROT_BOOT as u8,
            scope: RT_SCOPE_UNIVERSE as u8,
            kind: RTN_UNICAST as u8,
            // ... route attributes
        };

        self.send_recv(RTM_NEWROUTE, NLM_F_CREATE | NLM_F_ACK, msg)?;
        Ok(())
    }
}
```

### 9.3 Public IP Assignment

```yaml
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: public-web
spec:
  cidr: "2001:db8:1::/64"
  public: true
  assignPublicIp: true
```

```rust
impl VNetManager {
    pub fn assign_public_ip(&self, pod_name: &str, subnet: &str) -> Result<Ipv6Addr> {
        let pool = self.ipv6_pools.get(subnet)
            .ok_or_else(|| anyhow!("Subnet {} not found", subnet))?;

        let addr = pool.allocate();

        // Add to pod's network namespace
        self.netmux.add_ipv6_to_pod(pod_name, addr)?;

        // Add nftables rule for public routing
        self.nft.apply_rules(&[NftRule {
            chain: "prerouting",
            action: Action::DNAT,
            source: None,
            dest: Some(IpNetwork::from(addr)),
            protocol: Protocol::Any,
            ports: None,
        }])?;

        Ok(addr)
    }
}
```

## 10. LoadBalancer Implementation

### 10.1 LoadBalancer Service

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

### 10.2 Implementation

```rust
// network/lb.rs
pub struct LoadBalancerManager {
    lbs: DashMap<String, LbState>,
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
        let node_port = self.allocate_node_port(svc)?;

        let backends = self.resolve_backends(svc)?;

        // Create nftables DNAT rules
        self.nft.apply_rules(&[
            // DNAT from external IP to backends
            NftRule {
                chain: "prerouting",
                action: Action::DNAT,
                source: None,
                dest: Some(IpNetwork::from(external_ip)),
                protocol: Protocol::TCP,
                ports: Some((svc.spec.ports[0].port, svc.spec.ports[0].port)),
            },
            // Round-robin load balancing
            NftRule {
                chain: "prerouting",
                action: Action::DNAT,
                source: None,
                dest: Some(IpNetwork::from(external_ip)),
                protocol: Protocol::TCP,
                ports: Some((svc.spec.ports[0].port, svc.spec.ports[0].port)),
                // Use nftables maps for round-robin
                extra: Some(format!("daddr {} map @backends", external_ip)),
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
        // For cloud: allocate from external pool
        let node_ip = self.get_node_ip()?;
        Ok(node_ip)
    }

    fn allocate_node_port(&self, svc: &Service) -> Result<u16> {
        let port_range = 30000..32767;
        // Find available port in range
        for port in port_range {
            if !self.used_ports.read().contains(&port) {
                self.used_ports.write().insert(port);
                return Ok(port);
            }
        }
        Err(anyhow!("No available NodePort"))
    }
}
```

## 11. Clean nftables Chain Architecture

### 11.1 Chain Hierarchy

```
z8s-table
├── prerouting (DNAT for NodePort, LoadBalancer)
├── input (NSG rules for host)
├── forward (NSG rules for containers)
├── postrouting (SNAT for outbound)
└── chains per VNet
    ├── vnet-production-prerouting
    ├── vnet-production-forward
    └── vnet-production-postrouting
```

### 11.2 Chain Management Pipeline

```rust
pub struct NftPipeline {
    chains: DashMap<String, Chain>,
}

impl NftPipeline {
    pub fn apply(&self, rules: &[NftRule]) -> Result<()> {
        let batch = Batch::new(&self.table);

        for rule in rules {
            match rule.action {
                Action::DNAT => {
                    batch.add(rulechain!("prerouting", rule));
                }
                Action::SNAT => {
                    batch.add(rulechain!("postrouting", rule));
                }
                Action::Accept => {
                    batch.add(rulechain!("forward", rule));
                }
                Action::Drop => {
                    batch.add(rulechain!("forward", rule));
                }
            }
        }

        batch.commit()?;
        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        // Flush all chains
        for chain in self.chains.iter() {
            chain.value().flush()?;
        }
        // Delete table
        self.table.delete()
    }
}
```

---

# Part 3: Performance & CRI Optimization

## 12. Scheduler Redesign (O(n²) → O(1))

### 12.1 Current Bottleneck

```rust
// Current: O(pods²)
pub async fn scheduler_tick(store: &dyn StoreBackend, node_name: &str) {
    let pods = store.get_by_kind("Pod").await;  // O(all resources)
    for tracker in pods {
        let snapshot = node_load_snapshot(store).await;  // ANOTHER O(all)
        // Assign to least-loaded node
    }
}
```

### 12.2 Index-Based O(1) Scheduling

```rust
// cluster/scheduler.rs
pub struct Scheduler {
    /// Maintained incrementally on apply/delete events
    node_loads: DashMap<CompactString, NodeLoad>,
    /// Pending pods awaiting scheduling (channel-driven)
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

**Complexity:** O(nodes) per scheduling decision instead of O(pods). For 100 pods across 3 nodes: 3 comparisons instead of 100.

### 12.3 Batched Gossip Protocol

```rust
// cluster/gossip.rs
pub struct BatchedGossip {
    pending: Vec<GossipEntry>,
    peers: Vec<Peer>,
}

impl BatchedGossip {
    pub fn queue(&mut self, entry: GossipEntry) {
        self.pending.push(entry);
    }

    pub async fn flush_batch(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }

        let batch: Vec<GossipEntry> = self.pending.drain(..).collect();

        // Single serialization, single send per peer
        let frame = rmp_serde::to_vec(&batch)?;  // MessagePack: 40% smaller than JSON

        for peer in &self.peers {
            peer.send(Message::Binary(frame.clone())).await?;
        }

        Ok(())
    }
}
```

**Expected gain:** O(1) sends per tick instead of O(N). 90% reduction in gossip bandwidth.

## 13. OverlayFS Implementation

### 13.1 Current Problem

```rust
// cri/image.rs:275-291
fn copy_cache_to_container(cache_path: &str, container_rootfs: &str) -> Result<String> {
    let _ = std::fs::remove_dir_all(container_rootfs);
    Self::copy_dir(Path::new(cache_path), Path::new(container_rootfs))?;  // FULL COPY
    Ok(container_rootfs.to_string())
}
```

200MB nginx × 3 replicas = 600MB redundant I/O.

### 13.2 OverlayFS Design

```rust
// storage/overlay.rs
pub struct OverlayMount {
    lower: PathBuf,
    upper: PathBuf,
    work: PathBuf,
    merged: PathBuf,
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

    pub fn rootfs_path(&self) -> &Path {
        &self.merged
    }
}
```

**Gains:** Instant container startup (mount is O(1)), shared base layer saves N×image_size disk.

### 13.3 Image Cache Strategy

```
/var/lib/z8s/images/
├── sha256:abc123/          ← extracted layers (immutable lower dirs)
│   ├── layer-0/
│   ├── layer-1/
│   └── layer-2/
├── sha256:def456/
│   └── ...
└── index.json              ← image → digest mapping
```

## 14. io_uring Integration

### 14.1 Hot Paths

```rust
// compute/image.rs
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

### 14.2 Expected Gains

| Operation | Current (epoll) | Target (io_uring) |
|-----------|-----------------|-------------------|
| Image unpack (200MB) | ~2s | ~500ms |
| Log streaming | 2 context switches/4KB | 0 context switches |
| Manifest loading | Syscall per file | Batch submit |

## 15. Zero-Copy Process Tracking

```rust
// compute/lifecycle.rs
pub struct ProcessTracker {
    running: DashMap<CompactString, RunningContainer>,
}

impl ProcessTracker {
    pub fn register(&self, id: String, container: RunningContainer) {
        self.running.insert(id, container);
    }

    pub fn get_status(&self, id: &str) -> Option<ContainerStatus> {
        self.running.get(id).map(|c| c.status())
    }

    pub fn reap_zombies(&self) {
        // Non-blocking waitpid loop
        while let Ok(status) = waitpid(Pid::from_raw(-1), WNOHANG) {
            match status {
                WaitStatus::Exited(pid, code) => {
                    self.running.retain(|_, c| c.pid != Some(pid));
                }
                WaitStatus::StillAlive => break,
                _ => {}
            }
        }
    }
}
```

---

# Part 4: Security, Isolation & New Features

## 16. True Isolation Stack

### 16.1 Per-Container Isolation

```
┌─────────────────────────────────────────────────────────┐
│  Linux Namespaces                                        │
│  NEWUSER + NEWNS + NEWPID + NEWNET + NEWIPC + NEWUTS    │
│  ┌───────────────────────────────────────────────────┐  │
│  │  pivot_root into OCI rootfs (overlayfs)           │  │
│  │  ┌─────────────────────────────────────────────┐  │  │
│  │  │  Landlock (filesystem + network restrict)   │  │  │
│  │  │  ┌───────────────────────────────────────┐  │  │  │
│  │  │  │  seccomp-bpf (syscall allowlist)      │  │  │  │
│  │  │  │  ┌─────────────────────────────────┐  │  │  │  │
│  │  │  │  │  Capability drop                │  │  │  │  │
│  │  │  │  │  execvp(entrypoint)             │  │  │  │  │
│  │  │  │  └─────────────────────────────────┘  │  │  │  │
│  │  │  └───────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### 16.2 Network Isolation

```rust
// network/policy.rs
pub struct NetworkIsolation {
    netns: NetworkNamespace,
}

impl NetworkIsolation {
    pub fn create_isolated_netns(&self, pod_id: &str) -> Result<NetworkNamespace> {
        // Create new network namespace
        let netns = NetworkNamespace::new(format!("z8s-{}", pod_id))?;

        // Create veth pair
        let (host_veth, pod_veth) = self.create_veth_pair(pod_id)?;

        // Move pod end to container namespace
        self.move_to_netns(&pod_veth, &netns)?;

        // Assign IP from subnet pool
        let ip = self.ipam.allocate(pod_id)?;
        self.assign_ip(&pod_veth, ip)?;

        // Apply NSG rules
        self.apply_nsg_rules(&netns, pod_id)?;

        Ok(netns)
    }
}
```

### 16.3 Storage Isolation

```rust
// storage/isolation.rs
pub struct StorageIsolation {
    overlay: OverlayMount,
    volumes: Vec<VolumeMount>,
}

impl StorageIsolation {
    pub fn create_isolated_rootfs(&self, container_id: &str, image: &str) -> Result<Self> {
        // Mount overlayfs with image as lower layer
        let overlay = OverlayMount::mount(&self.image_cache(image), container_id)?;

        // Create isolated upper layer per container
        let upper = PathBuf::from(format!("/var/lib/z8s/overlay/{}/upper", container_id));
        fs::create_dir_all(&upper)?;

        Ok(Self { overlay, volumes: Vec::new() })
    }

    pub fn mount_volume(&mut self, vol: &VolumeSpec) -> Result<()> {
        match vol {
            VolumeSpec::EmptyDir { medium: Some("Memory") } => {
                // Mount tmpfs in upper layer
                self.mount_tmpfs(&vol.mount_path)?;
            }
            VolumeSpec::HostPath { path } => {
                // Bind mount from host
                self.bind_mount(path, &vol.mount_path)?;
            }
            VolumeSpec::ConfigMap { name } => {
                // Materialize configmap as files
                self.materialize_configmap(name, &vol.mount_path)?;
            }
            _ => {}
        }
        Ok(())
    }
}
```

### 16.4 Resource Limits

```rust
// compute/cgroup.rs
pub struct CgroupManager {
    base_path: PathBuf,
}

impl CgroupManager {
    pub fn create_cgroup(&self, pod_id: &str, limits: &ResourceLimits) -> Result<()> {
        let path = self.base_path.join(pod_id);
        fs::create_dir_all(&path)?;

        // Memory limits
        if let Some(memory) = limits.memory {
            fs::write(path.join("memory.max"), memory.to_string())?;
        }

        // CPU limits
        if let Some(cpu) = limits.cpu {
            fs::write(path.join("cpu.max"), cpu.to_string())?;
        }

        // PID limits
        if let Some(pids) = limits.pids {
            fs::write(path.join("pids.max"), pids.to_string())?;
        }

        Ok(())
    }

    pub fn assign_pid(&self, pid: u32, pod_id: &str) -> Result<()> {
        let path = self.base_path.join(pod_id).join("cgroup.procs");
        fs::write(path, pid.to_string())?;
        Ok(())
    }
}
```

## 17. PID 1 Support

### 17.1 Multi-Core Support

```rust
// init.rs
pub async fn run_as_init(node: Node) -> ! {
    // Block all signals except SIGCHLD, SIGTERM, SIGINT
    let mut signals = SignalFd::new(&[SIGCHLD, SIGTERM, SIGINT])?;

    // Pin to CPU 0 for init operations
    unsafe {
        let mut cpuset = std::mem::zeroed::<cpu_set_t>();
        CPU_SET(0, &mut cpuset);
        sched_setaffinity(0, std::mem::size_of::<cpu_set_t>(), &cpuset);
    }

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

### 17.2 Running Normal Processes

```rust
// node.rs
impl Node {
    pub fn run_host_process(&self, cmd: &str) -> Result<()> {
        // Execute in host namespace
        let child = unsafe { fork()? };

        if child == 0 {
            // Child: exec the command
            execvp(cmd, &[cmd])?;
        } else {
            // Parent: wait for completion
            waitpid(Pid::from(child), None)?;
        }

        Ok(())
    }

    pub fn run_dhcp(&self) -> Result<()> {
        self.run_host_process("dhclient")?;
        Ok(())
    }

    pub fn run_sshd(&self) -> Result<()> {
        self.run_host_process("/usr/sbin/sshd")?;
        Ok(())
    }
}
```

### 17.3 Capability-Aware Process Supervision

```rust
// compute/lifecycle.rs
pub struct ProcessSupervisor {
    capabilities: CapabilitySet,
}

impl ProcessSupervisor {
    pub fn spawn_with_caps(&self, cmd: &str, caps: &[Capability]) -> Result<u32> {
        let child = unsafe { fork()? };

        if child == 0 {
            // Drop all capabilities except specified
            self.drop_capabilities(caps)?;
            execvp(cmd, &[cmd])?;
        }

        Ok(child as u32)
    }

    fn drop_capabilities(&self, keep: &[Capability]) -> Result<()> {
        for set in [CapSet::Bounding, CapSet::Effective, CapSet::Permitted] {
            let mut caps = CapsHashSet::new();
            for cap in keep {
                caps.insert(*cap);
            }
            caps::set(None, set, &caps)?;
        }
        Ok(())
    }
}
```

## 18. RBAC (Roles)

### 18.1 RBAC Model

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: pod-manager
  namespace: default
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch", "create", "update", "patch", "delete"]
  - apiGroups: [""]
    resources: ["pods/log"]
    verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: pod-manager-binding
  namespace: default
subjects:
  - kind: ServiceAccount
    name: default
    namespace: default
roleRef:
  kind: Role
  name: pod-manager
  apiGroup: rbac.authorization.k8s.io
```

### 18.2 Implementation

```rust
// api/auth.rs
pub struct RbacManager {
    roles: DashMap<String, Role>,
    role_bindings: DashMap<String, RoleBinding>,
}

impl RbacManager {
    pub fn authorize(&self, user: &str, resource: &str, verb: &str) -> bool {
        // Find role bindings for user
        for binding in self.role_bindings.iter() {
            if binding.subjects.iter().any(|s| s.name == user) {
                let role = self.roles.get(&binding.role_ref.name);
                if let Some(role) = role {
                    for rule in &role.rules {
                        if rule.resources.contains(&resource.to_string())
                            && rule.verbs.contains(&verb.to_string())
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}
```

## 19. Default VNet for Pods

```yaml
apiVersion: z8s.io/v1
kind: VNet
metadata:
  name: default
  annotations:
    z8s.io/default: "true"
spec:
  cidr: "10.244.0.0/16"
  subnets:
    - name: default
      cidr: "10.244.0.0/24"
```

```rust
// network/vnet.rs
impl VNetManager {
    pub fn get_vnet_for_pod(&self, pod: &Pod) -> &VNetState {
        // Check pod's vnet annotation
        if let Some(vnet_name) = pod.metadata.annotations.get("z8s.io/vnet") {
            return self.vnets.get(vnet_name).unwrap();
        }

        // Use default VNet
        self.vnets.get("default").unwrap()
    }
}
```

---

# Part 5: Implementation Phases & Success Criteria

## 20. Implementation Phases

### Phase 0: Foundation (Week 1)
**Goal:** Establish generic resource framework without breaking existing functionality.

| Task | Files Touched | Risk |
|------|---------------|------|
| Create `resource/mod.rs` with `Resource<S,T>` + `ResourceSpec` trait | New file | Low |
| Create `resource/meta.rs` — extract `ObjectMeta`, `ListMeta`, `Time` from `types.rs` | New + `types.rs` | Low |
| Create `resource/store.rs` — move `StoreBackend` trait + `MemoryBackend` | Move from `store/` | Low |
| Create `api/crud.rs` — generic CRUD handler | New file | Medium |
| Migrate ONE resource (ConfigMap) to generic framework end-to-end | Multiple | Medium |

**Validation:** `cargo test` passes. `kubectl get configmaps` works via generic handler.

### Phase 1: Type System Collapse (Week 2)
**Goal:** Migrate all resources to generic framework. Delete `types.rs` god file.

| Task | Lines Removed | Lines Added |
|------|---------------|-------------|
| Migrate Pod, Service, Deployment to `Resource<S,T>` | ~800 | ~120 |
| Migrate Namespace, ConfigMap, Secret, Node | ~400 | ~60 |
| Migrate CRDs (VNet, Subnet, NSG, RouteTable) | ~200 | ~40 |
| Delete per-type API handlers, replace with `CrudHandler<R>` registrations | ~2,500 | ~150 |
| Delete `AnyResource` enum, replace with type-erased `Box<dyn Resource>` | ~400 | ~50 |

**Validation:** Full integration test suite passes. `kubectl get pods,svc,deploy,ns` all work.

### Phase 2: Runtime Unification (Week 3)
**Goal:** Merge three spawn paths into one `Spawner`.

| Task | Lines Removed | Lines Added |
|------|---------------|-------------|
| Create `compute/spawner.rs` with unified pipeline | — | ~250 |
| Delete `spawn_container_from_config`, `spawn_userns_container` | ~400 | — |
| Refactor `spawn_root_ns_container` into `Spawner::fork_full` | ~350 | ~100 |
| Integrate OverlayFS into image preparation | ~100 (copy_dir) | ~80 |
| Extract shared log/probe/cgroup into helper functions | ~200 | ~80 |

**Validation:** Pod lifecycle tests pass in both root and non-root modes.

### Phase 3: Network Modernization (Week 4)
**Goal:** Clean networking, remove TCP proxy, add VNet/NSG orchestration.

| Task | Lines Removed | Lines Added |
|------|---------------|-------------|
| Delete `np_controller.rs` (TCP proxy) | ~200 | — |
| Verify pure nftables DNAT path for NodePort + ClusterIP | — | ~30 (test) |
| Refactor `netmux/mod.rs` — separate IPAM from veth lifecycle | ~465 | ~300 (split) |
| Add `network/ipv6.rs` — IPv6 pool + netlink RTM_NEWADDR | — | ~80 |
| Add `network/policy.rs` — NSG enforcement via existing nft engine | — | ~60 |

**Validation:** `curl NodePort` works. Pod-to-pod connectivity via veth L3. NSG blocks/allows traffic.

### Phase 4: Scheduler & Cluster (Week 5)
**Goal:** O(1) scheduling, hardened leader election, batched gossip.

| Task | Lines Removed | Lines Added |
|------|---------------|-------------|
| Rewrite `scheduler.rs` with index-based `Scheduler` | ~160 | ~120 |
| Add `cluster/leader.rs` with epoch+expiry CAS | — | ~60 |
| Rewrite gossip to batch + MessagePack | ~116 | ~80 |
| Add anti-entropy sync (checksum → diff → apply) | — | ~60 |

**Validation:** Multi-node test: deploy 50 pods, verify even distribution, verify leader failover.

### Phase 5: Security & Isolation (Week 6)
**Goal:** Full isolation stack, RBAC, capability dropping.

| Task | Lines Removed | Lines Added |
|------|---------------|-------------|
| Implement Landlock integration | — | ~50 |
| Implement seccomp profile loading | — | ~80 |
| Implement capability dropping | — | ~60 |
| Add RBAC manager | — | ~150 |
| Add default VNet for pods | — | ~40 |

**Validation:** Container isolation tests pass. RBAC denies unauthorized access.

### Phase 6: Performance & Polish (Week 7)
**Goal:** io_uring integration, dependency removal, binary size optimization.

| Task | Impact |
|------|--------|
| Add `io_uring` feature flag for image unpack | 2-4× image unpack throughput |
| Remove `chrono`, `uuid`, `notify`, `ipnetwork`, `async-trait` | ~200KB binary reduction |
| Collapse test setup into shared `TestCluster` builder | ~400 lines removed |
| CLOC audit — verify ≤5,000 lines | Quality gate |
| Benchmark: scheduling latency, pod startup time, gossip bandwidth | Performance gate |

## 21. Success Criteria

| Metric | Target | How to Measure |
|--------|--------|----------------|
| CLOC | ≤ 5,000 | `cloc --include-lang=Rust src/` |
| Binary size | ≤ 8MB (static, stripped) | `ls -la target/release/z8s` |
| Pod startup (cached image) | < 200ms | Timer from apply → Running |
| Multi-node scheduling latency | < 50ms per pod | Timer from Pending → Assigned |
| Gossip convergence (3 nodes) | < 500ms | Timer from write → all nodes agree |
| NodePort latency overhead | < 10μs | Comparison: direct pod IP vs NodePort |
| Test coverage | ≥ 80% of public API | `cargo tarpaulin` |
| Zero `unwrap()` in production | 0 | `grep -r 'unwrap()' src/ --include='*.rs' \| grep -v test` |
| Zero `unsafe` without `// SAFETY:` | 0 | Manual audit |

## 22. Dependency Reduction

| Crate | Action | Replacement |
|-------|--------|-------------|
| `chrono` | Remove | `std::time` + 15-line RFC3339 formatter |
| `uuid` | Remove | `getrandom` + 12-line v4 format |
| `notify` | Remove | `nix` inotify directly |
| `async-trait` | Remove | Rust 2024 edition RPITIT |
| `ipnetwork` | Remove | `parse_cidr` in config.rs |
| `tokio-tungstenite` | Remove | `axum` built-in WS |
| `futures-util` | Minimize | Only `StreamExt` |

**Net:** Remove 6 crates → smaller binary, fewer transitive deps.

## 23. Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| OverlayFS not available (non-root) | Container startup regression | Keep `copy_dir` fallback, gated by `is_root()` |
| io_uring kernel version requirement | Feature unavailable on older hosts | Feature flag, epoll fallback is default |
| Generic CRUD handler doesn't cover all kubectl edge cases | API incompatibility | Keep escape hatches for custom handler overrides |
| MessagePack gossip breaks backward compat with v1 nodes | Rolling upgrade failure | Version field in gossip handshake, fall back to JSON |
| Removing `chrono` breaks timestamp parsing | Silent data corruption | Test RFC3339 roundtrip in CI with edge cases |
| DashMap adds a new dependency | Violates no-new-deps rule | Alternative: `RwLock<HashMap>` (zero deps) |

---

## 24. Phase Summary

```
Week 1: Foundation     → Generic framework + one migration
Week 2: Types          → All resources migrated, handlers collapsed
Week 3: Runtime        → Unified spawner + OverlayFS
Week 4: Network        → Clean net, no proxy, IPv6, NSG, LoadBalancer
Week 5: Cluster        → O(1) scheduler, batched gossip, RBAC
Week 6: Security       → Isolation stack, capabilities, seccomp
Week 7: Polish         → io_uring, dep removal, benchmarks
```

**Total estimated effort:** 7 weeks of focused development.

---

*This plan supersedes all previous plans. It is based on a complete audit of the z8s codebase and incorporates lessons learned from youki/libcontainer, k3s, and s6 architectures.*
