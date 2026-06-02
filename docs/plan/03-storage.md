# Module Plan: Storage

> Part of [00-overview.md](./00-overview.md) · Current ~495 LOC (`storage/*` + PVC/PV components)

## Principles

1. **Follow Kubernetes `StorageClass` semantics** — `storage.k8s.io/v1` on the wire; objects live in the store like any other resource ([01-api.md](./01-api.md) generic handler).
2. **Provisioner string drives behavior** — no hardcoded class list in Rust; lookup `StorageClass` by name from the store.
3. **S3 is a later StorageClass** — same PVC/PV flow; new provisioner `z8s.io/s3` with standard `parameters` (bucket, endpoint, credentials secret).

## Current state

```
storage/
├── mod.rs           # ProvisionerDispatcher + hardcoded StorageClass::builtin()
├── loop_prov.rs     # z8s.io/loop
└── hostpath.rs      # z8s.io/hostpath

api/handlers/storage_class.rs   # GET only — builtins not in store
components/storage/pvc.rs       # provision_for_pvc on apply
```

### Gaps

| Gap | Target |
|-----|--------|
| StorageClass not in store | Full CRUD via catalog; seed defaults at bootstrap |
| `StorageClass::by_name` hardcoded | `store.get("StorageClass", name)` |
| No default class | Support `storageclass.kubernetes.io/is-default-class: "true"` |
| Handler only lists builtins | Same `ResourceHandler` as PV/PVC |
| No standard parameters | Per-provisioner validation from SC `parameters` |
| S3 | Planned provisioner plugin (Phase S) |

## Kubernetes-standard StorageClass

### API shape (keep `types::StorageClass` aligned with upstream)

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: standard
  annotations:
    storageclass.kubernetes.io/is-default-class: "true"
provisioner: z8s.io/loop
parameters: {}                    # provisioner-specific
reclaimPolicy: Delete             # Delete | Retain
volumeBindingMode: Immediate      # Immediate | WaitForFirstConsumer
allowVolumeExpansion: false
mountOptions: []                  # passed to mount(8)
```

Compat paths (same handler): `/apis/storage.k8s.io/v1/storageclasses` and `/apis/z8s.io/v1/storageclasses` after normalization.

### PVC → StorageClass → PV flow (standard)

```text
PVC.spec.storageClassName  →  StorageClass (store)
                           →  provisioner field
                           →  Provisioner plugin
                           →  PV created + claimRef
                           →  PVC.spec.volumeName + status.phase=Bound
```

If `storageClassName` omitted: use default class annotation, else no dynamic provision (static PV bind only).

### Built-in provisioners (seed manifests, not code)

Ship `/etc/z8s/manifests/storage/` (or create on first boot):

| Name | provisioner | Use case |
|------|-------------|----------|
| `standard` (default) | `z8s.io/loop` | Sized block volumes, RWO |
| `hostpath` | `z8s.io/hostpath` | Restricted host dir bind |
| `local-path` | `z8s.io/hostpath` | Alias with stricter parameters |

Users add custom classes via `kubectl apply` — same as upstream k8s.

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: fast-loop
provisioner: z8s.io/loop
parameters:
  fsType: ext4
reclaimPolicy: Delete
volumeBindingMode: WaitForFirstConsumer
```

## Target architecture

```
┌──────────────────────────────────────────────────────────┐
│ StorageFacade                                             │
│  · resolve_class(pvc) → StorageClass from store           │
│  · default_class() → annotation scan                      │
├──────────────────────────────────────────────────────────┤
│ ProvisionerRegistry: provisioner string → plugin          │
│   z8s.io/loop      → LoopProvisioner                      │
│   z8s.io/hostpath  → HostPathProvisioner                  │
│   z8s.io/s3        → S3Provisioner (Phase S, feature)     │
├──────────────────────────────────────────────────────────┤
│ VolumeBinder (scheduler) · DiskQuota · admission validate   │
└──────────────────────────────────────────────────────────┘
```

### `StorageProvisioner` plugin trait

```rust
#[async_trait]
pub trait ProvisionerPlugin: Send + Sync {
    fn provisioner_id(&self) -> &'static str;
    /// Validate StorageClass.parameters at admission (optional SC create)
    fn validate_parameters(&self, params: &BTreeMap<String, String>) -> Result<()>;
    async fn provision(&self, pvc: &PersistentVolumeClaim, class: &StorageClass) -> Result<PersistentVolume>;
    async fn deprovision(&self, pv: &PersistentVolume, class: &StorageClass) -> Result<()>;
    fn node_affinity(&self, pv: &PersistentVolume, node: &str) -> bool;
}
```

