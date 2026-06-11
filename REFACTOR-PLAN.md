# z8s Refactoring & Optimization Plan

## Architecture: Two Binaries

```
z8s (CLI)                          z8s-node (daemon)
┌─────────────────────┐            ┌──────────────────────────────────┐
│ Lightweight process  │            │ Full node process                 │
│ Spawns z8s-node      │──spawn──▶ │ DB (redb)                         │
│ Shows status         │            │ API server (:6443)                │
│ Stop/start/restart   │            │ CRI (containers, images, exec)    │
│ Join tokens          │            │ Network (veth, nftables, DNS)     │
│ Kubeconfig           │            │ Scheduler (pod assignment)        │
│ Reset/cleanup        │            │ Gossip (multi-node sync)          │
└─────────────────────┘            │ Reconciler (watch + apply)        │
                                   └──────────────────────────────────┘
```

### Process Tree

```
$ z8s start
$ ps aux | grep z8s

z8s                                # CLI: just a process manager
├── z8s-node --port 6443           # main node (holds global lock)
├── z8s-node --port 7443           # worker node (peers with main)
└── z8s-node --port 8443           # worker node (peers with main)

$ z8s status
main:6443   PID=12345
node:7443   PID=12346
node:8443   PID=12347

$ z8s stop                        # sends SIGTERM to all
$ z8s stop --port 7443            # stops one node
$ z8s reset                       # stops all, wipes DB + rootfs
```

### What each binary knows

| Binary | Size | Dependencies | Knows about |
|--------|------|-------------|-------------|
| `z8s` (CLI) | ~200 LOC | `std` only, no async | Lock files, PIDs, signal sending |
| `z8s-node` | ~500 LOC wiring | core, runtime, network, api, sync | Everything (it's the node) |

### Why the split

- **CLI can crash safely** — it's stateless, just a process manager
- **Node is self-contained** — one binary, one process, everything inside
- **CLI can manage multiple nodes** — start 3 nodes from one terminal
- **No daemon mode needed** — CLI spawns node directly, no double-fork
- **Clean signal handling** — CLI sends SIGTERM, node handles graceful shutdown

---

## Current Problems

| Problem | Where | Impact |
|---------|-------|--------|
| Single 27K LOC monolith | One crate, ~100 files | Slow compiles, tight coupling, no test isolation |
| `main.rs` is 1140 LOC | CLI + daemon + process mgmt + kubeconfig | Unreadable |
| Gossip uses JSON over WebSocket | `store/ws.rs` | 3-5x larger payloads, slow |
| Anti-entropy is a no-op | `store/anti_entropy.rs` | Just logs a hash, never syncs |
| Full state dump on every peer connect | `ws.rs` SyncRequest | O(N) even for 1 changed key |
| No vector clocks | `gossip.rs` uses `term: u64` | No conflict detection |
| Duplicated gossip handling | `ws.rs` has two near-identical functions | ~200 LOC copy-paste |
| API handlers repeat CRUD boilerplate | `handlers/crd.rs`, `apply.rs` | Each resource = ~100 LOC of similar code |
| Components depend on everything | `components/mod.rs` imports cri, netmux, scheduler, storage, store | No module testable alone |
| `AnyResource` match arms everywhere | Every handler, every reconciler | Verbose, no compile-time safety |
| `netmux/mod.rs` is 797 LOC | One file with NetMux + 20 free functions | God module |

---

## Target Package Structure

**8 packages.** Every name tells you what it does.

```
z8s/
├── Cargo.toml                      # workspace root
│
├── core/                           # types, store, events, syscalls
│   ├── types/                      # all k8s + z8s resource types
│   │   ├── mod.rs                  # re-exports
│   │   ├── meta.rs                 # ObjectMeta, Time, Labels
│   │   ├── resource.rs             # Resource trait, AnyResource, ResourceState
│   │   ├── compute.rs              # Pod, Deployment, ContainerSpec, Probes
│   │   ├── network.rs              # Service, EndpointSlice, VNet, Subnet, NSG
│   │   ├── storage.rs              # PV, PVC, StorageClass, ConfigMap, Secret
│   │   ├── control.rs              # Namespace, Node, Event, ServiceAccount, RBAC
│   │   └── helpers.rs              # Quantity, IntOrString, generic ops
│   │
│   ├── syscall.rs                  # Direct Linux syscalls (mount, unshare, chroot, etc.)
│   ├── store.rs                    # StoreBackend trait, StoreOp, StoreEvent
│   ├── redb.rs                     # RedbBackend (persistence)
│   ├── memory.rs                   # MemoryBackend (testing)
│   ├── hub.rs                      # StoreEventHub (event bus)
│   ├── lease.rs                    # Heartbeat, leader election
│   └── token.rs                    # Join tokens
│
├── runtime/                        # container lifecycle: pull, spawn, exec, health
│   ├── mod.rs                      # RuntimeProvider trait
│   ├── spawn.rs                    # Fork, namespaces, pipes (the big one)
│   ├── image.rs                    # OCI pull, layer cache
│   ├── rootfs.rs                   # chroot, mount
│   ├── cgroup.rs                   # cgroups v2
│   ├── exec.rs                     # kubectl exec (PTY + pipe)
│   ├── health.rs                   # Liveness/readiness probes
│   └── spec.rs                     # ContainerSpec builder
│
├── network/                        # veth, nftables, DNS, ingress, IPAM
│   ├── mod.rs                      # NetMux, NetworkEngine trait
│   ├── ipam.rs                     # IP pool, CIDR, subnet allocation
│   ├── veth.rs                     # Veth pair create/delete/move
│   ├── nft.rs                      # nftables rules (DNAT, SNAT, filter)
│   ├── dns.rs                      # In-cluster DNS
│   ├── ingress.rs                  # Ingress HTTP listener
│   └── netlink.rs                  # Raw netlink ops (sysctl, routes, addrs)
│
├── sync/                           # multi-node: gossip, anti-entropy, vector clocks
│   ├── mod.rs                      # GossipEngine, GossipState
│   ├── wire.rs                     # Binary protocol types + encode/decode
│   ├── transport.rs                # WebSocket client + server
│   ├── merkle.rs                   # Merkle tree for anti-entropy
│   ├── clock.rs                    # Vector clock
│   └── apply.rs                    # Incoming batch apply logic
│
├── controller/                     # THE CONTROL LOOP (was scheduler + reconciler)
│   ├── mod.rs                      # run_controller (assign + reconcile)
│   ├── assign.rs                   # Node selection (least-loaded)
│   ├── index.rs                    # In-memory node load index
│   └── process.rs                  # ProcessTracker (running containers)
│
├── api/                            # HTTP API (thin shell over store)
│   ├── mod.rs                      # AppState, router, TLS, server
│   ├── handler.rs                  # Generic CrudHandler<R> (one file, all resources)
│   ├── apply.rs                    # kubectl apply
│   ├── watch.rs                    # Watch support (subscribe to StoreEventHub)
│   ├── auth.rs                     # Token registry + RBAC middleware
│   ├── proto.rs                    # Protobuf decoder
│   ├── table.rs                    # kubectl get table formatting
│   └── catalog.rs                  # API resource catalog + auto-routes
│
├── z8s/                            # CLI binary: process manager only
│   ├── Cargo.toml                  # deps: std only (no async, no tokio)
│   └── src/
│       ├── main.rs                 # dispatch: ~100 LOC
│       ├── spawn.rs                # spawn z8s-node processes
│       ├── status.rs               # read lock files, show status
│       ├── stop.rs                 # send SIGTERM, wait, SIGKILL
│       ├── lock.rs                 # flock-based lock management
│       └── kubeconfig.rs           # write ~/.kube/config
│
└── z8s-node/                       # node binary: the complete node
    ├── Cargo.toml                  # deps: core, runtime, network, api, sync, controller
    └── src/
        ├── main.rs                 # parse args, call run_node(): ~50 LOC
        ├── run.rs                  # wire everything together: ~300 LOC
        ├── bootstrap.rs            # init store, TLS, admin SA, defaults
        └── signal.rs               # SIGTERM → graceful shutdown → _exit
```
z8s/
├── Cargo.toml                      # workspace root
│
├── core/                           # types, store, events, syscalls
│   ├── types/                      # all k8s + z8s resource types
│   │   ├── mod.rs                  # re-exports
│   │   ├── meta.rs                 # ObjectMeta, Time, Labels
│   │   ├── resource.rs             # Resource trait, AnyResource, ResourceState
│   │   ├── compute.rs              # Pod, Deployment, ContainerSpec, Probes
│   │   ├── network.rs              # Service, EndpointSlice, VNet, Subnet, NSG
│   │   ├── storage.rs              # PV, PVC, StorageClass, ConfigMap, Secret
│   │   ├── control.rs              # Namespace, Node, Event, ServiceAccount, RBAC
│   │   └── helpers.rs              # Quantity, IntOrString, generic ops
│   │
│   ├── syscall.rs                  # Direct Linux syscalls (mount, unshare, chroot, etc.)
│   ├── store.rs                    # StoreBackend trait, StoreOp, StoreEvent
│   ├── redb.rs                     # RedbBackend (persistence)
│   ├── memory.rs                   # MemoryBackend (testing)
│   ├── hub.rs                      # StoreEventHub (event bus)
│   ├── lease.rs                    # Heartbeat, leader election
│   └── token.rs                    # Join tokens
│
├── runtime/                        # container lifecycle: pull, spawn, exec, health
│   ├── mod.rs                      # RuntimeProvider trait
│   ├── spawn.rs                    # Fork, namespaces, pipes (the big one)
│   ├── image.rs                    # OCI pull, layer cache
│   ├── rootfs.rs                   # chroot, mount
│   ├── cgroup.rs                   # cgroups v2
│   ├── exec.rs                     # kubectl exec (PTY + pipe)
│   ├── health.rs                   # Liveness/readiness probes
│   └── spec.rs                     # ContainerSpec builder
│
├── network/                        # veth, nftables, DNS, ingress, IPAM
│   ├── mod.rs                      # NetMux, NetworkEngine trait
│   ├── ipam.rs                     # IP pool, CIDR, subnet allocation
│   ├── veth.rs                     # Veth pair create/delete/move
│   ├── nft.rs                      # nftables rules (DNAT, SNAT, filter)
│   ├── dns.rs                      # In-cluster DNS
│   ├── ingress.rs                  # Ingress HTTP listener
│   └── netlink.rs                  # Raw netlink ops (sysctl, routes, addrs)
│
├── sync/                           # multi-node: gossip, anti-entropy, vector clocks
│   ├── mod.rs                      # GossipEngine, GossipState
│   ├── wire.rs                     # Binary protocol types + encode/decode
│   ├── transport.rs                # WebSocket client + server
│   ├── merkle.rs                   # Merkle tree for anti-entropy
│   ├── clock.rs                    # Vector clock
│   └── apply.rs                    # Incoming batch apply logic
│
├── api/                            # HTTP API (thin shell over store)
│   ├── mod.rs                      # AppState, router, TLS, server
│   ├── handler.rs                  # Generic CrudHandler<R>
│   ├── apply.rs                    # kubectl apply
│   ├── watch.rs                    # Watch support (subscribe to StoreEventHub)
│   ├── auth.rs                     # Token registry + RBAC middleware
│   ├── proto.rs                    # Protobuf decoder
│   ├── table.rs                    # kubectl get table
│   └── catalog.rs                  # API resource catalog
│
├── controller/                     # THE CONTROL LOOP (was scheduler + reconciler)
│   ├── mod.rs                      # run_controller (assign + reconcile)
│   ├── assign.rs                   # Node selection (least-loaded)
│   ├── index.rs                    # In-memory node load index
│   └── process.rs                  # ProcessTracker (running containers)
│
├── z8s/                            # CLI binary: process manager only
│   ├── Cargo.toml                  # deps: std only (no async, no tokio)
│   └── src/
│       ├── main.rs                 # dispatch: ~100 LOC
│       ├── spawn.rs                # spawn z8s-node processes
│       ├── status.rs               # read lock files, show status
│       ├── stop.rs                 # send SIGTERM, wait, SIGKILL
│       ├── lock.rs                 # flock-based lock management
│       └── kubeconfig.rs           # write ~/.kube/config
│
└── z8s-node/                       # node binary: the complete node
    ├── Cargo.toml                  # deps: core, runtime, network, api, sync, controller
    └── src/
        ├── main.rs                 # parse args
        ├── run.rs                  # wire everything
        ├── bootstrap.rs            # init store, TLS, defaults
        └── signal.rs               # graceful shutdown
```

### Why 8, not 13+

| Package | Responsibility | What it does NOT know about |
|---------|---------------|------------------------------|
| **core** | Types, store, events, syscalls | runtime, network, api, controller |
| **runtime** | Container lifecycle | api, controller, network, sync |
| **network** | Veths, nftables, DNS, IPAM | api, controller, runtime, sync |
| **sync** | Multi-node gossip + consensus | api, controller, runtime, network |
| **controller** | Reconcile loop (assign + start/stop) | api (reads from store, not API) |
| **api** | HTTP handlers + auth | runtime, network, sync, controller |
| **z8s** (CLI) | Process management | Everything about containers/networking |
| **z8s-node** | The node itself | Nothing (just connects boxes) |

API is a thin HTTP shell over the store. Controller is the brain that does all real work.

### Dependency Graph

```
core           (zero internal deps)
  ↑
runtime        (depends on: core)
network        (depends on: core)
sync           (depends on: core)
  ↑
controller     (depends on: core, runtime, network, sync)
  ↑
api            (depends on: core only — reads/writes store)
  ↑
z8s-node       (depends on: all above)

z8s (CLI)      (standalone — no deps on any z8s-* crate)
```

No cycles. CLI is completely independent.

### Key Merges

| Before | After | Why |
|--------|-------|-----|
| `z8s-types/` (14 files) + `z8s-store/` (14 files) + `z8s-components/` (mod.rs) | `core/types/` + `core/store.rs` + `core/hub.rs` | Types + persistence + event bus are one concept: "what is the truth" |
| `z8s-cri/` (16 files) | `runtime/` (8 files) | Container lifecycle is one job |
| `z8s-netmux/` (12 files) | `network/` (7 files) | Networking is one job |
| `z8s-gossip/` (6 files) | `sync/` (6 files) | Same, better name |
| `scheduler/` + `orchestrator/` | `controller/` (4 files) | One control loop, two roles |
| `z8s-api/` (18 files) | `api/` (8 files) | Components removed — handler IS the component |
| `main.rs` (1140) + `node.rs` (648) | `z8s/` (CLI) + `z8s-node/` (node) | Two clean binaries, each with one job |
| `nix` crate (all syscalls) | `core/syscall.rs` (direct libc) | Zero wrappers, direct kernel calls, smaller binary |

---

## Functional Programming Patterns

### 1. The Resource Trait — Eliminate Match Arms

Every k8s resource implements one trait. No more `match resource { AnyResource::Pod(p) => ... AnyResource::Service(s) => ... }` everywhere.

```rust
// core/types/resource.rs

pub trait Resource:
    Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    /// e.g. "Pod", "Service", "VNet"
    fn kind() -> &'static str;
    /// e.g. "v1", "apps/v1", "z8s.io/v1"
    fn api_version() -> &'static str;
    /// true for Pod, Service; false for Node, Namespace
    fn namespaced() -> bool;
    /// Short name for kubectl: "po", "svc", "no"
    fn short_name() -> &'static str { "" }
    /// Verbs this resource supports
    fn verbs() -> &'static [Verb] { &[Verb::Get, Verb::List, Verb::Watch] }

    fn metadata(&self) -> &ObjectMeta;
    fn metadata_mut(&mut self) -> &mut ObjectMeta;

    fn namespace(&self) -> Option<&str> {
        self.metadata().namespace.as_deref()
    }
    fn name(&self) -> &str {
        self.metadata().name.as_deref().unwrap_or("")
    }
    fn uid(&self) -> &str {
        self.metadata().uid.as_deref().unwrap_or("")
    }

    fn into_any(self) -> AnyResource;
    fn from_any(any: AnyResource) -> Option<Self>;
}
```

Usage — the compiler enforces correctness:

```rust
// Before: every handler has this
match &tracker.resource {
    AnyResource::Pod(p) => { /* 50 lines */ }
    AnyResource::Service(s) => { /* 50 lines */ }
    AnyResource::ConfigMap(c) => { /* 50 lines */ }
    _ => {} // silently ignore unknown kinds
}

