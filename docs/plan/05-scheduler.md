# Module Plan: Scheduler (Control Orchestrator)

> Part of [00-overview.md](./00-overview.md) · Current ~670 LOC (`scheduler/*`) + scattered logic in `components/*`

The **scheduler module is the only runtime driver** that executes work against CRI, storage, NetMux, and the store. It **triggers and handles tasks** arising from desired state: assign pods to nodes, start/stop containers, provision volumes, apply network state, update status.

The **API talks only to the store** (and auth). It never calls CRI, NetMux, or storage provisioners directly.

---

## 1. Who talks to whom

| Component | Store | CRI | NetMux | Storage | Scheduler |
|-----------|:-----:|:---:|:------:|:-------:|:---------:|
| **API** | read/write | — | — | — | notify only |
| **Manifest watcher** | write | — | — | — | notify only |
| **Gossip / sync** | write | — | — | — | notify only |
| **Scheduler** | read/write | **yes** | **yes** | **yes** | — |
| **Init / shutdown** | read | via scheduler | via scheduler | — | coordinated stop |

```text
                    ┌─────────────┐
  kubectl ─────────►│    API      │──── write desired state
  manifest watcher ─┤  (no CRI/   │
  gossip inbound ───┤   net/vol)  │
                    └──────┬──────┘
                           │ apply + StoreEvent
                           ▼
                    ┌─────────────┐
                    │    Store    │◄──── read/write ────┐
                    └──────┬──────┘                       │
                           │ notify                       │
                           ▼                              │
                    ┌─────────────┐                       │
                    │  Scheduler  │───────────────────────┘
                    │ (orchestrator)                      │
                    └──┬───┬───┬───┘
                       │   │   │
              ┌────────┘   │   └────────┐
              ▼            ▼            ▼
           ┌──────┐   ┌────────┐   ┌─────────┐
           │ CRI  │   │ NetMux │   │ Storage │
           └──────┘   └────────┘   └─────────┘
```

**Invariant:** If a package outside `scheduler/` imports `cri::`, `netmux::` (for reconcile), or `storage::Provisioner` for side effects — that is a **violation** to remove during migration.

Allowed exceptions:

- `api/cri/exec.rs` — WebSocket **attach** may call CRI exec path as a **subresource** (thin proxy), or exec is delegated via scheduler command channel (preferred long-term).
- `node.rs` — constructs engines once and injects into `Scheduler::new(engines)`.

---

## 2. Scheduler module = orchestrator

Rename internally for clarity (optional): `scheduler::Orchestrator` — public name can stay **Scheduler**.

### 2.1 Subsystems (one crate, clear files)

```
scheduler/
├── mod.rs           # Orchestrator facade, public API
├── engines.rs       # EngineSet: cri, net, vol, store — built in node.rs, owned here
├── loop.rs          # Main select! loop: events, ticker, shutdown
├── assign.rs        # Leader: pod → node (Scheduling)
├── reconcile.rs     # Per-node: desired → actual (Reconciliation)
├── queues.rs        # Work queues from StoreEvent
├── index.rs         # SchedulerIndex + store-backed indexes
├── process.rs       # Pod lifecycle via CRI only
├── tasks.rs         # Task enum + handlers
└── leases.rs        # thin wrapper → store/leases (leader election)
```

### 2.2 Task model

Every store change (or timer) produces **tasks**; the loop drains them:

```rust
pub enum Task {
    // Scheduling (leader only)
    AssignPod { uid: String },
    ReassignPodsOnNode { node: String },
    ReconcileDeployment { uid: String },

    // Reconciliation (this node)
    SyncPod { uid: String },           // start/stop/health via CRI
    SyncNetwork,                       // netmux.reconcile(snapshot)
    ProvisionVolume { pvc_uid: String },
    DeprovisionVolume { pv_uid: String },
    UpdatePodStatus { uid: String },

    // Housekeeping
    ReapZombies,
    HeartbeatNode,
}
```

```rust
impl Orchestrator {
    pub async fn run(self) -> Result<()> {
        loop {
            tokio::select! {
                Some(event) = self.store_events.recv() => self.enqueue_from_store(event).await,
                Some(task) = self.task_rx.recv() => self.dispatch(task).await?,
                _ = self.notify.notified() => self.drain_pending().await,
                _ = self.ticker.tick() => self.enqueue_sweep().await,
                _ = self.shutdown.changed() => break,
            }
        }
        self.shutdown_engines().await
    }
}
```

**`dispatch`** is the **only** place that calls:

- `engines.cri.start_pod` / `stop_pod` / `exec`
- `engines.net.reconcile(&snapshot)`
- `engines.vol.provision` / `deprovision`
- `engines.store.apply` / `update_status` (status writes after CRI/net/storage)

### 2.3 Engine set (dependency injection)