Register at startup:

```rust
registry.register("z8s.io/loop", Arc::new(LoopProvisioner));
registry.register("z8s.io/hostpath", Arc::new(HostPathProvisioner));
#[cfg(feature = "s3")]
registry.register("z8s.io/s3", Arc::new(S3Provisioner));
```

**Remove** `StorageClass::builtin()` / `by_name()` from `storage/mod.rs`.

### Resolve StorageClass (replaces hardcoded lookup)

```rust
pub async fn resolve_for_pvc(store: &dyn StoreBackend, pvc: &PersistentVolumeClaim) -> Result<Option<StorageClass>> {
    let name = pvc.spec.as_ref()
        .and_then(|s| s.storage_class_name.clone())
        .or_else(|| default_storage_class_name(store).await);
    if let Some(name) = name {
        return Ok(store.get_storage_class(&name).await);
    }
    Ok(None)
}

async fn default_storage_class_name(store: &dyn StoreBackend) -> Option<String> {
    store.get_by_kind("StorageClass").await
        .into_iter()
        .find(|t| t.resource.annotations()
            .get("storageclass.kubernetes.io/is-default-class")
            == Some("true"))
        .map(|t| t.resource.name())
}
```

### Admission (StorageClass create/update)

| Check | Rule |
|-------|------|
| `provisioner` | Must be registered in `ProvisionerRegistry` (or allow unknown for import-only) |
| `reclaimPolicy` | `Delete` \| `Retain` |
| `volumeBindingMode` | `Immediate` \| `WaitForFirstConsumer` |
| `parameters` | Delegate to `plugin.validate_parameters` |
| Default class | At most one `is-default-class: "true"` |

### Loop provisioner (`z8s.io/loop`)

| StorageClass field | Usage |
|--------------------|--------|
| `parameters.fsType` | `ext4` (default), `xfs` |
| PVC `resources.requests.storage` | loop file size (required) |
| `mountOptions` | appended to mount(8) |
| `reclaimPolicy: Retain` | keep file under `/var/lib/z8s/pv/` on PVC delete |

### HostPath provisioner (`z8s.io/hostpath`)

| Parameter | Meaning |
|-----------|---------|
| `basePath` | Under `/var/lib/z8s/host-volumes/` (allowlist) |

Admission rejects absolute paths outside allowlist.

### Volume binding modes (standard k8s)

| `volumeBindingMode` | Behavior |
|---------------------|----------|
| `Immediate` | Provision on PVC create (current) |
| `WaitForFirstConsumer` | PVC `Pending` until pod scheduled; provision on that node ([05-scheduler.md](./05-scheduler.md)) |

PV gets standard `nodeAffinity` when node-local:

```yaml
nodeAffinity:
  required:
    nodeSelectorTerms:
      - matchExpressions:
          - key: kubernetes.io/hostname
            operator: In
            values: [node-6444]
```

## Phase S — S3 StorageClass (later)

**Goal:** Any S3-compatible bucket (AWS S3, MinIO, Wasabi, etc.) as backing store for PVCs via a normal StorageClass — no special-case API.

### StorageClass example

```yaml
apiVersion: storage.k8s.io/v1
kind: StorageClass
metadata:
  name: s3-shared
provisioner: z8s.io/s3
parameters:
  bucket: my-app-data
  region: us-east-1
  # optional — omit for AWS default endpoint
  endpoint: https://s3.amazonaws.com
  prefix: z8s/volumes/
  # credentials via Secret (namespace-scoped reference)
  secretName: s3-credentials
  secretNamespace: default
  # mount implementation
  mountType: mountpoint   # mountpoint (default) | s3fs
reclaimPolicy: Delete
volumeBindingMode: Immediate
mountOptions:
  - allow-delete
  - allow-overwrite
```

### Credentials Secret (standard `v1/Secret`)

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: s3-credentials
  namespace: default
type: Opaque
stringData:
  accessKeyID: ...
  secretAccessKey: ...
  # optional sessionToken for STS
```

Provisioner reads Secret at provision time; never stores keys in PV labels.

### PV / mount model

S3 is **object storage**, not block — document constraints:

| Access mode | Support |
|-------------|---------|
| ReadWriteOnce | Yes (single node mount) |
| ReadOnlyMany | Yes (multiple pods, same node or shared mount policy) |
| ReadWriteMany | Phase S+ (requires consistent FUSE + cluster coordination) |

**Implementation options** (pick one for v1 of S3 class):

| `mountType` | Pros | Cons |
|-------------|------|------|
| `mountpoint` (mountpoint-s3) | Performance, POSIX-ish | Linux only, blob store |
| `s3fs` | Widely known | Slower, FUSE |

Per-PV path: `s3://bucket/prefix/<pv-uuid>/` mounted into pod at CRI volume mount.

