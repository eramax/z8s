# Refactoring Plan: Pluggable Component Architecture

> **Date:** 2026-05-28
> **Goal:** Restructure z8s into a pluggable, pipeline-driven component architecture that supports compute (Pod, Deployment, Job), network (Service, VNet, NSG, NetworkPolicy), and storage (PV, PVC, ConfigMap, Secret) resource domains with zero code duplication and easy extensibility.
> **Constraint:** Pure structural refactoring — no behavioral changes. Everything that works today must work after.

---

## 1. Current Architecture (Problems)

```
src/
  main.rs                 184 lines   — wires everything, manageable
  config.rs               184 lines   — CLI flags, fine
  init.rs                  83 lines   — PID 1 signal handler, fine
  controller.rs           177 lines   — Deployment-only, no shared interface
  api/
    types.rs              464 lines   — AnyResource enum + ResourceStore + helpers
  container/
    image.rs              296 lines   — OCI pull/unpack
    oci_config.rs         146 lines   — entrypoint/cmd resolution
    rootfs.rs           1,017 lines   — mount ns, pivot_root, chroot
    volumes.rs            430 lines   — volume bind-mounts
  network/
    mod.rs                372 lines   — NetworkManager (service proxy mgmt)
    dns.rs                369 lines   — in-cluster DNS
    port_publish.rs       207 lines   — setns port forwarding
    service_proxy.rs      340 lines   — TCP proxy for ClusterIP
  server/
    api.rs              ~1,800 lines  — ALL REST handlers in one file
    exec.rs               773 lines   — kubectl exec WebSocket
    proto.rs              316 lines   — k8s protobuf decoder
  supervisor/
    process.rs          1,513 lines   — GOD OBJECT: pods, containers, network, env, cgroups
    cgroup.rs             133 lines   — cgroup v2
    health.rs             140 lines   — health probes
  manifest/
    watcher.rs            187 lines   — file watcher, fine
```

### 1.1 Problems

| Problem | Impact |
|---|---|
| `supervisor/process.rs` is a 1,500-line god object | Knows about pods, containers, networking, services, env vars, ConfigMaps, Secrets, cgroups, health probes, zombie reaping. Impossible to add new resource types without modifying it. |
| `controller.rs` is hardcoded to Deployment | No pattern for adding VNet, NSG, Job. Each new controller = standalone file with no shared interface. |
| `server/api.rs` is 1,800 lines | All REST handlers for all resource types crammed into one file. |
| No resource categorization | Pod and VNet are fundamentally different domains but share no abstraction. Adding NetMux components requires touching many files. |
| Tight coupling | `ProcessSupervisor` holds `Mutex<Option<Arc<NetworkManager>>>` back-reference → circular dependency between compute and network. |
| No pipeline/plug-in mechanism | Adding NSG to the Pod creation flow requires modifying PodComponent's code directly. |

### 1.2 Design Goals

1. **Components are standalone and pluggable** — adding VNet/NSG = one file + one registration line
2. **Generics and code reuse** — Pod and Job share 95% of logic via composition
3. **Pipeline-driven flow** — NSG, NetworkPolicy are pipeline stages, not hardcoded calls
4. **Sub-directory grouping** — `components/compute/`, `components/network/`, `components/storage/`
5. **Ready for NetMux** — the structure accommodates nftables engine, VNet CRDs, IPAM pool allocator
6. **Zero behavioral changes** — this is pure restructuring

---

## 2. Target Architecture

```
src/
  main.rs                          # Entry point (slimmed)
  config.rs                        # CLI config (unchanged)
  init.rs                          # PID 1 handler (unchanged)

  api/                             # API layer
    mod.rs                         # Re-exports
    types.rs                       # AnyResource, ResourceState, ResourceTracker, helpers
    store.rs                       # ResourceStore (extracted from types.rs)
    server.rs                      # Axum router setup, AppState, shared helpers
    handlers/                      # Per-resource HTTP handlers (split from server/api.rs)
      mod.rs
      pod.rs
      deployment.rs
      service.rs
      namespace.rs
      configmap.rs
      secret.rs
      storage.rs                   # PV + PVC
      discovery.rs                 # Endpoints, EndpointSlices
      metrics.rs                   # top pods/nodes
      system.rs                    # healthz, readyz, version, API discovery
    proto.rs                       # k8s protobuf decoder (unchanged)
    exec.rs                        # kubectl exec WebSocket (unchanged)

  components/                      # Pluggable resource controllers
    mod.rs                         # Component trait, PipelineStage, ReconciliationPipeline, builder
    compute/                       # Compute resources (Pod, Deployment, Job)
      mod.rs                       # ComputeResource trait, ComputeLifecycle shared impl
      pod.rs                       # PodComponent
      deployment.rs                # DeploymentComponent
      # job.rs                     # (future)
    network/                       # Network resources (Service, VNet, NSG, NetworkPolicy)
      mod.rs                       # NetworkResource trait
      service.rs                   # ServiceComponent (ClusterIP, NodePort, endpoints)
      # vnet.rs                    # (future — NetMux Phase 3)
      # nsg.rs                     # (future — NetMux Phase 3)
      # network_policy.rs          # (future — NetMux Phase 4)
    storage/                       # Storage resources (ConfigMap, Secret, PV, PVC)
      mod.rs                       # StorageResource trait
      configmap.rs                 # ConfigMapComponent
      secret.rs                    # SecretComponent
      storage.rs                   # PVComponent, PVCComponent

  cri/                             # Container Runtime Interface (renamed from container/)
    mod.rs
    spec.rs                         # ContainerSpec, ContainerConfig, builders
    runtime.rs                      # ContainerRuntime: spawn, stop, exec
    image.rs                        # OCI image pull + unpack
    rootfs.rs                       # Rootfs setup, pivot_root, chroot, mount ns
    oci.rs                          # OCI config resolution (renamed from oci_config.rs)
    volumes.rs                      # Volume management
    cgroup.rs                       # cgroup v2 management
    health.rs                       # Health check probes

  net/                             # Network engine (renamed from network/)
    mod.rs                         # NetworkEngine trait, current impl
    dns.rs                         # In-cluster DNS server
    service_proxy.rs               # TCP proxy (current, to be replaced by NetMux)
    port_publish.rs                # Port forwarding (current, to be replaced by NetMux)
    # pool.rs                      # (future — NetMux Phase 2) IP pool allocator
    # netmux.rs                    # (future — NetMux Phase 2) nftables engine
    # veth.rs                      # (future — NetMux Phase 1) veth pair management
    # route.rs                     # (future — NetMux Phase 1) host route management

  scheduler/                       # Scheduling and reconciliation
    mod.rs                         # Reconciler: drives component reconcile loops
    process.rs                     # RunningContainer tracking, zombie reaping

  manifest/                        # Manifest watcher (unchanged)
    mod.rs
    watcher.rs
```

---

## 3. Core Abstractions

### 3.1 Component Trait

Every resource type implements `Component`. This is the top-level controller for a resource kind.

```rust
// components/mod.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCategory {
    Compute,
    Network,
    Storage,
}

#[async_trait]
pub trait Component: Send + Sync + 'static {
    fn kind(&self) -> &'static str;
    fn category(&self) -> ResourceCategory;

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()>;
    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()>;
    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()>;
}

pub struct ReconcileContext {
    pub store: Arc<ResourceStore>,
    pub pipeline: Arc<ReconciliationPipeline>,
    pub cri: Arc<cri::ContainerRuntime>,
    pub net: Arc<dyn net::NetworkEngine>,
}
```

### 3.2 Component Registry

```rust
// components/mod.rs

pub struct ComponentRegistry {
    components: HashMap<&'static str, Box<dyn Component>>,
}

impl ComponentRegistry {
    pub fn new() -> Self { Self { components: HashMap::new() } }

    pub fn register(&mut self, component: Box<dyn Component>) {
        self.components.insert(component.kind(), component);
    }

    pub fn get(&self, kind: &str) -> Option<&dyn Component> {
        self.components.get(kind).map(|c| c.as_ref())
    }

    pub fn by_category(&self, cat: ResourceCategory) -> Vec<&dyn Component> {
        self.components.values()
            .filter(|c| c.category() == cat)
            .map(|c| c.as_ref())
            .collect()
    }

    pub async fn reconcile_all(&self, ctx: &ReconcileContext) {
        let trackers = ctx.store.get_all().await;
        for tracker in &trackers {
            if let Some(component) = self.get(tracker.resource.kind()) {
                if let Err(e) = component.reconcile(ctx, tracker).await {
                    tracing::error!(
                        "Reconcile failed for {}: {}",
                        tracker.resource.uid(), e
                    );
                }
            }
        }
    }
}
```

### 3.3 Pipeline Stage Trait

This is the key innovation. Each stage is independently pluggable and reacts to resource events.

