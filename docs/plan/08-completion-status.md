# Plan completion status (v10 branch)

> Snapshot: 2026-06-02. Full modernization to ≤5k LOC and full cloud parity is a multi-month epic; this tracks wave exit criteria from [00-overview.md](./00-overview.md).

## Done on v10 (implementation)

| Wave | IDs | Status |
|------|-----|--------|
| API | A1–A2 | `catalog.rs`, `resource_handler.rs`, `catalog_routes`, compat mounts; legacy `handlers/*` reduced to system/metrics/apply/rbac |
| API | A3 | Watch + discovery via catalog (partial) |
| API | A4 | `types/*` split; no `k8s-openapi` (explicit skip) |
| CRI | C1 | `SpawnPipeline`: volumes, rootfs, merge env, pipes; `root_ns` / `user_ns` / `entrypoint` |
| CRI | C2 | `image_store`, `--overlay-rootfs`, unmount on stop |
| CRI | C3 | `capability.rs`, `z8s.io/cap-profile`, host-native default, Landlock skip per profile |
| CRI | — | `probe_runner`, `exec_query`, `exec_protocol` |
| Scheduler | P1 partial | `pick_least_loaded`, parallel WFFC PVC provision, `pod_start_parallelism`, exited-container lookup via `get_by_kind(Pod)` |
| RBAC | R0 | `kubernetes` Service + DNS via bootstrap + planner |
| RBAC | R1–R3 | Catalog authz paths, ClusterRole/Bindings, deny when policy exists |
| RBAC | R4–R5 | SA token + kubeconfig mount; SSAR uses real `authorize` |
| RBAC | R6 | Multi-doc apply authz |
| Store/O | O0 partial | Event-driven orchestrator + `StoreSnapshot` incremental reconcile |
| Z1 | — | PID 1 with no args → `run --daemon` foreground |

## Remaining (deferred epics)

| Area | Work | Notes |
|------|------|-------|
| LOC | H2 | ~22k → ~5.5k needs major deletion/generics; not a single PR |
| Scheduler | P0 metrics | 2-node 50-pod ≤1.2× single-node — needs lease leader-only + batch store writes |
| NetMux | N1–N6 | Declarative planner-only path; route table reconcile beyond stub |
| Storage | SC3+ | S3 class optional feature |
| CRI | exec session | `exec_ws_*` still in `exec.rs` (~800 LOC); split to `exec/session.rs` |
| CRI | Spawn steps | Post-fork net/cgroup as `SpawnStep`s |
| RBAC | R8 CI | `tests/test-rbac-cluster-dashboard.sh` needs live cluster |
| H3 | Nested z8s | `tests/test-z8s-nested-pid1.sh` + image `z8s:dev` |
| task7 | Roadmap | See [task7/00-overview.md](./task7/00-overview.md) |

## Test gates (manual / CI)

```bash
cargo build
cargo test
# optional cluster:
# tests/test-rbac-cluster-dashboard.sh
# tests/test-z8s-nested-pid1.sh
```
