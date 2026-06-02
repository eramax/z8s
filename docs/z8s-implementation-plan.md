# z8s v3 Implementation Plan — Phase 0 & 1: Generic Types + Handlers

## Approach: Declarative Macros + Enhanced Generic CRUD

**Why not `Resource<S>` generic?** The `#[serde(tag = "resourceType")]` wire format requires the enum. Generic wrappers break kubectl compatibility.

**Why not trait-based?** Still need the enum, so match arms remain. Marginal savings for significant complexity.

**Chosen:** Declarative `macro_rules!` to generate `AnyResource` enum + methods + defaults, plus enhanced `crd.rs` generic CRUD helpers.

## Implementation Steps

### Step 0.1 — Delete `StoredResource` dead code
- File: `src/types.rs` (~lines 2600-2750)
- ~150 lines, never used anywhere

### Step 0.2 — Add `define_kube_defaults!` macro
- File: `src/types.rs` (near top)
- Replace 38 hand-written `default_*` functions with 2 macro invocations
- Standard k8s types: `define_kube_defaults!` generates api_version + kind functions
- CRDs: `define_crd_defaults!` generates kind functions only (shared `default_api_version`)

### Step 0.3 — Add `define_any_resource!` macro
- File: `src/types.rs` (before AnyResource)
- Generates: enum definition, `metadata()`, `metadata_mut()`, `kind()`, `name()`, `namespace()`, `uid()`, `from_yaml_value()`, `from_json_value()`
- Delete old hand-written AnyResource enum + all methods
- Two categories: `namespaced: [...]` and `cluster_scoped: [...]`

### Step 0.4 — Simplify `parse_manifest_yaml`
- File: `src/types.rs`
- Use `AnyResource::from_yaml_value()` instead of 13-arm match

### Step 1.1 — Add generic CRUD helpers to `crd.rs`
- File: `src/api/handlers/crd.rs`
- Add: `generic_list_namespaced`, `generic_get_namespaced`, `generic_update_namespaced`, `generic_delete_namespaced`

### Steps 1.2-1.6 — Migrate handlers one at a time
- configmap.rs: 173 -> ~45 lines
- ingress.rs: 187 -> ~55 lines
- networkpolicy.rs: 192 -> ~55 lines
- pvc.rs: 130 -> ~45 lines
- pv.rs: 120 -> ~35 lines

### Steps 1.7-1.8 — Slim service.rs and secret.rs
- service.rs: Keep business logic, replace CRUD boilerplate (~275 -> ~170)
- secret.rs: Keep stringData encoding, replace CRUD boilerplate (~185 -> ~100)

### Handlers left unchanged (too much unique logic):
- pod.rs, deployment.rs, namespace.rs, node.rs, endpoints.rs, endpointslices.rs, event.rs, storage_class.rs, system.rs, metrics.rs

## Verification
- `cargo build` after every step
- `cargo test` after every step
- kubectl compatibility preserved (same serde wire format)