```rust
// components/mod.rs

#[derive(Debug, Clone)]
pub enum ResourceEvent {
    Created(AnyResource),
    Updated(AnyResource),
    Deleted {
        kind: String,
        namespace: String,
        name: String,
        uid: String,
    },
}

impl ResourceEvent {
    pub fn kind(&self) -> &str {
        match self {
            ResourceEvent::Created(r) | ResourceEvent::Updated(r) => r.kind(),
            ResourceEvent::Deleted { kind, .. } => kind,
        }
    }
}

#[async_trait]
pub trait PipelineStage: Send + Sync {
    fn name(&self) -> &str;

    /// Which resource kinds does this stage care about?
    fn interests(&self) -> &[&str];

    async fn on_created(&self, resource: &AnyResource, ctx: &StageContext) -> Result<()>;
    async fn on_deleted(&self, kind: &str, uid: &str, ctx: &StageContext) -> Result<()>;
}

pub struct StageContext {
    pub store: Arc<ResourceStore>,
    pub cri: Arc<cri::ContainerRuntime>,
    pub net: Arc<dyn net::NetworkEngine>,
}
```

### 3.4 Reconciliation Pipeline + Builder

```rust
// components/mod.rs

pub struct ReconciliationPipeline {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl ReconciliationPipeline {
    pub fn builder() -> PipelineBuilder {
        PipelineBuilder::new()
    }

    pub async fn dispatch_created(&self, resource: &AnyResource, ctx: &StageContext) {
        let kind = resource.kind();
        for stage in &self.stages {
            if !stage.interests().contains(&kind) {
                continue;
            }
            if let Err(e) = stage.on_created(resource, ctx).await {
                tracing::error!("Pipeline stage '{}' failed on {} create: {}",
                    stage.name(), kind, e);
            }
        }
    }

    pub async fn dispatch_deleted(&self, kind: &str, uid: &str, ctx: &StageContext) {
        for stage in &self.stages {
            if !stage.interests().contains(&kind) {
                continue;
            }
            if let Err(e) = stage.on_deleted(kind, uid, ctx).await {
                tracing::error!("Pipeline stage '{}' failed on {} delete: {}",
                    stage.name(), kind, e);
            }
        }
    }
}

pub struct PipelineBuilder {
    stages: Vec<Box<dyn PipelineStage>>,
}

impl PipelineBuilder {
    pub fn new() -> Self { Self { stages: vec![] } }

    pub fn stage(mut self, stage: Box<dyn PipelineStage>) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn build(self) -> ReconciliationPipeline {
        ReconciliationPipeline { stages: self.stages }
    }
}
```

---

## 4. Compute Domain — Shared Code (Pod / Job / Deployment)

### 4.1 ComputeResource Trait

Pod and Job both use `PodSpec`. They share: image pull, rootfs, container spawn, network attachment, IP allocation, DNS, health probes. The differences are restart policy and IP lifecycle.

```rust
// components/compute/mod.rs

pub trait ComputeResource {
    fn pod_spec(&self) -> Option<&PodSpec>;
    fn restart_policy(&self) -> &str;
    fn is_ephemeral(&self) -> bool;  // Job=true (release IP on completion), Pod=false
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
    fn labels(&self) -> &BTreeMap<String, String>;
    fn uid(&self) -> &str;
    fn resource_ref(&self) -> &AnyResource;
}
```

### 4.2 ComputeLifecycle — Shared Implementation

```rust
// components/compute/mod.rs

pub struct ComputeLifecycle {
    cri: Arc<ContainerRuntime>,
    process_tracker: Arc<ProcessTracker>,
}

impl ComputeLifecycle {
    /// Build a `ContainerSpec` via the builder, then hand to CRI.
    /// See section 7 for the full builder-based implementation.
    pub async fn start(
        &self,
        res: &dyn ComputeResource,
        ctx: &ReconcileContext,
    ) -> Result<()> {
        // Full implementation in section 7.5 — uses ContainerSpecBuilder
        // + resolve_env, resolve_volumes, resolve_resource_limits,
        //   resolve_security, resolve_network, resolve_probes
        // + ctx.cri.start_pod(&spec).await
        // + ctx.pipeline.dispatch_created(...)
        Ok(())
    }

    pub async fn stop(
        &self,
        res: &dyn ComputeResource,
        ctx: &ReconcileContext,
    ) -> Result<()> {
        // Build a minimal spec from the resource, hand to CRI
        // + ctx.cri.stop_pod(&spec).await
        // + ctx.pipeline.dispatch_deleted(...)
        Ok(())
    }

    pub async fn handle_exit(
        &self,
        res: &dyn ComputeResource,
        exit_code: i32,
        ctx: &ReconcileContext,
    ) -> Result<()> {
        let should_restart = match res.restart_policy() {
            "Always" => true,
            "OnFailure" => exit_code != 0,
            "Never" => false,
            _ => true,
        };

        if res.is_ephemeral() && exit_code == 0 {
            // Job completed — set Succeeded, release IP
            ctx.store.update_state(res.uid(), ResourceState::Succeeded).await;
        } else if should_restart {
            // Exponential backoff, re-queue as Pending
            ctx.store.update_state(res.uid(), ResourceState::Pending).await;
        } else {
            let state = if exit_code == 0 {
                ResourceState::Succeeded
            } else {
                ResourceState::Failed(format!("exit code {}", exit_code))
            };
            ctx.store.update_state(res.uid(), state).await;
        }

        Ok(())
    }
}
```

### 4.3 Pod Component

Thin wrapper — delegates to shared `ComputeLifecycle`:

```rust
// components/compute/pod.rs

pub struct PodComponent {
    lifecycle: ComputeLifecycle,
}

impl PodComponent {
    pub fn new(lifecycle: ComputeLifecycle) -> Self {
        Self { lifecycle }
    }
}

struct PodAdapter<'a>(&'a AnyResource);

impl ComputeResource for PodAdapter<'_> {
    fn pod_spec(&self) -> Option<&PodSpec> {
        match self.0 {
            AnyResource::Pod(p) => p.spec.as_ref(),
            _ => None,
        }
    }
    fn restart_policy(&self) -> &str {
        self.pod_spec()
            .and_then(|s| s.restart_policy.as_deref())
            .unwrap_or("Always")
    }
    fn is_ephemeral(&self) -> bool { false }
    fn namespace(&self) -> &str { self.0.namespace() }
    fn name(&self) -> &str { self.0.name() }
    fn labels(&self) -> &BTreeMap<String, String> {
        static EMPTY: BTreeMap<String, String> = BTreeMap::new();
        match self.0 {
            AnyResource::Pod(p) => p.metadata.labels.as_ref().unwrap_or(&EMPTY),
            _ => &EMPTY,
        }
    }
    fn uid(&self) -> &str { &self.0.uid() }  // Note: returns &str to owned string
    fn resource_ref(&self) -> &AnyResource { self.0 }
}

#[async_trait]
impl Component for PodComponent {
    fn kind(&self) -> &'static str { "Pod" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Compute }

    async fn reconcile(&self, ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        if tracker.state == ResourceState::Pending {
            let adapter = PodAdapter(&tracker.resource);
            self.lifecycle.start(&adapter, ctx).await?;
        }
        Ok(())
    }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let adapter = PodAdapter(resource);
        self.lifecycle.start(&adapter, ctx).await
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let adapter = PodAdapter(resource);
        self.lifecycle.stop(&adapter, ctx).await
    }
}
```

### 4.4 Job Component (Future) — Zero Duplication

```rust
// components/compute/job.rs (future)

pub struct JobComponent {
    lifecycle: ComputeLifecycle,  // same shared instance
}

struct JobAdapter<'a>(&'a AnyResource);

impl ComputeResource for JobAdapter<'_> {
    fn restart_policy(&self) -> &str { "Never" }
    fn is_ephemeral(&self) -> bool { true }  // releases IP on completion
    // ... rest is identical to PodAdapter
}
```

### 4.5 Deployment Component

```rust
// components/compute/deployment.rs

pub struct DeploymentComponent {
    store: Arc<ResourceStore>,
    lifecycle: Arc<ComputeLifecycle>,
    ctx: Arc<ReconcileContext>,
}

#[async_trait]
impl Component for DeploymentComponent {
    fn kind(&self) -> &'static str { "Deployment" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Compute }

    async fn reconcile(&self, _ctx: &ReconcileContext, tracker: &ResourceTracker) -> Result<()> {
        let AnyResource::Deployment(deploy) = &tracker.resource else { return Ok(()) };
        let spec = deploy.spec.as_ref().context("no spec")?;
        let name = deploy.metadata.name.as_deref().unwrap_or("unknown");
        let namespace = deploy.metadata.namespace.as_deref().unwrap_or("default");
        let replicas = spec.replicas.unwrap_or(1) as usize;
        let match_labels = spec.selector.match_labels.clone().unwrap_or_default();

        // Count matching pods
        let pods = self.store.get_by_kind("Pod").await;
        let matching: Vec<_> = pods.iter()
            .filter(|t| {
                if let AnyResource::Pod(pod) = &t.resource {
                    pod.metadata.namespace.as_deref() == Some(namespace)
                        && pod_owned_by_deployment(pod, name)
                        && labels_match(&match_labels, &pod.metadata.labels.clone().unwrap_or_default())
                } else { false }
            })
            .collect();

        // Scale up
        while matching.len() < replicas {
            let pod = create_pod_from_template(deploy, &format!("{}-pod-{}", name, short_uuid()))?;
            let resource = AnyResource::Pod(pod);
            self.store.apply(resource.clone()).await?;
            let adapter = PodAdapter(&resource);
            self.lifecycle.start(&adapter, &self.ctx).await?;
            break; // reconcile again next cycle
        }

        // Scale down (remove excess)
        let excess = matching.len().saturating_sub(replicas);
        for t in matching.iter().take(excess) {
            let adapter = PodAdapter(&t.resource);
            self.lifecycle.stop(&adapter, &self.ctx).await?;
            self.store.delete(&t.resource).await.ok();
        }

        Ok(())
    }
}
```