// After: zero match arms
fn reconcile<R: Resource>(ctx: &Ctx, resource: &R) -> Result<()> {
    let meta = resource.metadata();
    // ... generic logic using metadata() ...
}
```

### 2. Generic CRUD — One Handler, All Resources

```rust
// api/handler.rs

pub struct Crud<R: Resource> {
    _p: PhantomData<R>,
}

impl<R: Resource> Crud<R> {
    pub async fn list(state: State<App>, Path(ns): Path<String>) -> Json<List<R>> {
        let items = Query::from(&*state.store)
            .kind(R::kind())
            .namespace(&ns)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|r| R::from_any(r.spec))
            .collect();
        Json(List { items, ..Default::default() })
    }

    pub async fn get(
        state: State<App>,
        Path((ns, name)): Path<(String, String)>,
    ) -> Result<Json<R>, ApiErr> {
        Query::from(&*state.store)
            .kind(R::kind())
            .namespace(&ns)
            .collect::<Vec<_>>()
            .into_iter()
            .find(|r| r.spec.name() == name)
            .and_then(|r| R::from_any(r.spec))
            .map(Json)
            .ok_or_else(|| ApiErr::not_found(format!("{} not found", name)))
    }

    pub async fn create(
        state: State<App>,
        body: Bytes,
    ) -> Result<(StatusCode, Json<R>), ApiErr> {
        let mut resource: R = parse_body(&body)?;
        resource.metadata_mut().uid.get_or_insert_with(gen_uid);
        resource.metadata_mut().creation_timestamp.get_or_insert_with(now);
        state.store.write_spec(resource.clone().into_any(), None).await?;
        Ok((StatusCode::CREATED, Json(resource)))
    }

    pub async fn delete(
        state: State<App>,
        Path((ns, name)): Path<(String, String)>,
    ) -> Result<StatusCode, ApiErr> {
        let record = Query::from(&*state.store)
            .kind(R::kind())
            .namespace(&ns)
            .collect::<Vec<_>>()
            .into_iter()
            .find(|r| r.spec.name() == name)
            .ok_or_else(|| ApiErr::not_found(format!("{} not found", name)))?;
        state.store.set_deletion_timestamp(&record.spec.uid()).await?;
        Ok(StatusCode::OK)
    }
}
```

Route registration — one line per resource:

```rust
// api/mod.rs