```rust
pub struct EngineSet {
    pub store: Arc<dyn StoreBackend>,
    pub cri: Arc<dyn RuntimeProvider>,
    pub net: Arc<NetMux>,
    pub vol: Arc<dyn VolumeProvisioner>,
    pub node_name: String,
    pub is_leader: AtomicBool,
}

pub trait VolumeProvisioner: Send + Sync {
    async fn provision_pvc(&self, pvc: &PersistentVolumeClaim) -> Result<()>;
    async fn deprovision_pv(&self, pv: &PersistentVolume) -> Result<()>;
}
```

Built once in `node.rs`, moved into `Orchestrator` — **not** cloned into `ReconcileContext` for API.

### 2.4 Store events (API → scheduler)

API path ([01-api.md](./01-api.md)):

```rust
// api/resource_handler.rs — create/update/delete ONLY
store.apply(resource).await?;
store_events.emit(StoreEvent::Applied { resource, change });
scheduler_notify.notify_one();
```

No `registry.on_apply`, no `process_tracker` in `AppState` for mutations.

**Read paths** (get/list/watch) — API reads store only; optional **read-through status** from scheduler cache later, not live CRI queries per GET.

### 2.5 Scheduling vs reconciliation

| Phase | Runs on | Input | Output |
|-------|---------|-------|--------|
| **Scheduling** | Leader only | Unassigned pods, nodes, PVC binding mode | `pod.assigned_node`, gossip |
| **Reconciliation** | Every node | Store snapshot + `assigned_node == self` | CRI, NetMux, storage side effects |

Leader scheduling uses **global lease** on main store ([06-store-sync.md](./06-store-sync.md)) — workers do not run `assign.rs`.

Worker nodes: reconciliation only for local assignments.

---

## 3. Reconciliation by domain (scheduler owns all)

### 3.1 Compute (CRI) — `process.rs` + `SyncPod`

| Desired (store) | Action |
|-----------------|--------|
| Pod assigned local, not running | `cri.start_pod(spec)` |
| Pod deleted / scale down | `cri.stop_pod` |
| Container exited | update status, restart per policy |
| Probes | CRI health runner (or scheduler timer → CRI check) |

**Delete** `components/compute/pod.rs` reconcile body — logic moves here.

### 3.2 Network (NetMux) — `SyncNetwork`

| Store change | Task |
|--------------|------|
| EdgeService, VNet, Subnet, NSG, RouteTable, NetworkPolicy, local Pod | `Task::SyncNetwork` (coalesce) |

```rust
async fn sync_network(&self) -> Result<()> {
    let snap = self.engines.store.snapshot().await?;
    self.engines.net.reconcile(&snap).await?;
    Ok(())
}
```

**Delete** `NetworkManager`, `ServiceResource` net calls, `sync_services_for_labels` from anywhere outside scheduler ([04-netmux.md](./04-netmux.md)).

### 3.3 Storage — `ProvisionVolume`

| Store change | Task |
|--------------|------|
| PVC created, unbound | `ProvisionVolume` (respect StorageClass + binding mode) |
| PV delete, reclaim Delete | `DeprovisionVolume` |

**Delete** `PvcResource::on_apply` → `ctx.vol.provision` ([03-storage.md](./03-storage.md)).

Scheduler calls `VolumeBinder` before `AssignPod` when `WaitForFirstConsumer`.

### 3.4 Apps — `ReconcileDeployment`

Leader only:

- Compare deployment spec vs pod set in store (index, not `get_all`).
- Emit `store.apply` for pod creates/deletes.
- Assignment tasks follow from store events.

**Delete** `DeploymentResource::reconcile_impl` side effects — keep optional validation-only component or remove entirely.

### 3.5 Config (ConfigMap / Secret)

No CRI/net — store only. Scheduler mounts via **spec built at `SyncPod`** reading store (in `spec_builder` called from scheduler, not from API).

---

## 4. API surface (thin)

```rust
pub struct AppState {
    pub store: Arc<dyn StoreBackend>,
    pub scheduler_notify: Arc<Notify>,
    pub store_events: StoreEventTx,
    // REMOVED: process_tracker, registry, ctx with cri/net/vol
}
```

**Subresources** (exec, logs, metrics):

| Subresource | Phase 1 | Phase 2 (clean) |
|-------------|---------|-----------------|
| logs | Read ring buffer owned by scheduler/CRI | `scheduler.get_logs(pod)` |
| exec | Direct CRI (exception) | `scheduler.exec(cmd)` channel |
| metrics | Scheduler aggregates cgroup stats | same |

Document exec as temporary API exception until command queue exists.

---

## 5. Current problems (reframed)

### 5.1 Architecture violations today