---

## 5. Network Domain — Pipeline Stages

### 5.1 Service Component

```rust
// components/network/service.rs

pub struct ServiceComponent {
    proxies: Arc<Mutex<HashMap<String, RunningProxy>>>,
}

#[async_trait]
impl Component for ServiceComponent {
    fn kind(&self) -> &'static str { "Service" }
    fn category(&self) -> ResourceCategory { ResourceCategory::Network }

    async fn on_apply(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        let AnyResource::Service(svc) = resource else { return Ok(()) };
        // Bind/rebind ClusterIP + NodePort proxies
        // Reconcile pod port publishing for matching pods
        Ok(())
    }

    async fn on_delete(&self, ctx: &ReconcileContext, resource: &AnyResource) -> Result<()> {
        // Abort proxy tasks, cleanup
        Ok(())
    }
}
```

### 5.2 NSG Pipeline Stage (Future — NetMux Phase 3)

This demonstrates the plug-and-play pipeline pattern. The NSG stage runs **after** the Pod component starts the container — it doesn't modify Pod code at all.

```rust
// components/network/nsg.rs (future)

pub struct NsgStage {
    nft_writer: Arc<NftWriter>,
}

#[async_trait]
impl PipelineStage for NsgStage {
    fn name(&self) -> &str { "nsg" }
    fn interests(&self) -> &[&str] { &["Pod", "NSG"] }

    async fn on_created(&self, resource: &AnyResource, ctx: &StageContext) -> Result<()> {
        match resource {
            AnyResource::Pod(pod) => {
                // Pod created → find VNet → find NSGs → add pod IP to nftables set
                let pod_ip = get_pod_ip(pod);
                let vnet = resolve_pod_vnet(pod, ctx).await?;
                for nsg in find_nsgs_for_vnet(&vnet, ctx).await {
                    self.nft_writer.add_to_set(&nsg.set_name(), pod_ip).await?;
                }
            }
            AnyResource::Nsg(nsg) => {
                // NSG created → compile rules → evaluate existing pods
                self.nft_writer.compile_nsg_rules(nsg).await?;
                let pods = find_pods_in_vnet(nsg.vnet(), ctx).await;
                for pod in pods {
                    let ip = get_pod_ip(&pod);
                    self.nft_writer.add_to_set(&nsg.set_name(), ip).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn on_deleted(&self, kind: &str, uid: &str, ctx: &StageContext) -> Result<()> {
        match kind {
            "Pod" => {
                // Pod deleted → remove IP from all NSG sets
                self.nft_writer.remove_ip_from_all_sets(uid).await?;
            }
            "NSG" => {
                // NSG deleted → remove nftables set + forward rules
                self.nft_writer.remove_nsg(uid).await?;
            }
            _ => {}
        }
        Ok(())
    }
}
```

### 5.3 How NSG Gets Applied to a Pod — Complete Flow

```
Pod YAML applied to /etc/z8s/manifests/
    │
    ▼
ManifestWatcher detects file
    │
    ▼
store.apply(AnyResource::Pod(pod))  →  state = Pending
    │
    ▼
Reconciler tick
    │
    ▼
PodComponent.reconcile()  detects Pending pod
    │
    ▼
ComputeLifecycle.start(pod)
    ├── pull image, build rootfs
    ├── resolve env vars, volumes
    ├── create cgroup
    ├── spawn containers
    └── pipeline.dispatch_created(pod)
         │
         ▼
         ┌──────────────────────────────────────────────────┐
         │  IpamStage:        allocate IP from VNet pool    │
         │  AttachmentStage:  create veth, add host route   │
         │  DnsStage:         register A record             │
         │  ServiceStage:     update endpoint sets          │
         │  NsgStage:         add pod IP to NSG nft set     │  ← plug in
         │  PolicyStage:      evaluate labels → nft sets    │  ← plug in
         └──────────────────────────────────────────────────┘
```

Each stage:
- Is independently testable
- Is independently pluggable (add/remove with one `.stage()` call)
- Reacts only to resource kinds it declares in `interests()`
- Shares the serialized `NftWriter` channel (no concurrent write races)

### 5.4 Bidirectional Flows

The pipeline handles both directions:

**Pod created → NSG evaluates:**
- `NsgStage.on_created(Pod)` → finds VNet → finds NSGs → adds pod IP to nftables set

**NSG created → evaluates existing Pods:**
- `NsgStage.on_created(NSG)` → finds all pods in VNet → adds their IPs to the new set

**NSG updated → recompiles rules:**
- `NsgStage.on_created(NSG)` → recompiles nftables forward chain rules

No cascade. No O(n²) scans. Event-driven.

---

## 6. Storage Domain

```rust
// components/storage/mod.rs

pub trait StorageResource {
    fn namespace(&self) -> &str;
    fn name(&self) -> &str;
}

// ConfigMap and Secret are passive — other components (Pod env vars, volumes)
// read them from the store. Their Component impls are minimal.
```

---

## 7. CRI — Container Runtime Interface

Renamed from `container/`. The CRI is the **single point of contact** for all container operations. Components (`PodComponent`, `JobComponent`) never touch `nix`, `fork`, or `mount` directly — they build a `ContainerSpec` and hand it to the CRI.

### 7.1 Design Principles

1. **Builder pattern for container specs** — callers construct a `ContainerSpec` step by step (volumes, env, limits, security)
2. **All Linux-specific code lives in CRI** — `nix`, `libc`, `mount`, `fork`, `pivot_root`, `setns` are CRI internals
3. **Pluggable hooks** — volume resolvers, env providers, and network attachers are injected, not hardcoded
4. **Testable** — components can build a `ContainerSpec` without root or namespaces, then hand it to a mock CRI in tests

### 7.2 ContainerSpec — The Builder

This is the contract between components and the CRI. It's a self-contained description of what to run.

```rust
// cri/spec.rs

pub struct ContainerSpec {
    pub pod_name: String,
    pub pod_uid: String,
    pub namespace: String,
    pub hostname: String,
    pub containers: Vec<ContainerConfig>,
    pub cgroup_path: String,
    pub labels: BTreeMap<String, String>,
}

pub struct ContainerConfig {
    pub container_id: String,
    pub container_name: String,

    // Image
    pub image: String,
    pub rootfs_path: String,
    pub is_native: bool,

    // Execution
    pub entrypoint: String,
    pub args: Vec<String>,
    pub working_dir: Option<String>,

    // Environment
    pub env: Vec<(String, String)>,

    // Volumes
    pub volumes: Vec<ResolvedVolume>,

    // Resource limits
    pub memory_limit_bytes: Option<i64>,
    pub memory_low_bytes: Option<i64>,
    pub cpu_quota: Option<i64>,
    pub cpu_period: Option<i64>,

    // Security
    pub run_as_user: Option<u32>,
    pub run_as_group: Option<u32>,
    pub privileged: bool,
    pub extra_capabilities: Vec<String>,

    // Network
    pub isolated_net: bool,
    pub published_ports: HashMap<u16, u16>,

    // Health
    pub probes: Vec<ProbeConfig>,
}

pub struct ResolvedVolume {
    pub host_path: String,
    pub container_path: String,
    pub read_only: bool,
}
```

### 7.3 ContainerSpecBuilder

Components build specs fluently. Each method returns `&mut Self` for chaining.