fn routes() -> Router {
    let r = Router::new();

    // Helper macro: one line registers GET/POST/GET{name}/DELETE{name}
    macro_rules! crud {
        ($path:expr, $type:ty) => {{
            let p = $path;
            r.route(&format!("{}/{{ns}}", p),
                get(Crud::<$type>::list).post(Crud::<$type>::create))
             .route(&format!("{}/{{ns}}/{{name}}", p),
                get(Crud::<$type>::get).delete(Crud::<$type>::delete))
        }};
    }

    let r = crud!("/api/v1/namespaces", Namespace);
    let r = crud!("/api/v1/pods", Pod);
    let r = crud!("/api/v1/services", Service);
    let r = crud!("/api/v1/configmaps", ConfigMap);
    let r = crud!("/api/v1/secrets", Secret);
    let r = crud!("/apis/apps/v1/deployments", Deployment);
    // ... each is ONE line
    r
}
```

### 3. Store Query Builder — Declarative Reads

```rust
// core/store.rs

pub struct Query<'a> {
    store: &'a dyn StoreBackend,
    kind: Option<&'a str>,
    ns: Option<&'a str>,
    labels: Vec<(&'a str, &'a str)>,
}

impl<'a> Query<'a> {
    pub fn from(store: &'a dyn StoreBackend) -> Self {
        Self { store, kind: None, ns: None, labels: vec![] }
    }
    pub fn kind(mut self, k: &'a str) -> Self { self.kind = Some(k); self }
    pub fn namespace(mut self, ns: &'a str) -> Self { self.ns = Some(ns); self }
    pub fn label(mut self, k: &'a str, v: &'a str) -> Self {
        self.labels.push((k, v)); self
    }

    pub async fn collect(self) -> Vec<ResourceRecord> {
        let items = match self.kind {
            Some(k) => self.store.get_by_kind(k).await,
            None => self.store.get_all().await,
        };
        items.into_iter()
            .filter(|r| self.ns.map_or(true, |ns| r.spec.namespace() == Some(ns)))
            .filter(|r| self.labels.iter().all(|(k, v)| {
                r.spec.metadata().labels.as_ref()
                    .and_then(|l| l.get(*k))
                    .map(|val| val == *v)
                    .unwrap_or(false)
            }))
            .collect()
    }

    pub async fn one(self) -> Option<ResourceRecord> {
        self.collect().await.into_iter().next()
    }

    pub async fn count(self) -> usize {
        self.collect().await.len()
    }
}
```

### 4. Iterator-Heavy Reconciliation

```rust
// Before: imperative loops
let mut total = 0;
for record in &pods {
    if let AnyResource::Pod(p) = &record.spec {
        if record.assigned_node.is_some() { continue; }
        let node = match pick_least_loaded(&node_loads) {
            Some(n) => n.to_string(),
            None => continue,
        };
        match store.assign_node(&record.spec.uid(), &node).await {
            Ok(()) => {
                info!("Assigned {} -> {}", record.spec.name(), node);
                total += 1;
            }
            Err(e) => error!("Failed: {}", e),
        }
    }
}

// After: functional pipeline
let assigned: usize = pods.iter()
    .filter(|r| r.assigned_node.is_none())
    .filter_map(|r| {
        let node = pick_least_loaded(&node_loads)?;
        Some((r.spec.uid(), node))
    })
    .scan(&mut node_loads, |loads, (uid, node)| {
        increment_load(loads, &node);
        Some((uid, node))
    })
    .map(|(uid, node)| async {
        store.assign_node(&uid, &node).await
            .map(|_| info!("Assigned {} -> {}", uid, node))
            .unwrap_or_else(|e| error!("Failed {}: {}", uid, e));
    })
    // ... collect + await
    .count();
```

### 5. Functional Error Handling — Combinators Over Match

```rust
// Before
let mut result = Vec::new();
for rule in rules {
    match self.apply_rule(rule).await {
        Ok(()) => {}
        Err(e) => { warn!("Failed: {}: {}", rule.name, e); continue; }
    }
    result.push(rule);
}

// After: iterator with side effects
let applied: usize = rules.iter()
    .inspect(|r| debug!("Applying rule {}", r.name))
    .filter_map(|rule| self.apply_rule(rule).await.ok().map(|_| rule))
    .inspect(|r| info!("Applied rule {}", r.name))
    .count();
```

### 6. Declarative Network Rules — Builder Pattern

```rust
// Before: imperative nft setup
net.nft.add_snat("default", "10.0.0.0/8").await?;
net.nft.add_forward_allow("10.0.0.0/8", "0.0.0.0/0").await?;
net.nft.add_forward_deny("0.0.0.0/0", "10.0.0.0/8").await?;
net.nft.add_dnat(cluster_ip, port, &backends).await?;

// After: declarative rule builder
use network::rule::{RuleSet, Rule};

RuleSet::new("default-vnet")
    .allow("10.0.0.0/8", "0.0.0.0/0")     // pods → internet
    .deny("0.0.0.0/0", "10.0.0.0/8")       // internet → pods
    .snat("10.0.0.0/8")                      // masquerade
    .dnat(cluster_ip, port, &backends)        // service proxy
    .apply(&net.nft).await?;
```

### 7. Pipe-Style Data Flow

```rust
// core/types/helpers.rs

/// Chain operations on a resource.
pub trait Pipe {
    fn pipe<F, R>(self, f: F) -> R where F: FnOnce(Self) -> R, Self: Sized {
        f(self)
    }
}
impl<T> Pipe for T {}

// Usage: readable pipeline
let response = resource
    .pipe(|r| { validate(&r); r })
    .pipe(|r| { r.metadata_mut().uid.get_or_insert_with(gen_uid); r })
    .pipe(|r| { r.metadata_mut().creation_timestamp.get_or_insert_with(now); r })
    .pipe(|r| store.apply(r.into_any()));
```

### 8. Pattern Matching as Destructuring

```rust
// Instead of match arms, destructure in closures
let (pods, svcs): (Vec<_>, Vec<_>) = records.into_iter()
    .partition(|r| matches!(r.spec, AnyResource::Pod(_)));

// Or: group by kind
let grouped: HashMap<&str, Vec<&ResourceRecord>> = records.iter()
    .fold(HashMap::new(), |mut acc, r| {
        acc.entry(r.spec.kind()).or_default().push(r);
        acc
    });
```

### 9. Gossip as a Reducer

```rust
// sync/apply.rs

/// Apply incoming gossip entries as a fold over state.
pub fn reduce_gossip(
    state: &mut GossipState,
    entries: Vec<SyncEntry>,
) -> Vec<SyncEntry> {
    entries.into_iter()
        .filter(|e| state.dedup(&e.key, e.term))
        .collect()
}

// The entire gossip loop becomes:
loop {
    let incoming = receive_frame().await;
    let to_apply = reduce_gossip(&mut state, incoming);
    apply_batch(&store, &to_apply).await;
    broadcast_changes(&peers, &to_apply).await;
}
```

### 10. Composable Lifecycle Hooks

```rust
// Instead of one monolithic reconcile function, compose small functions

pub async fn reconcile_pod(ctx: &Ctx, pod: &Pod) -> Result<()> {
    ensure_namespace_exists(ctx, pod)?
        .pipe(|_| ensure_image_pulled(ctx, pod))?
        .pipe(|_| ensure_volume_mounted(ctx, pod))?
        .pipe(|_| ensure_container_running(ctx, pod))?
        .pipe(|_| ensure_health_probes_passing(ctx, pod))
}

fn ensure_namespace_exists(ctx: &Ctx, pod: &Pod) -> Result<()> {
    if pod.namespace().is_some() {
        let ns = Query::from(ctx.store.as_ref())
            .kind("Namespace")
            .one().await;
        if ns.is_none() {
            bail!("namespace {} not found", pod.namespace().unwrap());
        }
    }
    Ok(())
}

fn ensure_image_pulled(ctx: &Ctx, pod: &Pod) -> Result<()> {
    for container in pod.spec.containers.iter().filter(|c| !c.image.is_empty()) {
        ctx.runtime.pull_image(&container.image).await?;
    }
    Ok(())
}
```

---

## Multi-Node Sync Optimization

### Binary Protocol

```rust
// sync/wire.rs

pub const MAGIC: u32 = 0x5A_38_53_01; // "Z8S\x01"

#[derive(Serialize, Deserialize)]
pub enum Frame {
    Batch { entries: Vec<SyncEntry> },
    SyncSince { since: u64 },
    SyncResp { entries: Vec<SyncEntry> },
    Heartbeat { node: String, term: u64 },
    Ack { term: u64 },
}

#[derive(Serialize, Deserialize)]
pub struct SyncEntry {
    pub key: String,                    // resource UID
    pub record: ResourceRecord,         // full record (spec + status + metadata)
    pub term: u64,                      // logical timestamp
    pub clock: VectorClock,             // for conflict detection
}
```

Wire format: `[magic:4][version:1][type:1][len:4][bincode payload]`

### Vector Clocks

```rust
// sync/clock.rs

#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub struct VectorClock(HashMap<String, u64>);

impl VectorClock {
    pub fn increment(&mut self, node: &str) -> u64 {
        let t = self.0.entry(node.into()).or_insert(0);
        *t += 1;
        *t
    }

    pub fn merge(&mut self, other: &Self) {
        for (k, v) in &other.0 {
            let e = self.0.entry(k.into()).or_insert(0);
            *e = (*e).max(*v);
        }
    }

    pub fn happens_before(&self, other: &Self) -> bool {
        other.0.iter().all(|(k, v)| self.0.get(k).map_or(false, |s| s <= v))
    }

    pub fn conflicts(&self, other: &Self) -> bool {
        !self.happens_before(other) && !other.happens_before(self)
    }
}
```

### Merkle Anti-Entropy

```rust
// sync/merkle.rs

pub struct MerkleTree {
    root: [u8; 32],
    leaves: BTreeMap<String, [u8; 32]>,
}

impl MerkleTree {
    pub fn from_store(entries: &[(String, u64)]) -> Self { ... }

    /// Returns only the keys that differ — typically 0-5, not thousands.
    pub fn diff(&self, peer: &Self) -> Vec<String> {
        if self.root == peer.root { return vec![]; }
        self.diff_recurse(&self.root, &peer.root)
    }
}
```

Anti-entropy protocol:
```
1. Exchange root hashes
2. If equal → in sync (skip)
3. If different → recursive hash comparison (log2(N) steps)
4. Only exchange divergent keys
5. Apply with vector clock conflict resolution
```

### Incremental Sync

Instead of full dump on every connect:

```
1. Connect → send SyncSince { since: watermark.min_term() }
2. Peer responds with only changes since that term
3. Apply changes, update watermark
4. Send Ack { term: max_received_term }
```

---

## Wiring z8s-node

```rust
// z8s-node/src/run.rs — the ONLY place that knows about all packages

pub async fn run_node(port: u16, cfg: Config) -> Result<()> {
    // 1. Store
    let store = init_store(&cfg)?;

    // 2. Runtime
    let runtime = Runtime::new(&cfg)?;

    // 3. Network
    let net = Network::new(&cfg)?;

    // 4. Sync (gossip)
    let sync = SyncEngine::new(&cfg, &store);

    // 5. API
    let api = ApiServer::new(&store, &runtime, &net, &sync);

    // 6. Scheduler (reads store, assigns pods)
    let scheduler = Scheduler::new(&store, &sync);

    // 7. Run everything
    tokio::select! {
        _ = api.serve(port) => {}
        _ = scheduler.run() => {}
        _ = sync.run() => {}
        _ = shutdown_signal() => {}
    }

    // 8. Cleanup
    shutdown(store, runtime, net, sync).await
}

// z8s-node/src/main.rs — minimal entry point
fn main() -> Result<()> {
    let cfg = parse_args();           // --port, --peers, --data-dir, etc.
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_node(cfg.port, cfg))
}
```

## Wiring z8s CLI

```rust
// z8s/src/main.rs — process manager only, no async runtime

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("start")  => cmd_start(&args),
        Some("stop")   => cmd_stop(&args),
        Some("status") => cmd_status(),
        Some("reset")  => cmd_reset(),
        Some("node")   => cmd_node(&args),    // node start/stop/list/token
        Some("set")    => cmd_set(&args),     // set kubeconfig
        _              => print_help(),
    }
}

