# Module Plan: API

> Part of [00-overview.md](./00-overview.md) · Current ~4,600 LOC (`server.rs` + `handlers/*` + `proto.rs`)

## Design principle: one group, one handler · API talks to store only

**Every resource — core/v1 Pod, apps/v1 Deployment, networking Ingress, and z8s VNet — is served by the same `z8s.io/v1` machinery.** We do not maintain parallel handler trees per upstream API group.

**API does not call CRI, NetMux, or storage provisioners.** After admission + RBAC ([07-rbac.md](./07-rbac.md)), handlers `store.apply` / `delete` and `scheduler_notify.notify_one()`. The [scheduler](./05-scheduler.md) performs all side effects.

| Anti-pattern (today) | Target |
|----------------------|--------|
| 23 files under `handlers/` | **1** `resource_handler.rs` + **1** `catalog.rs` |
| `/api/v1/pods` vs `/apis/z8s.io/v1/vnets` separate code | Same `dispatch(plural, verb, …)` |
| `crd.rs` generic helpers only for z8s CRDs | Generics for **all** kinds |
| Mixed list `apiVersion` (`v1` vs `z8s.io/v1`) | Store always `z8s.io/v1`; wire version from compat layer |

Existing `handlers/crd.rs` (`generic_list`, `generic_create_namespaced`, …) is the seed — generalize and delete the rest.

## Current state

```
api/
├── server.rs          # 1,067 LOC — monolithic router
├── proto.rs           # protobuf bodies
└── handlers/          # 23 files — DUPLICATE CRUD (to remove)
    ├── pod.rs, deployment.rs, service.rs, …   # upstream groups
    └── vnet.rs, subnet.rs, … + crd.rs        # z8s only generics
```

## Target layout (~500 LOC API)

```
api/
├── server.rs              # RouterBuilder: compat mounts + subresources
├── catalog.rs             # RESOURCE_TABLE: plural → metadata (~120 LOC)
├── resource_handler.rs    # dispatch CRUD for any catalog entry (~150 LOC)
├── compat.rs              # Path alias + apiVersion wire encode (~80 LOC)
├── admission.rs           # default · validate · normalize to z8s.io/v1
├── watch.rs               # WatchHub
├── subresource/           # log, exec, scale, status only (~120 LOC total)
│   ├── pod_log.rs
│   ├── pod_exec.rs
│   └── deployment_scale.rs
├── auth/                  # authn + authz middleware (~480 LOC) — see 07-rbac.md
│   ├── authn.rs
│   ├── authz.rs
│   └── middleware.rs
└── codec/                 # JSON + protobuf → AnyResource
```

**No** `handlers/pod.rs`, `handlers/deployment.rs`, etc.

## Resource catalog (single source of truth)