```rust
// cri/spec.rs

pub struct ContainerSpecBuilder {
    spec: ContainerSpec,
}

impl ContainerSpecBuilder {
    pub fn new(pod_name: &str, pod_uid: &str, namespace: &str) -> Self {
        Self {
            spec: ContainerSpec {
                pod_name: pod_name.to_string(),
                pod_uid: pod_uid.to_string(),
                namespace: namespace.to_string(),
                hostname: pod_name.to_string(),
                containers: vec![],
                cgroup_path: String::new(),
                labels: BTreeMap::new(),
            },
        }
    }

    pub fn labels(&mut self, labels: BTreeMap<String, String>) -> &mut Self {
        self.spec.labels = labels;
        self
    }

    pub fn hostname(&mut self, hostname: &str) -> &mut Self {
        self.spec.hostname = hostname.to_string();
        self
    }

    pub fn add_container(&mut self, config: ContainerConfig) -> &mut Self {
        self.spec.containers.push(config);
        self
    }

    pub fn cgroup(&mut self, path: &str) -> &mut Self {
        self.spec.cgroup_path = path.to_string();
        self
    }

    pub fn build(self) -> ContainerSpec {
        self.spec
    }
}
```

### 7.4 ContainerConfigBuilder

Each container within the spec also uses a builder:

```rust
// cri/spec.rs

pub struct ContainerConfigBuilder {
    cfg: ContainerConfig,
}

impl ContainerConfigBuilder {
    pub fn new(container_id: &str, name: &str) -> Self {
        Self {
            cfg: ContainerConfig {
                container_id: container_id.to_string(),
                container_name: name.to_string(),
                image: String::new(),
                rootfs_path: String::new(),
                is_native: false,
                entrypoint: String::new(),
                args: vec![],
                working_dir: None,
                env: vec![],
                volumes: vec![],
                memory_limit_bytes: None,
                memory_low_bytes: None,
                cpu_quota: None,
                cpu_period: None,
                run_as_user: None,
                run_as_group: None,
                privileged: false,
                extra_capabilities: vec![],
                isolated_net: false,
                published_ports: HashMap::new(),
                probes: vec![],
            },
        }
    }

    // --- Image ---

    pub fn image(&mut self, image: &str, rootfs_path: &str) -> &mut Self {
        self.cfg.image = image.to_string();
        self.cfg.rootfs_path = rootfs_path.to_string();
        self.cfg.is_native = image.is_empty() || image == "host" || image.starts_with("host://");
        self
    }

    pub fn native(&mut self) -> &mut Self {
        self.cfg.is_native = true;
        self
    }

    // --- Execution ---

    pub fn entrypoint(&mut self, ep: &str, args: Vec<String>) -> &mut Self {
        self.cfg.entrypoint = ep.to_string();
        self.cfg.args = args;
        self
    }

    pub fn working_dir(&mut self, dir: &str) -> &mut Self {
        self.cfg.working_dir = Some(dir.to_string());
        self
    }

    // --- Environment ---

    pub fn env(&mut self, vars: Vec<(String, String)>) -> &mut Self {
        self.cfg.env = vars;
        self
    }

    pub fn add_env(&mut self, key: &str, value: &str) -> &mut Self {
        self.cfg.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn add_env_if_missing(&mut self, key: &str, value: &str) -> &mut Self {
        if !self.cfg.env.iter().any(|(k, _)| k == key) {
            self.cfg.env.push((key.to_string(), value.to_string()));
        }
        self
    }

    pub fn merge_env(&mut self, vars: Vec<(String, String)>) -> &mut Self {
        for (key, value) in vars {
            if !self.cfg.env.iter().any(|(k, _)| k == &key) {
                self.cfg.env.push((key, value));
            }
        }
        self
    }

    // --- Volumes ---

    pub fn volumes(&mut self, vols: Vec<ResolvedVolume>) -> &mut Self {
        self.cfg.volumes = vols;
        self
    }

    pub fn add_volume(&mut self, host_path: &str, container_path: &str, read_only: bool) -> &mut Self {
        self.cfg.volumes.push(ResolvedVolume {
            host_path: host_path.to_string(),
            container_path: container_path.to_string(),
            read_only,
        });
        self
    }

    // --- Resource Limits ---

    pub fn memory_limit(&mut self, bytes: i64) -> &mut Self {
        self.cfg.memory_limit_bytes = Some(bytes);
        self
    }

    pub fn memory_low(&mut self, bytes: i64) -> &mut Self {
        self.cfg.memory_low_bytes = Some(bytes);
        self
    }

    pub fn cpu_limit(&mut self, quota: i64, period: i64) -> &mut Self {
        self.cfg.cpu_quota = Some(quota);
        self.cfg.cpu_period = Some(period);
        self
    }

    pub fn resource_limits(
        &mut self,
        memory_limit: Option<i64>,
        memory_low: Option<i64>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
    ) -> &mut Self {
        self.cfg.memory_limit_bytes = memory_limit;
        self.cfg.memory_low_bytes = memory_low;
        self.cfg.cpu_quota = cpu_quota;
        self.cfg.cpu_period = cpu_period;
        self
    }

    // --- Security ---

    pub fn run_as(&mut self, uid: Option<u32>, gid: Option<u32>) -> &mut Self {
        self.cfg.run_as_user = uid;
        self.cfg.run_as_group = gid;
        self
    }

    pub fn privileged(&mut self, yes: bool) -> &mut Self {
        self.cfg.privileged = yes;
        self
    }

    pub fn capabilities(&mut self, caps: Vec<String>) -> &mut Self {
        self.cfg.extra_capabilities = caps;
        self
    }

    // --- Network ---

    pub fn isolated_network(&mut self, yes: bool) -> &mut Self {
        self.cfg.isolated_net = yes;
        self
    }

    pub fn publish_port(&mut self, container_port: u16, host_port: u16) -> &mut Self {
        self.cfg.published_ports.insert(container_port, host_port);
        self
    }

    pub fn published_ports(&mut self, ports: HashMap<u16, u16>) -> &mut Self {
        self.cfg.published_ports = ports;
        self
    }

    // --- Health ---

    pub fn probes(&mut self, probes: Vec<ProbeConfig>) -> &mut Self {
        self.cfg.probes = probes;
        self
    }

    pub fn add_probe(&mut self, probe: ProbeConfig) -> &mut Self {
        self.cfg.probes.push(probe);
        self
    }

    // --- Build ---

    pub fn build(self) -> ContainerConfig {
        self.cfg
    }
}
```

### 7.5 How Components Use the Builder

The `ComputeLifecycle` builds specs step by step. Each concern (env, volumes, limits) is resolved by a dedicated method that adds to the builder:

```rust
// components/compute/mod.rs

impl ComputeLifecycle {
    pub async fn start(
        &self,
        res: &dyn ComputeResource,
        ctx: &ReconcileContext,
    ) -> Result<()> {
        let containers = extract_containers(res.resource_ref());
        let pod_uid = res.uid().to_string();
        let pod_name = res.name().to_string();
        let namespace = res.namespace().to_string();

        if self.process_tracker.is_running(&pod_name).await {
            return Ok(());
        }

        // 1. Cgroup
        ctx.cri.create_pod_cgroup(&pod_uid)?;

        // 2. Service target ports (for port publishing)
        let service_ports = self.service_target_ports(res, ctx).await;

        // 3. Fetch ConfigMaps and Secrets for env/volume resolution
        let (cms, secrets) = self.fetch_configmaps_and_secrets(ctx).await;

        // 4. Build container specs via builder
        let mut spec_builder = ContainerSpecBuilder::new(&pod_name, &pod_uid, &namespace);
        spec_builder.labels(res.labels().clone());

        for container in &containers {
            let container_id = format!("{}-{}", pod_name, container.name);
            let image_ref = container.image.clone().unwrap_or_default();

            // Resolve image → rootfs
            let rootfs_path = if image_ref.is_empty() || image_ref == "host" || image_ref.starts_with("host://") {
                String::new()
            } else {
                ctx.cri.unpack_image(&image_ref, &container_id).await?
            };

            // Resolve entrypoint
            let (entrypoint, args) = if rootfs_path.is_empty() {
                resolve_native_argv(container)
            } else {
                crate::cri::oci::resolve_argv(container, &rootfs_path)
            };

            let mut cb = ContainerConfigBuilder::new(&container_id, &container.name);
            cb.image(&image_ref, &rootfs_path)
              .entrypoint(&entrypoint, args);

            // --- Environment ---
            self.resolve_env(&mut cb, res, container, &cms, &secrets, ctx).await;
            cb.add_env_if_missing("HOME", &default_home(res));
            cb.add_env_if_missing("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
            if !rootfs_path.is_empty() {
                self.merge_image_env(&mut cb, &rootfs_path);
            }

            // --- Volumes ---
            self.resolve_volumes(&mut cb, res, container, &pod_uid, &cms, &secrets);

            // --- Resource limits ---
            self.resolve_resource_limits(&mut cb, container);

            // --- Security ---
            self.resolve_security(&mut cb, res, container);

            // --- Network ---
            let ports = self.resolve_network(&mut cb, container, &service_ports);

            // --- Health probes ---
            self.resolve_probes(&mut cb, container);

            spec_builder.add_container(cb.build());
        }

        let spec = spec_builder.build();

        // 5. Hand spec to CRI — all Linux-specific work happens inside
        ctx.cri.start_pod(&spec).await?;

        // 6. Dispatch pipeline (network stages, DNS, policy)
        ctx.pipeline.dispatch_created(res.resource_ref(), &ctx.stage_ctx()).await;

        Ok(())
    }

    // --- Resolve methods (each adds to builder, easy to override per resource type) ---

    async fn resolve_env(
        &self,
        cb: &mut ContainerConfigBuilder,
        res: &dyn ComputeResource,
        container: &Container,
        cms: &HashMap<(String, String), ConfigMap>,
        secrets: &HashMap<(String, String), Secret>,
        ctx: &ReconcileContext,
    ) {
        // Container env
        let mut env: Vec<(String, String)> = container.env.as_ref()
            .map(|e| e.iter().map(|v| (v.name.clone(), v.value.clone().unwrap_or_default())).collect())
            .unwrap_or_default();

        // envFrom (ConfigMaps + Secrets)
        env.extend(Self::resolve_env_from(container, res.namespace(), cms, secrets));

        // Service env vars (SVCNAME_SERVICE_HOST, SVCNAME_SERVICE_PORT)
        env.extend(self.resolve_service_env(res, ctx).await);

        cb.env(env);
    }

    fn resolve_volumes(
        &self,
        cb: &mut ContainerConfigBuilder,
        res: &dyn ComputeResource,
        container: &Container,
        pod_uid: &str,
        cms: &HashMap<(String, String), ConfigMap>,
        secrets: &HashMap<(String, String), Secret>,
    ) {
        // Only for Pod resources with a real spec
        let Some(pod) = Self::as_pod(res) else { return };
        let volumes = crate::cri::volumes::prepare_volumes(
            pod, &container.name, pod_uid,
            &|ns, name| cms.get(&(ns.to_string(), name.to_string())).cloned(),
            &|ns, name| secrets.get(&(ns.to_string(), name.to_string())).cloned(),
        ).unwrap_or_default();
        cb.volumes(volumes);
    }

    fn resolve_resource_limits(
        &self,
        cb: &mut ContainerConfigBuilder,
        container: &Container,
    ) {
        let Some(resources) = &container.resources else { return };
        if let Some(limits) = &resources.limits {
            if let Some(mem) = limits.get("memory") {
                let bytes = parse_quantity_bytes(mem);
                if bytes > 0 { cb.memory_limit(bytes as i64); }
            }
            if let Some(cpu) = limits.get("cpu") {
                let (quota, period) = parse_quantity_cpu(cpu);
                cb.cpu_limit(quota, period);
            }
        }
        if let Some(requests) = &resources.requests {
            if let Some(mem) = requests.get("memory") {
                let bytes = parse_quantity_bytes(mem);
                if bytes > 0 { cb.memory_low(bytes as i64); }
            }
        }
    }

    fn resolve_security(
        &self,
        cb: &mut ContainerConfigBuilder,
        res: &dyn ComputeResource,
        container: &Container,
    ) {
        let pod_sc = Self::as_pod(res).and_then(|p| p.spec.as_ref().and_then(|s| s.security_context.as_ref()));
        cb.run_as(
            resolve_run_as_user(pod_sc, container),
            resolve_run_as_group(pod_sc, container),
        );
        cb.privileged(container.security_context.as_ref().and_then(|sc| sc.privileged).unwrap_or(false));
        let caps: Vec<String> = container.security_context.as_ref()
            .and_then(|sc| sc.capabilities.as_ref())
            .and_then(|c| c.add.as_ref())
            .cloned()
            .unwrap_or_default();
        cb.capabilities(caps);
    }

    fn resolve_network(
        &self,
        cb: &mut ContainerConfigBuilder,
        container: &Container,
        service_ports: &[u16],
    ) -> Vec<u16> {
        let declared: Vec<u16> = container.ports.as_ref()
            .map(|ps| ps.iter().map(|p| p.container_port as u16).collect())
            .unwrap_or_default();
        let isolated = !declared.is_empty() || !service_ports.is_empty();
        cb.isolated_network(isolated);
        declared
    }

    fn resolve_probes(
        &self,
        cb: &mut ContainerConfigBuilder,
        container: &Container,
    ) {
        let mut probes = vec![];
        for probe in [&container.liveness_probe, &container.readiness_probe, &container.startup_probe]
            .into_iter().flatten()
        {
            if let Some(config) = ProbeConfig::from_probe(probe) {
                probes.push(config);
            }
        }
        cb.probes(probes);
    }
}
```

### 7.6 ContainerRuntime — The CRI Implementation

The runtime takes a `ContainerSpec` and does all the Linux work. Components never see `nix`, `fork`, or `mount`.

```rust
// cri/runtime.rs

pub struct ContainerRuntime {
    pub image_manager: Arc<ImageManager>,
    pub cgroup_manager: Arc<CgroupManager>,
    running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    restart_counts: Arc<Mutex<HashMap<String, u32>>>,
}

impl ContainerRuntime {
    pub fn new(
        image_manager: Arc<ImageManager>,
        cgroup_manager: Arc<CgroupManager>,
    ) -> Self;

    // --- Image management ---

    pub async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String>;

    // --- Cgroup management ---

    pub fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String>;
    pub fn remove_cgroup(&self, pod_uid: &str) -> Result<()>;

    // --- Container lifecycle ---

    /// Start all containers described in the spec.
    /// This is the main entry point — the CRI owns fork/mount/namespace/pivot_root.
    pub async fn start_pod(&self, spec: &ContainerSpec) -> Result<()> {
        self.cgroup_manager.create_pod_cgroup(&spec.pod_uid)?;

        for cfg in &spec.containers {
            let rc = self.spawn_container(cfg, &spec.pod_uid, &spec.hostname).await?;
            self.running.lock().await.insert(cfg.container_id.clone(), rc);
        }

        Ok(())
    }

    /// Stop all containers for a pod, cleanup cgroup + volumes.
    pub async fn stop_pod(&self, spec: &ContainerSpec) {
        for cfg in &spec.containers {
            self.stop_container(&cfg.container_id).await;
        }
        self.cgroup_manager.remove_cgroup(&spec.pod_uid).ok();
        crate::cri::volumes::cleanup_emptydir(&spec.pod_uid);
    }

    /// Spawn a single container — all the fork/mount/namespace logic lives here.
    /// Root mode vs user-ns mode branching is internal to the CRI.
    async fn spawn_container(
        &self,
        cfg: &ContainerConfig,
        pod_uid: &str,
        hostname: &str,
    ) -> Result<RunningContainer>;

    pub async fn stop_container(&self, container_id: &str);

    // --- Query methods ---

    pub async fn is_pod_running(&self, pod_name: &str) -> bool;
    pub async fn is_pod_alive(&self, pod_name: &str) -> bool;
    pub async fn is_pod_ready(&self, pod_name: &str) -> bool;

    pub async fn backend_connect_port(&self, pod_name: &str, container_port: u16) -> u16;
    pub async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String>;
    pub async fn pod_restart_counts(&self, pod_name: &str) -> HashMap<String, u32>;

    pub async fn get_running(&self, container_id: &str) -> Option<RunningContainer>;
    pub async fn remove_running(&self, container_id: &str) -> Option<RunningContainer>;

    // --- Resource limits ---

    pub fn apply_resource_limits(&self, pod_uid: &str, cfg: &ContainerConfig) -> Result<()> {
        if let Some(bytes) = cfg.memory_limit_bytes {
            self.cgroup_manager.set_memory_limit(pod_uid, bytes)?;
        }
        if let Some(bytes) = cfg.memory_low_bytes {
            self.cgroup_manager.set_memory_low(pod_uid, bytes)?;
        }
        if let (Some(quota), Some(period)) = (cfg.cpu_quota, cfg.cpu_period) {
            self.cgroup_manager.set_cpu_limit(pod_uid, quota, period)?;
        }
        Ok(())
    }
}
```

### 7.7 Internal: spawn_container Reads From ContainerConfig

The actual `spawn_container` method reads everything it needs from `ContainerConfig`. No `k8s_openapi` types, no `AnyResource`, no `ResourceStore`:

```rust
// cri/runtime.rs

impl ContainerRuntime {
    async fn spawn_container(
        &self,
        cfg: &ContainerConfig,
        pod_uid: &str,
        hostname: &str,
    ) -> Result<RunningContainer> {
        // 1. Apply resource limits from cfg
        self.apply_resource_limits(pod_uid, cfg)?;

        // 2. Determine entrypoint (native vs OCI)
        let (entrypoint, args) = if cfg.is_native {
            (cfg.entrypoint.clone(), cfg.args.clone())
        } else {
            crate::cri::oci::resolve_argv_from_parts(
                &cfg.entrypoint, &cfg.args, &cfg.rootfs_path
            )
        };

        // 3. Stage volumes in rootfs
        if !cfg.is_native && !cfg.volumes.is_empty() {
            crate::cri::volumes::scrub_rootfs_volume_mounts(&cfg.rootfs_path, &cfg.volumes);
            crate::cri::volumes::stage_volumes_in_rootfs(&cfg.rootfs_path, &cfg.volumes);
            crate::cri::rootfs::prepare_rootfs(&cfg.rootfs_path)?;
        }

        // 4. Fork + namespace setup (root vs userns — internal branching)
        if crate::cri::rootfs::is_root() {
            self.spawn_root_ns(cfg, pod_uid, hostname, &entrypoint, &args).await
        } else {
            self.spawn_userns(cfg, pod_uid, hostname, &entrypoint, &args).await
        }

        // spawn_root_ns / spawn_userns use cfg.env, cfg.run_as_user,
        // cfg.run_as_group, cfg.privileged, cfg.extra_capabilities,
        // cfg.isolated_net, cfg.volumes — all from ContainerConfig
    }
}
```