fn cmd_start(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);
    let port_lock = acquire_lock(&port_lock_path(port))?;
    let child = spawn_z8s_node(port, args)?;   // exec z8s-node --port {port}
    wait_for_ready(port, child.id());
    println!("z8s started on port {port}");
    Ok(())
}

fn cmd_stop(args: &[String]) -> Result<()> {
    let port = parse_port(args, 6443);
    let pid = read_lock_pid(&port_lock_path(port))?;
    send_signal(pid, SIGTERM);
    wait_for_exit(pid, Duration::from_secs(5));
    println!("z8s stopped.");
    Ok(())
}

fn spawn_z8s_node(port: u16, args: &[String]) -> Result<Child> {
    let self_path = std::env::current_exe()?;
    // Find z8s-node binary (same directory or PATH)
    let node_bin = find_node_binary()?;
    let log_path = format!("/tmp/z8s-{port}.log");
    let log = File::create(&log_path)?;
    Command::new(node_bin)
        .arg("--port").arg(port.to_string())
        .args(extra_args(args))
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
}
```

---

## Reconciler + Scheduler = One Module: `controller`

They're the same thing — a control loop that makes desired == current. The only difference is scope:

```
┌─────────────────────────────────────────────────────────┐
│                    controller/                            │
│                                                          │
│  ┌──────────────────────────────────────────────────┐    │
│  │  mod.rs — one control loop, two roles            │    │
│  │                                                   │    │
│  │  1. Assign role (leader only):                    │    │
│  │     - Read unassigned specs                       │    │
│  │     - Pick least-loaded node                      │    │
│  │     - Write assigned_node to spec                 │    │
│  │     - Increment generation                        │    │
│  │                                                   │    │
│  │  2. Reconcile role (every node):                  │    │
│  │     - Read specs where assigned_node == me        │    │
│  │     - Compare generation vs observed_generation   │    │
│  │     - Start/stop/update container                 │    │
│  │     - Write status back to DB                     │    │
│  └──────────────────────────────────────────────────┘    │
│                                                          │
│  One tick = assign (if leader) + reconcile (always)      │
└─────────────────────────────────────────────────────────┘
```

### Why merge them

| Before | After | Why |
|--------|-------|-----|
| `scheduler/scheduler.rs` (leader election + assignment) | `controller/mod.rs` (assign) | Same loop, same DB reads |
| `scheduler/orchestrator.rs` (reconcile loop) | `controller/mod.rs` (reconcile) | Same loop, same DB reads |
| `scheduler/process.rs` (ProcessTracker) | `controller/process.rs` | Process state belongs to the controller |
| `scheduler/assign.rs` (node selection) | `controller/assign.rs` | Assign logic belongs to controller |
| `scheduler/index.rs` (load index) | `controller/index.rs` | Load tracking belongs to controller |

### The single control loop

```rust
// controller/mod.rs