```rust
pub struct ResourceEntry {
    pub kind: &'static str,
    pub plural: &'static str,
    pub namespaced: bool,
    pub category: ResourceCategory,   // compute | network | storage
    /// Upstream apiVersion kubectl may send (for wire encoding on GET)
    pub compat_versions: &'static [&'static str],
}

pub const CATALOG: &[ResourceEntry] = &[
    entry!("Pod", "pods", true, Compute, &["v1"]),
    entry!("Service", "services", true, Network, &["v1"]),  // → EdgeService reconcile
    entry!("Deployment", "deployments", true, Compute, &["apps/v1"]),
    entry!("ConfigMap", "configmaps", true, Storage, &["v1"]),
    entry!("Secret", "secrets", true, Storage, &["v1"]),
    entry!("Namespace", "namespaces", false, Compute, &["v1"]),
    entry!("Node", "nodes", false, Compute, &["v1"]),
    entry!("PersistentVolume", "persistentvolumes", false, Storage, &["v1"]),
    entry!("PersistentVolumeClaim", "persistentvolumeclaims", true, Storage, &["v1"]),
    entry!("StorageClass", "storageclasses", false, Storage, &["storage.k8s.io/v1", "z8s.io/v1"]),
    entry!("Ingress", "ingresses", true, Network, &["networking.k8s.io/v1"]),
    entry!("NetworkPolicy", "networkpolicies", true, Network, &["networking.k8s.io/v1"]),
    entry!("Endpoints", "endpoints", true, Network, &["v1"]),
    entry!("EndpointSlice", "endpointslices", true, Network, &["discovery.k8s.io/v1"]),
    entry!("Event", "events", true, Storage, &["v1"]),
    entry!("VNet", "vnets", false, Network, &["z8s.io/v1"]),
    entry!("Subnet", "subnets", false, Network, &["z8s.io/v1"]),
    entry!("Nsg", "nsgs", false, Network, &["z8s.io/v1"]),
    entry!("RouteTable", "routetables", false, Network, &["z8s.io/v1"]),
    entry!("EdgeService", "edgeservices", true, Network, &["z8s.io/v1"]),
    entry!("Role", "roles", true, Rbac, &["rbac.authorization.k8s.io/v1", "z8s.io/v1"]),
    entry!("RoleBinding", "rolebindings", true, Rbac, &["rbac.authorization.k8s.io/v1", "z8s.io/v1"]),
    entry!("ClusterRole", "clusterroles", false, Rbac, &["rbac.authorization.k8s.io/v1", "z8s.io/v1"]),
    entry!("ClusterRoleBinding", "clusterrolebindings", false, Rbac, &["rbac.authorization.k8s.io/v1", "z8s.io/v1"]),
    entry!("ServiceAccount", "serviceaccounts", true, Rbac, &["v1", "z8s.io/v1"]),
];
```

Macro `entry!` keeps the table declarative. Discovery (`/api`, `/apis`) is **generated from `CATALOG`**, advertising **`z8s.io/v1`** as the served version for every kind.

## Generic `ResourceHandler` (replaces all CRUD handlers)

```rust
impl ResourceHandler {
    pub async fn list(state: &AppState, entry: &ResourceEntry, ns: Option<&str>, wire: WireContext) -> Response {
        let items = state.store.get_by_kind(entry.kind).await;
        // filter namespace, encode each item with wire.api_version()
    }

    pub async fn create(state: &AppState, entry: &ResourceEntry, body: Bytes, ns: Option<&str>) -> Response {
        let mut resource = decode_any(&body, entry.kind)?;
        admission::normalize(&mut resource)?;  // force apiVersion z8s.io/v1
        state.store.apply(resource.clone()).await?;
        state.gossip.broadcast(&resource).await;  // optional batch
        state.emit_store_event(StoreEvent::Applied { resource });
        state.scheduler_notify.notify_one();    // scheduler runs CRI/net/vol
        encode(resource, wire)
    }
    // get, update, patch, delete — same shape
}
```

`AnyResource::from_json_value` already exists; extend with **kind alias map**:

| Incoming kind | Canonical kind |
|---------------|----------------|
| Service | EdgeService (or Service if catalog keeps alias) |
| List | same kind + `List` suffix |

## Compat router (kubectl paths, not handlers)

One Axum route pattern per HTTP method family:

```rust
// Canonical
.route("/apis/z8s.io/v1/{plural}", …)
.route("/apis/z8s.io/v1/namespaces/{ns}/{plural}", …)

// Aliases — same handler, different WireContext
.route("/api/v1/{plural}", …)
.route("/api/v1/namespaces/{ns}/{plural}", …)
.route("/apis/apps/v1/namespaces/{ns}/{plural}", …)  // only deployments in practice
.route("/apis/networking.k8s.io/v1/namespaces/{ns}/{plural}", …)
```

`WireContext` derived from request URI:

```rust
struct WireContext {
    pub api_version: String,  // e.g. "v1", "apps/v1", "z8s.io/v1"
}

impl WireContext {
    fn from_uri(uri: &str) -> Self { /* parse first matching compat prefix */ }
    fn encode_list(&self, kind: &str, items: Vec<AnyResource>) -> Value {
        // items[].apiVersion = self.api_version for kubectl; store unchanged
    }
}
```

**Store invariant:** `resource.api_version() == "z8s.io/v1"` always after admission.

## Admission: normalize upstream YAML

Pipeline on every mutating request:

1. **Parse** — JSON/YAML/Protobuf → `AnyResource`
2. **Alias kind** — `Service` → `EdgeService` if unified
3. **Set** `apiVersion = "z8s.io/v1"`, `kind` = canonical
4. **Default** — VNet, capabilities, namespace
5. **Validate** — CIDR, names, references

```rust
fn normalize(resource: &mut AnyResource) {
    resource.set_api_version("z8s.io/v1");
}
```

## Discovery & OpenAPI

`/apis` returns **one preferred version per kind: `z8s.io/v1`**, plus optional `storedVersions` listing compat upstream strings for documentation.

```json
{
  "groupVersion": "z8s.io/v1",
  "resources": [
    { "name": "pods", "kind": "Pod", "namespaced": true },
    { "name": "deployments", "kind": "Deployment", "namespaced": true },
    { "name": "vnets", "kind": "VNet", "namespaced": false }
  ]
}
```

Legacy group paths remain routable for kubectl compiled defaults; they are **not** separate API implementations.

## Subresources (only exceptions to generic CRUD)

| Path suffix | Module | Notes |
|-------------|--------|-------|
| `/log` | `subresource/pod_log.rs` | process_tracker |
| `/exec` | `subresource/pod_exec.rs` | WebSocket → cri |
| `/scale` | `subresource/deployment_scale.rs` | patch replicas field |
| `/status` | optional | return cached status from tracker |

RBAC: all routes pass through `auth::middleware` — see [07-rbac.md](./07-rbac.md). Path → `AuthzRequest` uses **`catalog::authz_from_path`**, not hand-rolled URI tables. `Role`, `RoleBinding`, `ClusterRole`, `ClusterRoleBinding`, `ServiceAccount` are normal `z8s.io/v1` catalog entries (generic CRUD).

## Watch

Single `WatchHub` keyed by `(kind, namespace)` — no per-handler watch code.

## Types (`types.rs` shrink strategy)

- All persisted resources use `api_version: z8s.io/v1` in serde default.
- `compat_versions` is API-only metadata in catalog, not per-struct fields.
- Optional: `#[serde(alias = "v1")]` on incoming only — prefer explicit normalizer.

## Migration steps

1. **Expand `CATALOG`** with every kind currently in `handlers/*`.
2. **Route all CRUD** through `ResourceHandler` + compat aliases; delegate to existing `generic_*` in `crd.rs`.
3. **Fix** `generic_list_namespaced` to use `z8s.io/v1` list `apiVersion` when request path is z8s (today inconsistent line 110 vs 19).
4. **Move** pod status / patch logic to `components/compute/status.rs`; handler only calls `store.get` + status builder.
5. **Delete** handler files one kind at a time once routes point at catalog.
6. **Collapse** `server.rs` to `RouterBuilder::from_catalog(&CATALOG)`.
7. **Regenerate** discovery from catalog.

## LOC budget

| Component | Current | Target |
|-----------|---------|--------|
| `handlers/*` (23 files) | ~3,500 | **0** (removed) |
| `crd.rs` + new handler | 232 | 150 |
| `catalog.rs` | 0 | 120 |
| `compat.rs` | 0 | 80 |
| `server.rs` | 1,067 | 100 |
| admission + watch + subresource | 0 | 200 |
| **Total API** | **~4,600** | **~500** |

## Testing

- Table-driven: for each `CATALOG` entry, hit **three** URLs (z8s.io/v1, v1, apps/v1 if applicable) → same store key.
- Assert stored object always `apiVersion: z8s.io/v1`.
- Assert GET via `/api/v1/...` returns `apiVersion: v1` in JSON for kubectl compatibility.
- Existing `test.sh` / kubectl flows unchanged.

## Risks

- **Patch merge** must be kind-aware — one `PatchApplier` registry keyed by `kind`, not per-file handlers.
- **Protobuf create** must normalize like JSON.
- Some clients cache discovery — document that z8s.io/v1 is authoritative; aliases are compatibility.

---

*Previous: [00-overview.md](./00-overview.md) · Next: [02-cri.md](./02-cri.md)*