### 7.8 Why This Design Works for Adding New Features

| "I want to add..." | What you do |
|---|---|
| A new volume type (e.g., NFS) | Add a resolver in `cri/volumes.rs`, call `cb.add_volume(...)` in `ComputeLifecycle.resolve_volumes()` |
| A new env var source | Add a method to `ComputeLifecycle`, call `cb.merge_env(...)` |
| A new resource limit (e.g., pids.max) | Add a field to `ContainerConfig`, add a builder method, apply in `ContainerRuntime.apply_resource_limits()` |
| Job with custom cleanup | Override `ComputeLifecycle.stop()` in `JobComponent` — the spec already knows about ephemeral IPs |
| Init containers | Add a second pass in `ComputeLifecycle.start()` — same builder, same CRI call |
| Sidecar injection | Add containers to `ContainerSpecBuilder` in a pipeline stage |

### 7.9 File Mapping

| Current file | New file | Notes |
|---|---|---|
| `container/mod.rs` | `cri/mod.rs` | Updated re-exports |
| (new) | `cri/spec.rs` | `ContainerSpec`, `ContainerConfig`, builders |
| `container/image.rs` | `cri/image.rs` | Unchanged |
| `container/oci_config.rs` | `cri/oci.rs` | Renamed |
| `container/rootfs.rs` | `cri/rootfs.rs` | Unchanged |
| `container/volumes.rs` | `cri/volumes.rs` | Unchanged |
| `supervisor/cgroup.rs` | `cri/cgroup.rs` | Unchanged |
| `supervisor/health.rs` | `cri/health.rs` | Unchanged |
| `supervisor/process.rs` (spawn logic) | `cri/runtime.rs` | Takes `ContainerSpec` instead of `AnyResource` |
| `supervisor/process.rs` (tracking) | `scheduler/process.rs` | `ProcessTracker` |

### 7.10 Future Extensibility: CRI Provider Trait

When we want to support alternative runtimes (e.g., a remote containerd shim, or a mock for testing), the CRI can be behind a trait:

```rust
// cri/mod.rs

#[async_trait]
pub trait RuntimeProvider: Send + Sync {
    async fn start_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_container(&self, container_id: &str) -> Result<()>;
    async fn is_pod_alive(&self, pod_name: &str) -> bool;
    async fn is_pod_ready(&self, pod_name: &str) -> bool;
    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16;
    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String>;
}
```

The current `ContainerRuntime` implements `RuntimeProvider`. Tests can use `MockRuntime`. Future remote shims implement the same trait. The `ContainerSpec` is the shared contract — it doesn't care **how** the container runs, only **what** to run.

---

## 8. Net — Network Engine

Renamed from `network/`. The `NetworkManager` struct is split: service proxy management moves to `components/network/service.rs`, DNS stays in `net/dns.rs`.

### 8.1 NetworkEngine Trait

```rust
// net/mod.rs

#[async_trait]
pub trait NetworkEngine: Send + Sync {
    fn dns_port(&self) -> Option<u16>;
    async fn sync_service(&self, svc: &Service);
    async fn remove_service(&self, ns: &str, name: &str);
    async fn compute_endpoints(&self, svc: &Service) -> Endpoints;
    async fn compute_endpointslices(&self, svc: &Service) -> Vec<EndpointSlice>;
}
```

### 8.2 File Mapping

| Current file | New file | Notes |
|---|---|---|
| `network/mod.rs` | `net/mod.rs` + `components/network/service.rs` | Split: trait stays, impl moves |
| `network/dns.rs` | `net/dns.rs` | Unchanged |
| `network/port_publish.rs` | `net/port_publish.rs` | Unchanged (replaced by NetMux later) |
| `network/service_proxy.rs` | `net/service_proxy.rs` | Unchanged (replaced by NetMux later) |

---

## 9. Scheduler

### 9.1 Reconciler

Drives the component reconcile loops on a timer:

```rust
// scheduler/mod.rs

pub struct Reconciler {
    registry: Arc<ComponentRegistry>,
    ctx: Arc<ReconcileContext>,
    process_tracker: Arc<ProcessTracker>,
}

impl Reconciler {
    pub async fn run(&self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(10));
        loop {
            ticker.tick().await;
            self.reconcile_all().await;
        }
    }

    async fn reconcile_all(&self) {
        // 1. Reap zombies, handle exited containers
        let exited = self.process_tracker.reap_zombies().await;
        self.handle_exits(&exited).await;

        // 2. Run component reconcile for all resources
        self.registry.reconcile_all(&self.ctx).await;
    }

    async fn handle_exits(&self, exited: &[(u32, i32)]) {
        // Map PID → container → pod → component → handle_exit
    }
}
```

### 9.2 ProcessTracker

Container process tracking and zombie reaping, extracted from `supervisor/process.rs`:

```rust
// scheduler/process.rs

pub struct ProcessTracker {
    pub running: Arc<Mutex<HashMap<String, RunningContainer>>>,
    restart_counts: Arc<Mutex<HashMap<String, u32>>>,
}

impl ProcessTracker {
    pub fn new() -> Self;

    pub async fn is_running(&self, pod_name: &str) -> bool;
    pub async fn insert(&self, container_id: String, rc: RunningContainer);
    pub async fn remove(&self, container_id: &str) -> Option<RunningContainer>;
    pub async fn get(&self, container_id: &str) -> Option<RunningContainer>;

    pub async fn reap_zombies(&self) -> Vec<(u32, i32)>;
}
```

---

## 10. API — Split Handlers

### 10.1 Router Setup

```rust
// api/server.rs

pub async fn run_server(
    store: Arc<ResourceStore>,
    runtime: Arc<ContainerRuntime>,
    net: Arc<dyn NetworkEngine>,
) {
    let state = build_app_state(store, runtime, net).await;
    let app = build_router(state);
    let addr = format!("0.0.0.0:{}", crate::config::get().api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(handlers::pod::routes())
        .merge(handlers::deployment::routes())
        .merge(handlers::service::routes())
        .merge(handlers::namespace::routes())
        .merge(handlers::configmap::routes())
        .merge(handlers::secret::routes())
        .merge(handlers::storage::routes())
        .merge(handlers::discovery::routes())
        .merge(handlers::metrics::routes())
        .merge(handlers::system::routes())
        .with_state(state)
}
```

### 10.2 Per-Resource Handler Example

```rust
// api/handlers/pod.rs

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/pods", get(list_pods_all))
        .route("/api/v1/namespaces/{namespace}/pods", get(list_pods).post(create_pod))
        .route("/api/v1/namespaces/{namespace}/pods/{name}", any(pod_handler))
        .route("/api/v1/namespaces/{namespace}/pods/{name}/log", get(get_pod_log))
        .route("/api/v1/namespaces/{namespace}/pods/{name}/exec",
            get(exec::exec_handler).post(exec::exec_post_handler))
}

async fn create_pod(...) -> Result<...> { /* moved from server/api.rs */ }
async fn list_pods(...) -> Result<...> { /* moved from server/api.rs */ }
async fn get_pod_log(...) -> Result<...> { /* moved from server/api.rs */ }
async fn pod_handler(...) -> Result<...> { /* moved from server/api.rs */ }
```

---

## 11. Updated main.rs