| Violation | Fix |
|-----------|-----|
| `ReconcileContext { cri, net, vol, netmux }` passed to components | Move to `EngineSet` inside scheduler only |
| `registry.reconcile_all` + per-component IO | Single orchestrator loop |
| API `AppState::process_tracker` | Scheduler owns tracker |
| Manifest watcher calls `store.apply` + registry | apply + notify scheduler |
| Gossip handler starts pods | apply + notify; scheduler starts if local |

### 5.2 Performance (5× multi-node) — still valid

Root causes unchanged; fixes live in orchestrator design:

| Issue | Orchestrator fix |
|-------|------------------|
| `get_all()` every 2s | `StoreEvent` + indexes; 10s safety sweep only |
| Per-pod `sync_services_for_labels` | Single `SyncNetwork` on network dirty flag |
| Dual scheduler lease | Main-only `assign.rs` |
| Full pod scans in tick | `SchedulerIndex` incremental |
| Per-assignment gossip storm | `apply_batch` + gossip coalesce |

See §7 for metrics.

---

## 6. Indexes & store snapshot

```rust
pub struct OrchestratorIndex {
    pub pods: HashMap<Uid, PodView>,
    pub unassigned: Vec<Uid>,
    pub by_node: HashMap<NodeName, Vec<Uid>>,
    pub deployments: HashMap<Uid, DeploymentView>,
    pub network_dirty: bool,
    pub pvc_unbound: Vec<Uid>,
}

impl Orchestrator {
    fn on_store_event(&mut self, ev: StoreEvent) {
        self.index.apply(ev);
        self.enqueue_tasks_from_index();
    }
}
```

`store.snapshot()` — one read transaction per `SyncNetwork` or sweep ([06-store-sync.md](./06-store-sync.md)).

---

## 7. Multi-node flow

```text
1. API (main): POST Pod → store.apply → notify
2. Scheduler (main, leader): AssignPod → assigned_node=node-B → store.apply → gossip
3. Worker B scheduler: StoreEvent → SyncPod → cri.start_pod
4. Worker B: SyncNetwork (local backends)
5. Scheduler: UpdatePodStatus → store (gossip optional)
```

Workers **never** run assignment or deployment scale logic.

---

## 8. Shutdown (PID 1)

Orchestrator handles SIGTERM via `node.rs`:

1. Stop accepting new tasks
2. `cri.stop_all` from store pod list (timeout)
3. `net.cleanup_nft`
4. `store` flush
5. `_exit` via watchdog

Init remains separate for zombie reap; **does not** start pods.

---

## 9. LOC budget

| File | Current | Target |
|------|---------|--------|
| `scheduler/*` | ~670 | ~550 (orchestrator + tasks) |
| `components/*` reconcile | ~800 | ~150 (validation/admission only) or remove |
| `process.rs` | 329 | 140 (CRI only, no store in trait surface) |
| **Net effect** | | Centralized control, less duplication |

---

## 10. Implementation phases

| Phase | Deliverable |
|-------|-------------|
| **O0** | `EngineSet` + `Orchestrator` loop skeleton; store notify from API |
| **O1** | Move `ProcessTracker`/`start_pod` into scheduler; remove from `AppState` |
| **O2** | `SyncNetwork` only from scheduler; delete service sync from pod component |
| **O3** | PVC provision tasks; remove `PvcResource::on_apply` IO |
| **O4** | Leader-only assign; fix global lease |
| **O5** | Deployment tasks on leader; drop component reconcile |
| **O6** | Remove `ComponentRegistry::reconcile_all`; delete dead `ReconcileContext` |
| **O7** | Exec/logs via scheduler command channel (optional) |

---

## 11. Testing

- Unit: `enqueue_from_store(Pod)` → task list contains `SyncPod` iff local assignment
- Unit: leader vs worker — worker never gets `AssignPod`
- Integration: `kubectl apply` pod → only scheduler spawns CRI (strace / log assert)
- Bench: [schedule-50 script] — 2-node ≤ 1.5× single-node

---

## 12. Cross-module contracts

| Module | Contract with scheduler |
|--------|-------------------------|
| [01-api](./01-api.md) | Store write + notify; no engines in `AppState` |
| [03-storage](./03-storage.md) | `VolumeProvisioner` impl called only from `ProvisionVolume` task |
| [04-netmux](./04-netmux.md) | `NetMux::reconcile(snap)` only from `SyncNetwork` task |
| [02-cri](./02-cri.md) | `RuntimeProvider` only from `SyncPod` / shutdown |
| [06-store-sync](./06-store-sync.md) | Events + batch apply + leader lease |
| [07-rbac](./07-rbac.md) | API enforces auth **before** store write; scheduler trusts store on node |

---

*Previous: [04-netmux.md](./04-netmux.md) · Next: [06-store-sync.md](./06-store-sync.md)*