### Provisioner ID

Use **`z8s.io/s3`** (consistent with `z8s.io/loop`). Optional alias `s3.csi.z8s.io` in docs for users migrating from CSI manifests — same plugin.

### Cargo feature

```toml
[features]
default = []
s3 = ["aws-sdk-s3", "mountpoint", ...]  # keeps edge binary small without S3
```

### S3 implementation phases

| Step | Deliverable |
|------|-------------|
| S0 | `validate_parameters` + SC admission only; provision returns clear "not enabled" |
| S1 | Provision: create prefix in bucket, PV with CSI-like volume attributes |
| S2 | CRI mount via mountpoint-s3 in pod mount namespace |
| S3 | Deprovision: delete prefix (respect `reclaimPolicy: Retain`) |
| S4 | MinIO/custom `endpoint` tests |

### Not in scope for S3 v1

- Bucket auto-creation (bucket must exist)
- Versioning/lifecycle (ops concern)
- EBS/Azure disk (separate provisioners, same StorageClass pattern)

## PV / PVC / StorageClass in catalog

| Kind | API | In store | Handler |
|------|-----|----------|---------|
| StorageClass | `storage.k8s.io/v1` | yes | generic CRUD |
| PersistentVolume | `v1` | yes | generic CRUD |
| PersistentVolumeClaim | `v1` | yes | generic CRUD + reconcile |

Delete `api/handlers/storage_class.rs`; register in catalog like [01-api.md](./01-api.md).

Bootstrap: ensure default `standard` StorageClass exists (same pattern as default VNet).

## Scheduler integration ([05-scheduler.md](./05-scheduler.md))

Storage plugins are **not** called from API or `PvcResource::on_apply`.

| Task | When | Action |
|------|------|--------|
| `ProvisionVolume` | PVC unbound in store event | `engines.vol.provision_pvc` |
| `DeprovisionVolume` | PV deleted, reclaim Delete | `engines.vol.deprovision_pv` |
| Before `AssignPod` | `volumeBindingMode: WaitForFirstConsumer` | provision on chosen node, then assign |

`StorageClass` objects are store-only; API apply + notify scheduler.

ConfigMap/Secret stay separate (see [01-api.md](./01-api.md)); S3 only **references** Secrets.

## Disk isolation (unchanged goals)

- Loop: size from PVC request; `nosuid,nodev` on mount
- Optional cgroup `io.max` on loop device
- hostPath: allowlist only

## LOC budget

| File | Current | Target |
|------|---------|--------|
| storage/mod.rs | 266 | 100 (facade + registry) |
| loop_prov.rs | 185 | 110 |
| hostpath.rs | 44 | 45 |
| storage_class.rs (handler) | 63 | 0 (removed) |
| class_resolve.rs (new) | 0 | 50 |
| s3_prov.rs (feature) | 0 | ~200 (Phase S) |
| **Total storage/** | **~495** | **~350** (+ S3 behind feature) |

## Implementation order

1. **StorageClass in store** — seed `standard` + `hostpath`; migrate handler to catalog CRUD.
2. **ProvisionerRegistry** — resolve class from store; remove `builtin()`.
3. **Default class** — annotation `is-default-class`.
4. **Admission** — validate provisioner + parameters.
5. **Loop size + hostPath allowlist** from PVC/SC parameters.
6. **WaitForFirstConsumer** + scheduler hook.
7. **Phase S** — `z8s.io/s3` behind `feature = "s3"`; docs + example manifests.

## Testing

- `kubectl get storageclass` lists store objects (not hardcoded handler only).
- PVC with `storageClassName: standard` → Bound PV, loop size ≈ request.
- PVC without class + default annotation → uses `standard`.
- Unknown provisioner on SC create → admission rejected.
- (Phase S) MinIO StorageClass + Secret → pod sees mounted prefix; deprovision deletes prefix.

## Example: user-defined class (standard k8s)

```yaml
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: data
  namespace: default
spec:
  accessModes: [ReadWriteOnce]
  storageClassName: s3-shared    # after Phase S
  resources:
    requests:
      storage: 10Gi              # logical quota / prefix sizing hint
```

---

*Previous: [02-cri.md](./02-cri.md) · Next: [04-netmux.md](./04-netmux.md)*