```rust
// main.rs

mod api;
mod components;
mod config;
mod cri;
mod init;
mod manifest;
mod net;
mod scheduler;

use components::{ComponentRegistry, PipelineBuilder, ReconcileContext};
use components::compute::{ComputeLifecycle, PodComponent, DeploymentComponent};
use components::network::ServiceComponent;
use components::storage::{ConfigMapComponent, SecretComponent, StorageComponent};
use cri::{ContainerRuntime, ImageManager, CgroupManager};
use scheduler::{Reconciler, ProcessTracker};

#[tokio::main]
async fn main() -> Result<()> {
    config::init();
    // ... logging, PID 1 check (unchanged) ...

    let store = Arc::new(ResourceStore::new());

    // CRI
    let image_manager = Arc::new(ImageManager::new()?);
    let cgroup_manager = Arc::new(CgroupManager::new()?);
    let process_tracker = Arc::new(ProcessTracker::new());
    let runtime = Arc::new(ContainerRuntime::new(image_manager, cgroup_manager));

    // Network
    let net = Arc::new(net::CurrentNetwork::new(store.clone(), runtime.clone()));
    if let Some(port) = net::dns::run_dns(store.clone()).await {
        net::set_dns_port(port);
    }

    // Pipeline stages
    // Current: no stages (port_publish is called inside CRI spawn)
    // Future NetMux stages will be added here:
    //   .stage(Box::new(IpamStage::new(pool)))
    //   .stage(Box::new(AttachmentStage::new()))
    //   .stage(Box::new(DnsStage::new(dns)))
    //   .stage(Box::new(NsgStage::new(nft_writer)))
    //   .stage(Box::new(PolicyStage::new(nft_writer)))
    let pipeline = Arc::new(PipelineBuilder::new().build());

    let ctx = Arc::new(ReconcileContext {
        store: store.clone(),
        pipeline: pipeline.clone(),
        cri: runtime.clone(),
        net: net.clone(),
    });

    // Components — each is a thin wrapper, CRI does the heavy lifting
    let lifecycle = ComputeLifecycle::new(runtime.clone(), process_tracker.clone());
    let mut registry = ComponentRegistry::new();
    registry.register(Box::new(PodComponent::new(lifecycle.clone())));
    registry.register(Box::new(DeploymentComponent::new(
        store.clone(), Arc::new(lifecycle), ctx.clone(),
    )));
    registry.register(Box::new(ServiceComponent::new()));
    registry.register(Box::new(ConfigMapComponent::new()));
    registry.register(Box::new(SecretComponent::new()));
    registry.register(Box::new(StorageComponent::new()));
    let registry = Arc::new(registry);

    // Manifest watcher
    let watcher = Arc::new(ManifestWatcher::new(store.clone()));
    watcher.load_existing().await?;

    // Background tasks
    let w = watcher.clone();
    tokio::spawn(async move { w.start_watching().await });
    let r = registry.clone();
    let c = ctx.clone();
    tokio::spawn(async move { Reconciler::new(r, c, process_tracker.clone()).run().await });

    // API server
    let s = store.clone();
    let rt = runtime.clone();
    let n = net.clone();
    tokio::spawn(async move { api::server::run_server(s, rt, n).await });

    // Initial reconcile
    registry.reconcile_all(&ctx).await;

    // Signal handling (unchanged)
    // ...
}
```

---

## 12. Migration Strategy — Step by Step

Each step compiles and passes tests before proceeding.

### Step 1: Create directories + move files (pure renames)

1. Create `cri/`, `net/`, `scheduler/`, `components/`, `api/handlers/`
2. Copy files to new locations with updated `mod` declarations
3. Add re-export shims in old locations so nothing breaks
4. `cargo build && cargo test`

### Step 2: Extract `ResourceStore` from `api/types.rs`

1. Move `ResourceStore`, `ResourceTracker`, `ResourceState` to `api/store.rs`
2. Re-export from `api/types.rs`
3. `cargo build && cargo test`

### Step 3: Extract `ContainerRuntime` from `supervisor/process.rs`

1. Create `cri/runtime.rs` with `ContainerRuntime` struct
2. Move spawn/stop/tracking methods
3. Leave zombie reaping in `scheduler/process.rs` as `ProcessTracker`
4. `cargo build && cargo test`

### Step 4: Add `Component` trait + `PipelineStage` trait

1. Create `components/mod.rs` with traits and builder
2. No implementations yet — just the abstractions
3. `cargo build`

### Step 5: Implement `PodComponent`

1. Create `components/compute/mod.rs` with `ComputeLifecycle`
2. Create `components/compute/pod.rs` with `PodComponent`
3. Wire in `main.rs`
4. `cargo build && cargo test`
5. Verify pods still start/stop/restart

### Step 6: Implement `DeploymentComponent`

1. Create `components/compute/deployment.rs`
2. Move logic from `controller.rs`
3. Remove old `controller.rs`
4. `cargo build && cargo test`
5. Verify deployment scaling works

### Step 7: Implement `ServiceComponent`

1. Create `components/network/service.rs`
2. Move service proxy management from `net/mod.rs`
3. `cargo build && cargo test`
4. Verify ClusterIP/NodePort proxying works

### Step 8: Implement storage components

1. Create `components/storage/configmap.rs`, `secret.rs`, `storage.rs`
2. These are minimal — ConfigMap/Secret are passive resources
3. `cargo build && cargo test`

### Step 9: Split `server/api.rs` into handlers

1. Create `api/server.rs` with router setup
2. Create `api/handlers/*.rs` per resource type
3. Remove old `server/` directory
4. `cargo build && cargo test`

### Step 10: Remove old directories

1. Remove `supervisor/` (all code moved to `cri/` + `scheduler/`)
2. Remove `container/` (moved to `cri/`)
3. Remove `network/` (moved to `net/`)
4. Remove `controller.rs` (moved to `components/compute/deployment.rs`)
5. Remove `server/` (moved to `api/`)
6. Final `cargo build && cargo test`

---

## 13. Future: Adding New Components

After this refactoring, adding a new component (e.g., VNet) is:

1. Create `components/network/vnet.rs` implementing `Component`
2. Register in `main.rs`: `registry.register(Box::new(VnetComponent::new()));`
3. Done. No existing code changes.

Adding a new pipeline stage (e.g., NSG) is:

1. Create `components/network/nsg.rs` implementing `PipelineStage`
2. Add to pipeline builder in `main.rs`: `.stage(Box::new(NsgStage::new(nft_writer)))`
3. Done. No existing code changes.

Adding a new compute resource (e.g., Job) is:

1. Create `components/compute/job.rs` with `JobAdapter` implementing `ComputeResource`
2. Reuses `ComputeLifecycle` — zero duplication
3. Register in `main.rs`
4. Done.

---

## 14. Decoupling Architecture — Four Independent Layers

The system is built as **four independent modules** connected only by thin trait boundaries. Each module can be extracted into its own crate with zero refactoring — they share no internal types, only trait interfaces defined in `api/types.rs` and the `Component`/`PipelineStage` traits.

### 14.1 The Four Layers

```
┌─────────────────────────────────────────────────────────┐
│                     API Server                          │
│    api/ — axum HTTP handlers, types, store             │
│    Depends on: Scheduler (trait) only                  │
│    Knows nothing about: CRI internals, net internals   │
└────────────────────────┬────────────────────────────────┘
                         │ uses
                         ▼
┌─────────────────────────────────────────────────────────┐
│                     Scheduler                           │
│    scheduler/ — reconciliation loop, process tracking  │
│    Depends on: CRI (trait) + NetMux (trait)            │
│    Orchestrates both, but they don't know each other   │
│    Knows nothing about: HTTP, YAML, k8s types          │
└──────────┬──────────────────────────┬───────────────────┘
           │ uses                     │ uses
           ▼                          ▼
┌──────────────────────┐   ┌──────────────────────────────┐
│        CRI           │   │        NetMux / Net           │
│  cri/                │   │  net/                         │
│  Container spawn,    │   │  DNS, nftables, veth,         │
│  images, rootfs,     │   │  service proxy, IPAM,         │
│  volumes, cgroups,   │   │  port publish                 │
│  security            │   │                               │
│                      │   │                               │
│  Depends on:         │   │  Depends on:                  │
│  NOTHING external    │   │  NOTHING external             │
│  (pure Linux ops)    │   │  (pure network ops)           │
│                      │   │                               │
│  Knows nothing about:│   │  Knows nothing about:         │
│  - NetMux            │   │  - CRI                        │
│  - API server        │   │  - Containers, images, rootfs │
│  - YAML/k8s types    │   │  - API server                 │
│  - Scheduler         │   │  - Scheduler                  │
│  - ResourceStore     │   │  - ResourceStore              │
└──────────────────────┘   └──────────────────────────────┘
```

### 14.2 Trait Boundaries (The Only Coupling Points)

The four layers communicate exclusively through traits. These traits are the **narrowest possible interface** between modules.

#### CRI ↔ Scheduler: `RuntimeProvider`

```rust
// cri/spec.rs — standalone, no imports from other z8s modules

pub struct ContainerSpec { ... }       // pure data
pub struct ContainerConfig { ... }     // pure data
pub struct ResolvedVolume { ... }      // pure data

// cri/runtime.rs

#[async_trait]
pub trait RuntimeProvider: Send + Sync {
    async fn start_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_pod(&self, spec: &ContainerSpec) -> Result<()>;
    async fn stop_container(&self, container_id: &str) -> Result<()>;
    async fn is_pod_alive(&self, pod_name: &str) -> bool;
    async fn is_pod_ready(&self, pod_name: &str) -> bool;
    async fn backend_connect_port(&self, pod_name: &str, port: u16) -> u16;
    async fn get_container_logs(&self, pod_name: &str, container_name: &str) -> Vec<String>;
    async fn unpack_image(&self, image_ref: &str, container_id: &str) -> Result<String>;
    fn create_pod_cgroup(&self, pod_uid: &str) -> Result<String>;
    fn remove_cgroup(&self, pod_uid: &str) -> Result<()>;
}
```