pub async fn run_controller(ctx: Arc<Ctx>) {
    let mut lease = acquire_leader_lease(&ctx.store).await;
    let mut index = NodeIndex::new();
    index.rebuild(&ctx.store).await;

    loop {
        // ── Phase 1: Assign (leader only) ──────────────────────
        if lease.is_leader() {
            let unassigned = ctx.store.get_unassigned("Pod").await;
            let node_loads = index.snapshot();

            for record in &unassigned {
                if let Some(node) = pick_least_loaded(&node_loads) {
                    ctx.store.assign_node(&record.spec.uid(), node).await.ok();
                    index.increment(node);
                    // Gossip the assignment to peers
                    if let Some(updated) = ctx.store.get(&record.spec.uid()).await {
                        ctx.sync.broadcast(&updated).await;
                    }
                }
            }

            // Reassign pods from dead nodes
            let dead = ctx.store.get_dead_nodes().await;
            for record in ctx.store.get_by_kind("Pod").await {
                if let Some(ref assigned) = record.assigned_node {
                    if dead.contains(assigned) {
                        if let Some(node) = pick_least_loaded(&node_loads) {
                            ctx.store.assign_node(&record.spec.uid(), node).await.ok();
                        }
                    }
                }
            }
        }

        // ── Phase 2: Reconcile (every node) ────────────────────
        // Only process records where generation > observed_generation AND assigned to me
        let needing = ctx.store.get_needing_reconcile(&ctx.node_name).await;
        for record in &needing {
            if let Err(e) = reconcile(ctx.clone(), record).await {
                tracing::error!("Reconcile failed for {}: {}", record.spec.uid(), e);
            }
        }

        // ── Lease renewal ──────────────────────────────────────
        if lease.needs_renewal() {
            lease = renew_or_reacquire(&ctx.store, &lease).await;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn reconcile(ctx: &Ctx, record: &ResourceRecord) -> Result<()> {
    match &record.spec {
        AnyResource::Pod(pod) => reconcile_pod(ctx, record, pod).await,
        AnyResource::Deployment(dep) => reconcile_deployment(ctx, record, dep).await,
        AnyResource::Service(svc) => reconcile_service(ctx, record, svc).await,
        _ => Ok(()),
    }
}

async fn reconcile_pod(ctx: &Ctx, record: &ResourceRecord, pod: &Pod) -> Result<()> {
    let desired_running = record.spec.metadata().deletion_timestamp.is_none();
    let current_running = record.status.phase == Phase::Running;

    if desired_running && !current_running {
        // Start container
        ctx.runtime.start_pod(&record.spec).await?;
        ctx.store.write_status(&record.spec.uid(), ResourceStatus {
            phase: Phase::Running,
            pod_ip: Some(ctx.net.allocate_ip()?.to_string()),
            started_at: Some(now()),
            ..Default::default()
        }).await?;
    } else if !desired_running && current_running {
        // Stop container
        ctx.runtime.stop_pod(&record.spec).await?;
        ctx.store.write_status(&record.spec.uid(), ResourceStatus {
            phase: Phase::Succeeded,
            finished_at: Some(now()),
            ..Default::default()
        }).await?;
    }

    ctx.store.set_observed_generation(&record.spec.uid(), record.generation).await?;
    Ok(())
}
```

### Final Package Structure (updated)

```
z8s/
├── Cargo.toml
│
├── core/                           # types, store, events, syscalls
│   ├── types/                      # all resource types
│   ├── syscall.rs                  # Direct Linux syscalls (mount, unshare, etc.)
│   ├── store.rs                    # StoreBackend trait
│   ├── redb.rs                     # RedbBackend
│   ├── hub.rs                      # StoreEventHub
│   ├── lease.rs                    # Leader election
│   └── token.rs                    # Join tokens
│
├── runtime/                        # container lifecycle
│   ├── mod.rs                      # RuntimeProvider trait
│   ├── spawn.rs                    # Fork, namespaces, pipes
│   ├── image.rs                    # OCI pull, layer cache
│   ├── rootfs.rs                   # chroot, mount
│   ├── cgroup.rs                   # cgroups v2
│   ├── exec.rs                     # kubectl exec
│   ├── health.rs                   # Probes
│   └── spec.rs                     # ContainerSpec builder
│
├── network/                        # veth, nftables, DNS, IPAM
│   ├── mod.rs                      # NetMux, NetworkEngine trait
│   ├── ipam.rs                     # IP pool, CIDR
│   ├── veth.rs                     # Veth pair create/delete
│   ├── nft.rs                      # nftables rules
│   ├── dns.rs                      # In-cluster DNS
│   ├── ingress.rs                  # Ingress HTTP listener
│   └── netlink.rs                  # Raw netlink ops
│
├── sync/                           # multi-node gossip
│   ├── mod.rs                      # GossipEngine
│   ├── wire.rs                     # Binary protocol
│   ├── transport.rs                # WebSocket client + server
│   ├── merkle.rs                   # Merkle tree anti-entropy
│   ├── clock.rs                    # Vector clock
│   └── apply.rs                    # Batch apply logic
│
├── controller/                     # THE CONTROL LOOP (was scheduler + reconciler)
│   ├── mod.rs                      # run_controller (assign + reconcile)
│   ├── assign.rs                   # Node selection (least-loaded)
│   ├── index.rs                    # In-memory node load index
│   └── process.rs                  # ProcessTracker (running containers)
│
├── api/                            # HTTP API (thin shell over store)
│   ├── mod.rs                      # AppState, router, TLS
│   ├── handler.rs                  # Generic CrudHandler<R>
│   ├── apply.rs                    # kubectl apply
│   ├── watch.rs                    # Watch support (subscribe to StoreEventHub)
│   ├── auth.rs                     # Token registry + RBAC
│   ├── proto.rs                    # Protobuf decoder
│   ├── table.rs                    # kubectl get table
│   └── catalog.rs                  # API resource catalog
│
├── z8s/                            # CLI binary (process manager)
│   ├── Cargo.toml                  # deps: std only
│   └── src/
│       ├── main.rs                 # dispatch
│       ├── spawn.rs                # spawn z8s-node
│       ├── status.rs               # show status
│       ├── stop.rs                 # send signals
│       ├── lock.rs                 # flock management
│       └── kubeconfig.rs           # write kubeconfig
│
└── z8s-node/                       # node binary (the complete node)
    ├── Cargo.toml                  # deps: core, runtime, network, api, sync, controller
    └── src/
        ├── main.rs                 # parse args
        ├── run.rs                  # wire everything
        ├── bootstrap.rs            # init store, TLS, defaults
        └── signal.rs               # graceful shutdown
```
z8s/
├── Cargo.toml
│
├── core/                           # types, store, events
│   ├── types/                      # all resource types
│   ├── store.rs                    # StoreBackend trait
│   ├── redb.rs                     # RedbBackend
│   ├── hub.rs                      # StoreEventHub
│   ├── lease.rs                    # Leader election
│   └── token.rs                    # Join tokens
│
├── runtime/                        # container lifecycle
│   ├── mod.rs                      # RuntimeProvider trait
│   ├── spawn.rs                    # Fork, namespaces, pipes
│   ├── image.rs                    # OCI pull, layer cache
│   ├── rootfs.rs                   # chroot, mount
│   ├── cgroup.rs                   # cgroups v2
│   ├── exec.rs                     # kubectl exec
│   ├── health.rs                   # Probes
│   └── spec.rs                     # ContainerSpec builder
│
├── network/                        # veth, nftables, DNS, IPAM
│   ├── mod.rs                      # NetMux, NetworkEngine trait
│   ├── ipam.rs                     # IP pool, CIDR
│   ├── veth.rs                     # Veth pair create/delete
│   ├── nft.rs                      # nftables rules
│   ├── dns.rs                      # In-cluster DNS
│   ├── ingress.rs                  # Ingress HTTP listener
│   └── netlink.rs                  # Raw netlink ops
│
├── sync/                           # multi-node gossip
│   ├── mod.rs                      # GossipEngine
│   ├── wire.rs                     # Binary protocol
│   ├── transport.rs                # WebSocket client + server
│   ├── merkle.rs                   # Merkle tree anti-entropy
│   ├── clock.rs                    # Vector clock
│   └── apply.rs                    # Batch apply logic
│
├── controller/                     # THE CONTROL LOOP (was scheduler + reconciler)
│   ├── mod.rs                      # run_controller (assign + reconcile)
│   ├── assign.rs                   # Node selection (least-loaded)
│   ├── index.rs                    # In-memory node load index
│   └── process.rs                  # ProcessTracker (running containers)
│
├── api/                            # HTTP API (thin shell over store)
│   ├── mod.rs                      # AppState, router, TLS
│   ├── handler.rs                  # Generic CrudHandler<R>
│   ├── apply.rs                    # kubectl apply
│   ├── watch.rs                    # Watch support (subscribe to StoreEventHub)
│   ├── auth.rs                     # Token registry + RBAC middleware
│   ├── proto.rs                    # Protobuf decoder
│   ├── table.rs                    # kubectl get table
│   └── catalog.rs                  # API resource catalog
│
├── z8s/                            # CLI binary (process manager)
│   ├── Cargo.toml                  # deps: std only
│   └── src/
│       ├── main.rs                 # dispatch
│       ├── spawn.rs                # spawn z8s-node
│       ├── status.rs               # show status
│       ├── stop.rs                 # send signals
│       ├── lock.rs                 # flock management
│       └── kubeconfig.rs           # write kubeconfig
│
└── z8s-node/                       # node binary (the complete node)
    ├── Cargo.toml                  # deps: core, runtime, network, api, sync, controller
    └── src/
        ├── main.rs                 # parse args
        ├── run.rs                  # wire everything
        ├── bootstrap.rs            # init store, TLS, defaults
        └── signal.rs               # graceful shutdown
```

### Dependency Graph (final)

```
core           (zero internal deps)
  ↑
runtime        (depends on: core)
network        (depends on: core)
sync           (depends on: core)
  ↑
controller     (depends on: core, runtime, network, sync)
  ↑
api            (depends on: core only — reads/writes store)
  ↑
z8s-node       (depends on: all above — wires them together)

z8s (CLI)      (standalone)
```

### Why API only talks to store

```
kubectl apply -f pod.yaml
        │
        ▼
┌───────────────┐
│   API Server  │  writes spec to store
│               │  does NOT touch runtime, network, or sync
└───────┬───────┘
        │ store event emitted
        ▼
┌───────────────┐
│  Controller   │  reads spec from store
│               │  talks to runtime, network, sync
│               │  writes status back to store
└───────────────┘
```

| Layer | Knows about | Does NOT know about |
|-------|-------------|---------------------|
| **API** | core (types + store) | runtime, network, sync, controller |
| **Controller** | core, runtime, network, sync | api |
| **Runtime** | core | api, controller, network, sync |
| **Network** | core | api, controller, runtime, sync |
| **Sync** | core | api, controller, runtime, network |

API is just a thin HTTP layer over the store. Controller is the brain that does all real work.

---

## 19. Remove `nix` Crate — Direct Kernel Syscalls via rustix

**Problem**: `nix` is a safe wrapper around Linux syscalls, but adds:
- 1 extra dependency (and its transitive deps)
- ~50μs overhead per call (wrapper + error conversion)
- Unsafe blocks still needed for most operations

**Fix**: Use `rustix` 1.1.4 for type-safe syscall wrappers. For syscalls not in rustix (`fork`), use inline assembly.

### Syscall Mapping

| Current (nix) | rustix 1.1.4 | Module |
|---------------|-------------|--------|
| `mount(...)` | `rustix::mount::mount(...)` | `rustix::mount` |
| `umount2(...)` | `rustix::mount::unmount(...)` | `rustix::mount` |
| `chroot(...)` | `rustix::process::chroot(...)` | `rustix::process` |
| `chdir(...)` | `rustix::process::chdir(...)` | `rustix::process` |
| `unshare(...)` | `rustix::thread::unshare_unsafe(...)` | `rustix::thread` |
| `sethostname(...)` | `rustix::system::sethostname(...)` | `rustix::system` |
| `setns(...)` | `rustix::thread::move_into_link_name_space(...)` | `rustix::thread` |
| `kill(pid, sig)` | `rustix::process::kill_process(...)` | `rustix::process` |
| `pipe(...)` | `rustix::pipe::pipe_with(...)` | `rustix::pipe` |
| `dup2(...)` | `rustix::io::dup2(...)` | `rustix::io` |
| `write(...)` | `rustix::io::write(...)` | `rustix::io` |
| `read(...)` | `rustix::io::read(...)` | `rustix::io` |
| `flock(...)` | `rustix::fs::flock(...)` | `rustix::fs` |
| `open(...)` | `rustix::fs::open(...)` | `rustix::fs` |
| `fork(...)` | **inline asm** (SYS_fork=57 on x86_64, 220 on aarch64) | `core/syscall.rs` |

### rustix Features Required

```toml
rustix = { version = "1.1.4", features = ["process", "fs", "mount", "thread", "pipe", "net", "system"] }
```

### Cargo.toml change

```toml
# Before
[dependencies]
nix = { version = "0.31", features = ["fs", "signal", "sched", "mount", "process", "term", "ioctl", "user", "resource", "hostname", "socket"] }

# After
[dependencies]
rustix = { version = "1.1.4", features = ["process", "fs", "mount", "thread", "pipe", "net", "system"] }
```

### Raw Syscall Helpers

```rust
// core/syscall.rs — thin wrappers, no unsafe in public API

use std::ffi::CString;
use std::os::unix::io::RawFd;

/// Mount a filesystem.
pub fn mount(
    source: Option<&str>,
    target: &str,
    fstype: Option<&str>,
    flags: u64,
    data: Option<&str>,
) -> Result<(), i32> {
    let src = source.map(|s| CString::new(s).unwrap());
    let tgt = CString::new(target).unwrap();
    let fs = fstype.map(|s| CString::new(s).unwrap());
    let dat = data.map(|s| CString::new(s).unwrap());

    let ret = unsafe {
        libc::mount(
            src.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null()),
            tgt.as_ptr(),
            fs.as_ref().map(|p| p.as_ptr()).unwrap_or(std::ptr::null()),
            flags,
            dat.as_ref().map(|p| p.as_ptr() as *const libc::c_void).unwrap_or(std::ptr::null()),
        )
    };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Unshare namespaces.
pub fn unshare(flags: u64) -> Result<(), i32> {
    let ret = unsafe { libc::unshare(flags as i32) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Set hostname.
pub fn sethostname(name: &str) -> Result<(), i32> {
    let c = CString::new(name).unwrap();
    let ret = unsafe { libc::sethostname(c.as_ptr(), c.as_bytes().len()) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Change root directory.
pub fn chroot(path: &str) -> Result<(), i32> {
    let c = CString::new(path).unwrap();
    let ret = unsafe { libc::chroot(c.as_ptr()) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Fork a process.
pub fn fork() -> Result<u32, i32> {
    let ret = unsafe { libc::fork() };
    if ret >= 0 { Ok(ret as u32) } else { Err(ret) }
}

/// Wait for child process.
pub fn waitpid(pid: i32, status: &mut i32, flags: i32) -> Result<i32, i32> {
    let ret = unsafe { libc::wait4(pid as i32, status, flags, std::ptr::null_mut()) };
    if ret >= 0 { Ok(ret) } else { Err(ret) }
}

/// Send signal to process.
pub fn kill(pid: i32, sig: i32) -> Result<(), i32> {
    let ret = unsafe { libc::kill(pid, sig) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Create a pipe.
pub fn pipe2(flags: i32) -> Result<(RawFd, RawFd), i32> {
    let mut fds = [0i32; 2];
    let ret = unsafe { libc::pipe2(fds.as_mut_ptr(), flags) };
    if ret == 0 { Ok((fds[0], fds[1])) } else { Err(ret) }
}

/// Create device node.
pub fn mknod(path: &str, mode: u32, dev: u64) -> Result<(), i32> {
    let c = CString::new(path).unwrap();
    let ret = unsafe { libc::mknod(c.as_ptr(), mode as libc::mode_t, dev) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Write to a file descriptor.
pub fn write(fd: RawFd, data: &[u8]) -> Result<usize, i32> {
    let ret = unsafe { libc::write(fd, data.as_ptr() as *const libc::c_void, data.len()) };
    if ret >= 0 { Ok(ret as usize) } else { Err(ret) }
}

/// Read from a file descriptor.
pub fn read(fd: RawFd, buf: &mut [u8]) -> Result<usize, i32> {
    let ret = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if ret >= 0 { Ok(ret as usize) } else { Err(ret) }
}

/// Close a file descriptor.
pub fn close(fd: RawFd) -> Result<(), i32> {
    let ret = unsafe { libc::close(fd) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Duplicate file descriptor.
pub fn dup2(old: RawFd, new: RawFd) -> Result<RawFd, i32> {
    let ret = unsafe { libc::dup2(old, new) };
    if ret >= 0 { Ok(ret) } else { Err(ret) }
}

/// Set file descriptor close-on-exec.
pub fn set_cloexec(fd: RawFd) -> Result<(), i32> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 { return Err(flags); }
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// File lock.
pub fn flock(fd: RawFd, operation: i32) -> Result<(), i32> {
    let ret = unsafe { libc::flock(fd, operation) };
    if ret == 0 { Ok(()) } else { Err(ret) }
}

/// Set user namespace mappings.
pub fn write_uid_map(pid: i32, map: &str) -> Result<(), i32> {
    let path = format!("/proc/{}/uid_map", pid);
    let fd = unsafe { libc::open(CString::new(&path).unwrap().as_ptr(), libc::O_WRONLY) };
    if fd < 0 { return Err(fd); }
    let ret = write(fd, map.as_bytes());
    unsafe { libc::close(fd); }
    ret.map(|_| ())
}

pub fn write_gid_map(pid: i32, map: &str) -> Result<(), i32> {
    let path = format!("/proc/{}/gid_map", pid);
    let fd = unsafe { libc::open(CString::new(&path).unwrap().as_ptr(), libc::O_WRONLY) };
    if fd < 0 { return Err(fd); }
    let ret = write(fd, map.as_bytes());
    unsafe { libc::close(fd); }
    ret.map(|_| ())
}

pub fn write_setgroups(pid: i32, value: &str) -> Result<(), i32> {
    let path = format!("/proc/{}/setgroups", pid);
    let fd = unsafe { libc::open(CString::new(&path).unwrap().as_ptr(), libc::O_WRONLY) };
    if fd < 0 { return Err(fd); }
    let ret = write(fd, value.as_bytes());
    unsafe { libc::close(fd); }
    ret.map(|_| ())
}
```

### What changes

| Before (nix) | After (direct) |
|--------------|----------------|
| `nix::mount::mount(...)` | `syscall::mount(...)` |
| `nix::sched::unshare(...)` | `syscall::unshare(...)` |
| `nix::unistd::chroot(...)` | `syscall::chroot(...)` |
| `nix::sys::signal::kill(...)` | `syscall::kill(...)` |
| `nix::sys::wait::waitpid(...)` | `syscall::waitpid(...)` |
| `nix::unistd::pipe()` | `syscall::pipe2(...)` |
| `nix::sys::stat::mknod(...)` | `syscall::mknod(...)` |

### Benefits

- **Zero dependencies** for syscalls (just `libc`)
- **No wrapper overhead** — direct kernel calls
- **Same safety** — we still use `unsafe` blocks, but the code is simpler
- **Smaller binary** — `nix` adds ~200KB to the binary
- **Faster startup** — no dynamic dispatch through nix wrappers

### Cargo.toml change

```toml
# Before
[dependencies]
nix = { version = "0.31", features = ["fs", "signal", "sched", "mount", "process", "term", "ioctl", "user", "resource", "hostname", "socket"] }

# After
[dependencies]
libc = "0.2"  # already a transitive dep of tokio/nix
```

---

## Implementation Order

| Step | What | LOC Moved | LOC Deleted |
|------|------|-----------|-------------|
| 1 | Create workspace, extract `core/` (types + store + events + syscalls) | ~4000 | ~200 |
| 2 | Create `core/syscall.rs` — replace nix with direct libc | ~300 | ~800 (nix calls) |
| 3 | Extract `runtime/` (cri + spawn) | ~3500 | ~100 |
| 4 | Extract `network/` (netmux + netlink) | ~2500 | ~300 |
| 5 | Extract `sync/` (gossip + anti-entropy) | ~800 | ~200 |
| 6 | Merge scheduler + reconciler into `controller/` | ~600 | ~400 |
| 7 | Extract `api/` (handlers + auth + catalog) | ~3000 | ~500 |
| 8 | Create `z8s/` CLI binary | ~300 | ~800 |
| 9 | Create `z8s-node/` binary | ~500 | ~300 |
| 10 | Add `Resource` trait, implement for all types | ~500 | ~1000 |
| 11 | Generic `Crud<R>` handler | ~200 | ~1500 |
| 12 | Binary gossip protocol + vector clocks | ~400 | ~300 |
| 13 | Merkle anti-entropy | ~300 | 0 |
| 14 | DB redesign: spec + status + generation | ~800 | ~200 |

**Net result**: ~17,700 LOC moved, ~6,800 LOC deleted. Final: ~20,000 LOC across 8 packages + 2 binaries. Zero `nix` dependency.

---

## Testing

```bash
cargo test --workspace                    # all crates
cargo test -p core                        # types + store
cargo test -p runtime                     # container lifecycle
cargo test -p network                     # IPAM, veth, nft
cargo test -p sync                        # protocol, merkle, vector clock
cargo test -p controller                  # assign, reconcile, index
cargo test -p api                         # handler tests
cargo test -p z8s                         # CLI
cargo clippy --workspace -- -D warnings   # lint
cargo fmt --check                         # format

# Integration: full lifecycle
cargo run -p z8s -- start --port 6443
kubectl apply -f pod.yaml
kubectl get pods          # should show Running
cargo run -p z8s -- stop
```

### New Design: Spec + Status + Generation

Every resource in the DB has three parts:

```
┌─────────────────────────────────────────────────────────┐
│                    ResourceRecord                         │
│                                                          │
│  ┌──────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │     Spec      │  │    Status    │  │   Metadata    │  │
│  │  (desired)    │  │  (current)   │  │  (identity)   │  │
│  │               │  │              │  │               │  │
│  │  containers   │  │  phase       │  │  uid, name    │  │
│  │  volumes      │  │  conditions  │  │  namespace    │  │
│  │  node (set    │  │  nodeIP      │  │  labels       │  │
│  │   by sched)   │  │  containerIP │  │  annotations  │  │
│  │  restartPolicy│  │  ready       │  │  creationTS   │  │
│  │  resources    │  │  message     │  │  generation   │  │
│  └──────────────┘  └──────────────┘  └───────────────┘  │
│                                                          │
│  generation: u64              // incremented on spec write│
│  observed_generation: u64     // what reconciler processed│
│  assigned_node: Option<String>// scheduler assignment     │
└─────────────────────────────────────────────────────────┘
```

### The Reconciliation Contract

```
User writes spec (kubectl apply)
        │
        ▼
┌───────────────┐
│   API Server  │  writes spec to DB, increments generation
└───────┬───────┘
        │
        ▼
┌───────────────┐
│   Scheduler   │  reads unassigned specs, assigns nodes
│   (leader)    │  writes assigned_node, increments generation
└───────┬───────┘
        │
        ▼
┌───────────────┐
│  Node Reconciler │  reads spec, compares generation vs observed
│  (each node)   │  if different → start/stop/update container
└───────┬───────┘       writes status (phase, conditions, IP)
        │                sets observed_generation = generation
        ▼
┌───────────────┐
│    DB         │  status is persisted — survives restart
└───────────────┘
```

### Rust Types

```rust
// core/types/resource.rs

/// The full resource record stored in DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRecord {
    /// The desired state (what the user wants).
    pub spec: AnyResource,
    /// The current state (what's actually running).
    pub status: ResourceStatus,
    /// Monotonically increasing — incremented on every spec change.
    pub generation: u64,
    /// What the reconciler has processed. If != generation → needs reconciliation.
    pub observed_generation: u64,
    /// Which node should run this resource. Set by scheduler.
    pub assigned_node: Option<String>,
    /// Last time this record was updated (epoch ms).
    pub last_updated: i64,
}

/// Current state of a resource — computed by reconciler, persisted to DB.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceStatus {
    /// Current phase.
    pub phase: Phase,
    /// Human-readable message (e.g. "Waiting for image pull").
    pub message: Option<String>,
    /// Conditions (like k8s conditions).
    pub conditions: Vec<Condition>,
    /// Pod IP (if running).
    pub pod_ip: Option<String>,
    /// Node IP (if assigned).
    pub host_ip: Option<String>,
    /// Container statuses.
    pub container_statuses: Vec<ContainerStatus>,
    /// Start time.
    pub started_at: Option<String>,
    /// Finish time (for succeeded/failed).
    pub finished_at: Option<String>,
    /// Restart count across all containers.
    pub restart_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum Phase {
    #[default]
    Pending,
    Running,
    Succeeded,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// Type: "Ready", "PodScheduled", "Initialized", "ContainersReady"
    #[serde(rename = "type")]
    pub type_: String,
    /// "True", "False", "Unknown"
    pub status: String,
    /// Why this condition changed: "PodScheduled", "ImagePullBackOff", etc.
    pub reason: Option<String>,
    /// Human-readable detail.
    pub message: Option<String>,
    /// When this condition last changed.
    pub last_transition_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContainerStatus {
    pub name: String,
    pub ready: bool,
    pub restart_count: u32,
    pub image: String,
    pub container_id: Option<String>,
    pub state: ContainerState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContainerState {
    pub waiting: Option<ContainerStateWaiting>,
    pub running: Option<ContainerStateRunning>,
    pub terminated: Option<ContainerStateTerminated>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStateWaiting {
    pub reason: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStateRunning {
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerStateTerminated {
    pub exit_code: i32,
    pub reason: String,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}
```

### DB Tables

```rust
// core/store.rs

// Main resource table — keyed by UID, stores full ResourceRecord
const RESOURCES: TableDefinition<&str, &[u8]> = TableDefinition::new("resources");

// Events table — keyed by "{resource_uid}/{event_id}", stores EventRecord
const EVENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("events");

// Scheduler lease (leader election)
const LEASES: TableDefinition<&str, &[u8]> = TableDefinition::new("leases");

// Node heartbeats
const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");

// Join tokens for cluster membership
const JOIN_TOKENS: TableDefinition<&str, &[u8]> = TableDefinition::new("join_tokens");
```

### Events System

Every action on a resource creates an event. Events are append-only, stored in the DB, and queryable.

```rust
// core/types/event.rs

/// A single event record — one thing that happened to a resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRecord {
    /// Unique event ID (monotonic per resource).
    pub event_id: u64,
    /// What happened: "Scheduled", "Pulled", "Created", "Started", "Killing", "Failed", etc.
    pub reason: String,
    /// Human-readable message: "Successfully assigned nginx to node-1"
    pub message: String,
    /// When this happened (epoch ms).
    pub timestamp: i64,
    /// Who/what caused this: "controller", "scheduler", "api", "kubelet"
    pub source: String,
    /// Resource kind: "Pod", "Service", "Deployment"
    pub resource_kind: String,
    /// Resource name: "nginx"
    pub resource_name: String,
    /// Resource namespace: "default"
    pub resource_namespace: Option<String>,
    /// Resource UID (links to ResourceRecord).
    pub resource_uid: String,
    /// Event type: "Normal" or "Warning"
    pub event_type: EventType,
    /// Count of repeated events (for dedup).
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub enum EventType {
    Normal,
    Warning,
}
```

### Event Creation API

```rust
// core/store/events.rs

impl dyn StoreBackend {
    /// Record an event for a resource. Appends to events table.
    pub async fn record_event(
        &self,
        resource: &AnyResource,
        reason: &str,
        message: &str,
        source: &str,
        event_type: EventType,
    ) -> Result<EventRecord> {
        let uid = resource.uid();
        let event_id = self.next_event_id(uid).await?;
        let event = EventRecord {
            event_id,
            reason: reason.to_string(),
            message: message.to_string(),
            timestamp: now_epoch_ms(),
            source: source.to_string(),
            resource_kind: resource.kind().to_string(),
            resource_name: resource.name().to_string(),
            resource_namespace: resource.namespace().map(|s| s.to_string()),
            resource_uid: uid,
            event_type,
            count: 1,
        };
        self.write_event(&event).await?;
        Ok(event)
    }

    /// Get all events for a resource (newest first).
    pub async fn get_events(&self, resource_uid: &str) -> Vec<EventRecord>;

    /// Get events for a resource kind in a namespace.
    pub async fn get_events_by_kind(
        &self,
        kind: &str,
        namespace: Option<&str>,
    ) -> Vec<EventRecord>;

    /// Get recent events across all resources.
    pub async fn get_recent_events(&self, limit: usize) -> Vec<EventRecord>;

    /// Prune old events (keep last N per resource).
    pub async fn prune_events(&self, keep_per_resource: usize) -> Result<usize>;
}
```

### Events are Created At Every Step

```
Pod lifecycle events:

1. API: "Scheduled"      "Successfully assigned nginx to node-1"
2. API: "Pulling"        "Pulling image nginx:latest"
3. API: "Pulled"         "Successfully pulled image nginx:latest"
4. API: "Created"        "Created container nginx"
5. API: "Started"        "Started container nginx"
6. API: "Killing"        "Stopping container nginx"        (on delete)
7. API: "Deleted"        "Removed finalizer"               (on delete)

Scheduler events:

1. "Scheduled"           "Successfully assigned pod-X to node-1"
2. "Preempted"           "pod-X preempted pod-Y"           (future)
3. "FailedScheduling"    "0/3 nodes are available"         (no capacity)

Controller events:

1. "Reconciling"         "Reconciling pod spec generation 5"
2. "ScalingUp"           "Scaling deployment nginx from 2 to 5"
3. "ScalingDown"         "Scaling deployment nginx from 5 to 2"

Network events:

1. "AllocatedIP"         "Allocated IP 10.42.0.5 for pod nginx"
2. "CreatedVeth"         "Created veth pair veth-abc12345"
3. "ServiceUpdate"       "Updated service nginx endpoints"
```

### How kubectl get events Shows Them

```
$ kubectl get events
LAST SEEN   TYPE      REASON      OBJECT      MESSAGE
0s          Normal    Scheduled   pod/nginx   Successfully assigned nginx to node-1
0s          Normal    Pulled      pod/nginx   Image pulled successfully
0s          Normal    Created     pod/nginx   Container created
0s          Normal    Started     pod/nginx   Container started

$ kubectl get events --field-selector involvedObject.name=nginx
LAST SEEN   TYPE      REASON      MESSAGE
0s          Normal    Scheduled   Successfully assigned nginx to node-1
0s          Normal    Pulled      Image pulled successfully
0s          Normal    Created     Container created
0s          Normal    Started     Container started
```

### Controller Records Events

```rust
// controller/reconcile.rs — updated to record events

async fn reconcile_pod(ctx: &Ctx, record: &ResourceRecord, pod: &Pod) -> Result<()> {
    let uid = record.spec.uid();

    // Record event on assignment
    if let Some(ref node) = record.assigned_node {
        ctx.store.record_event(
            &record.spec,
            "Scheduled",
            &format!("Successfully assigned {} to {}", record.spec.name(), node),
            "scheduler",
            EventType::Normal,
        ).await?;
    }

    // Record event on image pull
    for container in &pod.spec.containers {
        if !container.image.is_empty() {
            ctx.store.record_event(
                &record.spec,
                "Pulling",
                &format!("Pulling image {}", container.image),
                "controller",
                EventType::Normal,
            ).await?;

            match ctx.runtime.pull_image(&container.image).await {
                Ok(()) => {
                    ctx.store.record_event(
                        &record.spec,
                        "Pulled",
                        &format!("Successfully pulled image {}", container.image),
                        "controller",
                        EventType::Normal,
                    ).await?;
                }
                Err(e) => {
                    ctx.store.record_event(
                        &record.spec,
                        "Failed",
                        &format!("Failed to pull image {}: {}", container.image, e),
                        "controller",
                        EventType::Warning,
                    ).await?;
                    return Err(e);
                }
            }
        }
    }

    // Record event on container start
    ctx.runtime.start_pod(&record.spec).await?;
    ctx.store.record_event(
        &record.spec,
        "Started",
        &format!("Started container {}", pod.spec.containers[0].name),
        "controller",
        EventType::Normal,
    ).await?;

    // Update status
    ctx.store.write_status(&uid, ResourceStatus {
        phase: Phase::Running,
        pod_ip: Some(ctx.net.allocate_ip()?.to_string()),
        started_at: Some(now()),
        ..Default::default()
    }).await?;

    ctx.store.set_observed_generation(&uid, record.generation).await?;
    Ok(())
}
```

### Migration from Current Schema

```rust
// core/store/migration.rs

/// Migrate from old schema (AnyResource + ResourceState) to new (ResourceRecord).
/// Called once on first startup after upgrade.
pub fn migrate_v1_to_v2(db: &Database) -> Result<()> {
    let read_txn = db.begin_read()?;
    let old_table = read_txn.open_table(OLD_RESOURCES)?;

    let mut records = Vec::new();
    for entry in old_table.iter() {
        let (key, value) = entry?;
        let resource: AnyResource = serde_json::from_slice(value.value())?;
        let state: ResourceState = /* read from in-memory HashMap or default */;

        // Convert old ResourceState to new ResourceStatus
        let status = match state {
            ResourceState::Pending => ResourceStatus { phase: Phase::Pending, ..Default::default() },
            ResourceState::Running => ResourceStatus { phase: Phase::Running, ..Default::default() },
            ResourceState::Succeeded => ResourceStatus { phase: Phase::Succeeded, ..Default::default() },
            ResourceState::Failed(msg) => ResourceStatus { phase: Phase::Failed, message: Some(msg), ..Default::default() },
            ResourceState::Terminated => ResourceStatus { phase: Phase::Succeeded, ..Default::default() },
        };

        records.push(ResourceRecord {
            spec: resource.clone(),
            status,
            generation: 1,
            observed_generation: 0,  // force reconcile on first tick
            assigned_node: /* extract from resource if exists */,
            last_updated: now_epoch_ms(),
        });
    }
    drop(read_txn);

    // Write new records
    let write_txn = db.begin_write()?;
    let mut table = write_txn.open_table(NEW_RESOURCES)?;
    for record in &records {
        let key = record.spec.uid();
        let bytes = serde_json::to_vec(record)?;
        table.insert(key.as_str(), bytes.as_slice())?;
    }
    write_txn.commit()?;

    info!("Migrated {} resources from v1 to v2 schema", records.len());
    Ok(())
}
```

### Reconciler Logic (complete)

```rust
// controller/reconcile.rs

async fn reconcile(ctx: &Ctx, record: &ResourceRecord) -> Result<()> {
    match &record.spec {
        AnyResource::Pod(pod) => reconcile_pod(ctx, record, pod).await,
        AnyResource::Deployment(dep) => reconcile_deployment(ctx, record, dep).await,
        AnyResource::Service(svc) => reconcile_service(ctx, record, svc).await,
        _ => Ok(()),
    }
}

async fn reconcile_pod(ctx: &Ctx, record: &ResourceRecord, pod: &Pod) -> Result<()> {
    // 1. Skip if up-to-date
    if record.generation == record.observed_generation {
        return Ok(());
    }

    // 2. Skip if not assigned to this node
    if record.assigned_node.as_deref() != Some(&ctx.node_name) {
        return Ok(());
    }

    let uid = record.spec.uid();
    let desired_running = record.spec.metadata().deletion_timestamp.is_none();
    let current_phase = &record.status.phase;

    // 3. Determine what changed
    let spec_changed = record.generation > record.observed_generation;
    let needs_start = desired_running && matches!(current_phase, Phase::Pending | Phase::Unknown);
    let needs_stop = !desired_running && matches!(current_phase, Phase::Running);
    let needs_restart = desired_running && matches!(current_phase, Phase::Running)
        && spec_changed && image_changed(record);

    // 4. Execute reconciliation
    if needs_restart {
        // Stop then start (image or config changed)
        ctx.runtime.stop_pod(&record.spec).await.ok();
        update_status(ctx, &uid, ResourceStatus {
            phase: Phase::Pending,
            message: Some("Restarting due to spec change".into()),
            ..Default::default()
        }).await;
        ctx.runtime.start_pod(&record.spec).await?;
        update_status(ctx, &uid, ResourceStatus {
            phase: Phase::Running,
            started_at: Some(now()),
            ..Default::default()
        }).await;
    } else if needs_start {
        // Pull image first
        update_status(ctx, &uid, ResourceStatus {
            phase: Phase::Pending,
            message: Some("Pulling image".into()),
            ..Default::default()
        }).await;
        for container in &pod.spec.containers {
            if !container.image.is_empty() {
                ctx.runtime.pull_image(&container.image).await?;
            }
        }
        // Start container
        ctx.runtime.start_pod(&record.spec).await?;
        update_status(ctx, &uid, ResourceStatus {
            phase: Phase::Running,
            pod_ip: Some(ctx.net.allocate_ip()?.to_string()),
            started_at: Some(now()),
            conditions: vec![Condition {
                type_: "Ready".into(),
                status: "True".into(),
                reason: Some("ContainersReady".into()),
                message: Some("All containers ready".into()),
                last_transition_time: Some(now()),
            }],
            ..Default::default()
        }).await;
    } else if needs_stop {
        ctx.runtime.stop_pod(&record.spec).await?;
        update_status(ctx, &uid, ResourceStatus {
            phase: Phase::Succeeded,
            finished_at: Some(now()),
            ..Default::default()
        }).await;
    }

    // 5. Mark observed
    ctx.store.set_observed_generation(&uid, record.generation).await?;
    Ok(())
}

fn image_changed(record: &ResourceRecord) -> bool {
    // Compare current status container images with spec containers
    // If any image changed → needs restart
    match &record.spec {
        AnyResource::Pod(pod) => {
            pod.spec.containers.iter().any(|c| {
                record.status.container_statuses.iter()
                    .find(|s| s.name == c.name)
                    .map(|s| s.image != c.image)
                    .unwrap_or(true)
            })
        }
        _ => false,
    }
}

async fn update_status(ctx: &Ctx, uid: &str, status: ResourceStatus) {
    ctx.store.write_status(uid, status).await.ok();
    // Gossip status to peers
    if let Some(record) = ctx.store.get(uid).await {
        ctx.sync.broadcast(&record).await;
    }
}
```

### How kubectl get pod Shows Status

```
$ kubectl get pods
NAME    READY   STATUS    RESTARTS   AGE
nginx   1/1     Running   0          5m

$ kubectl describe pod nginx
Name:           nginx
Node:           node-1
Status:         Running
IP:             10.42.0.5
Conditions:
  Type            Status  Reason
  Ready           True
  PodScheduled    True
  ContainersReady True
Containers:
  web:
    Image:      nginx:latest
    Ready:      True
    State:      Running
      Started:  2026-06-11T10:00:00Z
Events:
  Normal  Scheduled   Successfully assigned nginx to node-1
  Normal  Pulled      Image pulled successfully
  Normal  Created     Container created
  Normal  Started     Container started
```

### Flow: Pod Creation to Running

```
1. kubectl apply -f pod.yaml
   → API writes spec to DB, generation=1, status=Pending

2. Scheduler picks node-1
   → DB: assigned_node="node-1", generation=2

3. Node-1 reconciler sees generation=2, observed=1
   → Pulls image
   → Writes status: phase=Pending, message="Pulling nginx:latest"

4. Image pulled
   → Writes status: phase=Pending, message="Creating container"

5. Container started
   → Writes status: phase=Running, pod_ip=10.42.0.5, conditions=[Ready=True]
   → Sets observed_generation=2

6. kubectl get pods shows: nginx  1/1  Running  0  1m
```

---

## Testing

```bash
cargo test --workspace                    # all crates
cargo test -p core                        # types + store
cargo test -p runtime                     # container lifecycle
cargo test -p network                     # IPAM, veth, nft
cargo test -p sync                        # protocol, merkle, vector clock
cargo test -p api                         # handler tests
cargo test -p z8s                         # CLI (lock files, spawn, status)
cargo clippy --workspace -- -D warnings   # lint
cargo fmt --check                         # format

# Integration: start a node, verify it's running, stop it
cargo run -p z8s -- start --port 6443
cargo run -p z8s -- status
cargo run -p z8s -- stop --port 6443
```