The CRI **never imports** anything from `net/`, `scheduler/`, `api/`, or `components/`. It receives a `ContainerSpec` (pure data) and returns results. It could be extracted to `z8s-cri` crate with zero changes.

#### NetMux ↔ Scheduler: `NetworkEngine`

```rust
// net/mod.rs

pub struct ServiceEndpoint { ... }     // pure data

#[async_trait]
pub trait NetworkEngine: Send + Sync {
    fn dns_port(&self) -> Option<u16>;
    async fn sync_service(&self, svc: &Service) -> Result<()>;
    async fn remove_service(&self, ns: &str, name: &str) -> Result<()>;
    async fn sync_services_for_labels(&self, ns: &str, labels: &BTreeMap<String, String>) -> Result<()>;
    async fn compute_endpoints(&self, svc: &Service) -> Endpoints;
    async fn compute_endpointslices(&self, svc: &Service) -> Vec<EndpointSlice>;

    // Future NetMux methods (added when NetMux is implemented):
    // async fn allocate_ip(&self, vnet: &str) -> Result<Ipv4Addr>;
    // async fn release_ip(&self, ip: Ipv4Addr) -> Result<()>;
    // async fn create_veth(&self, pod_uid: &str) -> Result<(String, String)>;
    // async fn add_route(&self, ip: Ipv4Addr, ifindex: u32) -> Result<()>;
    // async fn add_to_nft_set(&self, set: &str, ip: Ipv4Addr) -> Result<()>;
    // async fn remove_from_nft_set(&self, set: &str, ip: Ipv4Addr) -> Result<()>;
    // async fn compile_nsg(&self, nsg: &NsgSpec) -> Result<()>;
    // async fn compile_network_policy(&self, policy: &NetworkPolicySpec) -> Result<()>;
}
```

The network engine **never imports** anything from `cri/`, `scheduler/`, `api/`, or `components/`. It receives service/VNet/NSG specs and programs the network. It could be extracted to `z8s-net` crate with zero changes.

#### Scheduler ↔ API: `ResourceStore` + `ComponentRegistry`

```rust
// api/types.rs — shared data types, no business logic

pub enum AnyResource { ... }           // shared enum
pub enum ResourceState { ... }         // shared enum
pub struct ResourceTracker { ... }     // shared data
pub struct ResourceStore { ... }       // shared store
```

```rust
// components/mod.rs — shared orchestration traits

pub trait Component: Send + Sync { ... }
pub trait PipelineStage: Send + Sync { ... }
```

The API server **never imports** `cri/` or `net/` directly. It calls the scheduler via `ComponentRegistry.reconcile_all()`. It could be extracted to `z8s-api` crate with only `api/types.rs` as a shared dependency.

### 14.3 What Each Module CAN Import

```
Module          Can import                     CANNOT import
──────────────  ────────────────────────────   ────────────────────────────
cri/            std, external crates only       api/, net/, scheduler/, components/
net/            std, external crates only       api/, cri/, scheduler/, components/
scheduler/      cri/spec.rs (trait + data),     Nothing from cri/ internals
                net/ (trait only),
                api/types.rs (data only)
api/            components/ (traits only),      cri/, net/ directly
                scheduler/ (reconcile method)
components/     api/types.rs (data),            cri/ internals, net/ internals
                cri/spec.rs (trait + data),
                net/ (trait only)
```

### 14.4 Crate Extraction Readiness

Each module is written to be crate-ready. When the time comes:

```
z8s/                    (main binary, glues everything)
z8s-api/                (api/types.rs, api/server.rs, api/handlers/)
z8s-cri/                (cri/ — ContainerSpec, RuntimeProvider impl)
z8s-net/                (net/ — NetworkEngine impl, DNS, future NetMux)
z8s-scheduler/          (scheduler/ — Reconciler, ProcessTracker)
z8s-components/         (components/ — Component trait, PipelineStage, impls)
```

Shared types that multiple crates need (`AnyResource`, `ResourceStore`, `ContainerSpec`) go in `z8s-api` or a minimal `z8s-types` crate.

### 14.5 Dependency Graph (Visual)

```
  ┌──────────┐
  │ z8s-api  │──── uses ────▶ z8s-components (Component trait)
  │          │                  │
  │          │                  ├── uses ▶ z8s-cri (RuntimeProvider trait + ContainerSpec)
  │          │                  │
  │          │                  └── uses ▶ z8s-net (NetworkEngine trait)
  │          │
  │          │──── uses ────▶ z8s-scheduler
  │          │                  │
  │          │                  ├── uses ▶ z8s-cri (RuntimeProvider trait)
  │          │                  │
  │          │                  └── uses ▶ z8s-net (NetworkEngine trait)
  └──────────┘

  z8s-cri  ←── depends on NOTHING in z8s
  z8s-net  ←── depends on NOTHING in z8s
```

**z8s-cri and z8s-net are leaf nodes.** They have no upward dependencies. This is the decoupling guarantee.

### 14.6 Why This Matters for NetMux

When NetMux is implemented (network-architecture-plan.md), the work is entirely contained within `net/`:

1. Add `net/pool.rs` — IP pool allocator (uses `BTreeSet`, no z8s imports)
2. Add `net/netmux.rs` — nftables engine via `rustables` (no z8s imports)
3. Add `net/veth.rs` — veth pair creation (uses `libc`, no z8s imports)
4. Add new `NetworkEngine` methods (`allocate_ip`, `create_veth`, `add_route`, etc.)
5. Add pipeline stages in `components/network/` (NSG, VNet, NetworkPolicy)

Steps 1–4 touch **only** `net/`. The CRI, API, and scheduler are completely unaware. Step 5 adds new pipeline stages that use the new `NetworkEngine` methods — but again, CRI and API don't change.

### 14.7 Current Coupling Violations (To Fix During Migration)

| Current violation | Where | Fix |
|---|---|---|
| `supervisor/process.rs` imports `network::port_publish` | CRI knows about net | CRI receives `isolated_net: bool` in `ContainerConfig`, port publish moves to a pipeline stage or stays behind the `RuntimeProvider` boundary |
| `supervisor/process.rs` imports `network::NetworkManager` | CRI knows about net | Remove. Scheduler orchestrates network calls, not CRI |
| `container/rootfs.rs` calls `network::dns_port()` | CRI knows about net | DNS port passed via `ContainerConfig` or `ContainerSpec` |
| `server/api.rs` imports `supervisor::process` directly | API knows CRI internals | API goes through scheduler/component traits only |
| `server/exec.rs` imports `container::rootfs` | API knows CRI internals | Exec goes through `RuntimeProvider` trait |

---

## 15. Risk Assessment

| Risk | Mitigation |
|---|---|
| Breaking existing functionality | Step-by-step migration with `cargo test` after each step |
| `AnyResource` uid() returns owned String but trait wants &str | Use `Cow<str>` or adjust trait signatures |
| Large PR hard to review | 10 small commits (one per migration step) |
| Performance regression from trait dispatch | All components are `Arc<Box<dyn Component>>` — virtual dispatch cost is negligible vs container spawn |
| API handler split introduces route conflicts | Each handler module returns `Router<AppState>`, merged in `build_router` |

---

## 16. Testing Strategy

After each migration step:

1. `cargo build` — must compile
2. `cargo test` — all unit tests pass
3. Manual smoke test:
   - Apply a Pod YAML → verify it starts
   - Apply a Deployment YAML → verify scaling
   - Apply a Service YAML → verify ClusterIP proxy
   - `kubectl get pods` → verify API server works
   - `kubectl exec` → verify exec works
   - Delete a pod → verify it restarts

---

## 17. Summary

| Aspect | Before | After |
|---|---|---|
| Adding VNet | Touch 5+ files | 1 file + 1 registration line |
| Pod/Job code reuse | Copy/paste | Shared `ComputeLifecycle` via trait |
| NSG → Pod flow | Hardcoded method calls | Pipeline stage, plug-and-play |
| `supervisor/process.rs` | 1,500-line god object | Split into `cri/runtime.rs` (spawn) + `scheduler/process.rs` (tracking) |
| `server/api.rs` | 1,800-line monolith | Per-resource handler files |
| Adding new pipeline stage | Modify PodComponent | `.stage(Box::new(MyStage))` |
| CRI ↔ Network coupling | `ProcessSupervisor` holds `NetworkManager` reference | Zero coupling — scheduler orchestrates both |
| Crate extraction | Impossible — circular deps everywhere | Each module is crate-ready, leaf deps only |
| NetMux addition | Would touch CRI, API, supervisor | Lives entirely in `net/`, other modules unchanged |
| Testing | Must run as root with containers | CRI mock + Net mock → test scheduler/components without Linux |
