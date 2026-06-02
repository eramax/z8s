# z8s Plan vs Implementation Review

> Review date: 2026-06-02  
> Total source LOC: ~26,547  
> Target LOC: ≤5,500  
> Plan reference: `docs/plan/00-overview.md` through `08-completion-status.md`

---

## Executive Summary

The plan is architecturally sound and the implementation has made significant progress on **Wave 1–2** (Store, Scheduler, API) and partial **Wave 7** (CRI). The core abstractions — catalog-driven API, SpawnPipeline, planner/reconciler split, join tokens — are in place. However, the codebase is **~5× over the LOC target**, several planned modules are incomplete, and critical architecture violations remain where components still call CRI/NetMux/storage directly.

---

## Wave-by-Wave Status

### Wave 0 — Baseline (Metrics)

| ID | Status | Notes |
|----|--------|-------|
| E0 | **NOT DONE** | No `bench/schedule-50.sh` found. No 1-node vs 2-node benchmark exists. |
| E0b | **PARTIAL** | Violations are documented in `00-overview.md` § P0–P4 but no formal report file. |

### Wave 1 — Store & Cluster Foundation

| ID | Status | Notes |
|----|--------|-------|
| S1a | **PARTIAL** | Scheduler lease exists (`leases.rs`) but is **not** main-only — any node can acquire. `scheduler_leader` flag checked via `config::is_scheduler_leader()` but the lease mechanism is competitive, not main-restricted. |
| S1b | **DONE** | `apply_batch` implemented for both `MemoryBackend` and `RedbBackend` (single-txn). `StoreEventHub` with broadcast channel. `StoreSnapshot` exists. |
| S1c | **DONE** | Gossip coalesces by key (`pending` HashMap), batches up to 32 entries, flushes every 100ms. |
| J1 | **DONE** | `JoinTokenRecord` in `join_tokens.rs` (257 LOC). `z8s node <name> token` CLI works. WS Bearer auth implemented. Token rotation supported. |

### Wave 2 — Orchestrator Hub

| ID | Status | Notes |
|----|--------|-------|
| O0 | **PARTIAL** | `Orchestrator` loop exists (`orchestrator.rs`, 342 LOC). `EngineSet` defined but **never instantiated** — dead code. Orchestrator uses `ReconcileContext` from components instead. |
| O1 | **PARTIAL** | API notifies scheduler via `scheduler_notify`. But `AppState` still holds `process_tracker`, `registry`, `ctx` with CRI/NetMux. |
| O2 | **PARTIAL** | `ProcessTracker` moved to scheduler. `sync_pod.rs` exists. But `components/compute/spec_builder.rs` (662 LOC) still calls CRI types directly. |
| P1a | **NOT DONE** | `sync_services_for_labels` still exists conceptually via component `ServiceResource`. |
| P1b | **PARTIAL** | `OrchestratorIndex` exists (109 LOC) with pod tracking. But `get_all()` still used in full sweeps every 15 ticks. |
| P1c | **PARTIAL** | Leader-only `AssignPod` gated on `config::is_scheduler_leader()`. But lease is competitive, not main-only. Deployment scale still runs from `DeploymentResource` component, not scheduler tasks. |

### Wave 3 — NetMux

| ID | Status | Notes |
|----|--------|-------|
| N0 | **DONE** | `state.rs` exists with `RuleKey`, `TableId`, chain layout. |
| N1 | **DONE** | `planner.rs` (493 LOC) — pure `NetworkPlanner::plan()` from `StoreSnapshot`. |
| N2 | **PARTIAL** | `applier/netlink/` split exists (5 files, ~540 LOC). But `applier/nft.rs` doesn't exist — nft logic remains in monolithic `nftables.rs`. |
| O3 | **PARTIAL** | `sync_network.rs` (349 LOC) exists as reconcile entry point. But `NetworkManager` and direct netmux calls from components still exist. |
| N3 | **PARTIAL** | `planner.rs` handles EdgeService/ClusterIP DNAT. But `nftables.rs` still has `jump_track`/`nodeport_jump_track` (plan says to remove). No userspace TCP proxy removal confirmed. |
| N4 | **NOT DONE** | NSG/NetworkPolicy still uses `np_controller.rs` (230 LOC) imperative path, not planner diff. |
| N5 | **PARTIAL** | DNS exists (`dns.rs`, 422 LOC) but `NetworkState.dns` not yet driven by planner output in a clean way. |
| N6 | **NOT DONE** | Remote pod routes partially in planner (`planned_pod_routes`) but no gossip-based cross-node route propagation. |

### Wave 4 — Storage

| ID | Status | Notes |
|----|--------|-------|
| SC1 | **PARTIAL** | `class.rs` seeds default StorageClasses into store. But `default_storage_classes()` returns **hardcoded** structs, not store-fetched. |
| SC2 | **NOT DONE** | No `ProvisionerRegistry` — uses `ProvisionerDispatcher` with hardcoded match arms. Adding a provisioner requires editing source. |
| SC3 | **NOT DONE** | No `WaitForFirstConsumer` scheduler bind hook. |

### Wave 5 — In-cluster API + RBAC

| ID | Status | Notes |
|----|--------|-------|
| R0 | **DONE** | `kubernetes` EdgeService + DNS `kubernetes.default.svc.cluster.local` via bootstrap. |
| R1 | **PARTIAL** | Authz engine exists but role lookup key ordering bug mentioned in plan is unclear if fixed. Catalog path resolver exists (`authz_from_path`). |
| R2 | **PARTIAL** | GET/list/watch enforcement mentioned in `authorize_middleware_with_store` but completeness unverified. |
| R3 | **DONE** | ClusterRole + ClusterRoleBinding in catalog. |
| R4 | **DONE** | SA token + kubeconfig mount in `auth/token.rs`. |
| R5 | **PARTIAL** | SSAR uses real `authorize` call. |
| R6 | **DONE** | Multi-doc apply authz in `auth/apply.rs`. |
| R8 | **NOT DONE** | No `test-rbac-cluster-dashboard.sh` found. |

### Wave 6 — API Surface Shrink

| ID | Status | Notes |
|----|--------|-------|
| A1 | **DONE** | `catalog.rs` (543 LOC) with `ResourceEntry`, `CATALOG` (24 kinds), `COMPAT_MOUNTS`. |
| A2 | **DONE** | `resource_handler.rs` (366 LOC) generic CRUD. Legacy handlers reduced to 5 files (apply, crd, metrics, rbac, system). |
| A3 | **PARTIAL** | Watch + discovery exist but discovery has hardcoded system routes alongside catalog. |
| A4 | **PARTIAL** | `types/` split into 16 files (~3,100 LOC). No `k8s-openapi` used. But types are still verbose. |

### Wave 7 — CRI Performance

| ID | Status | Notes |
|----|--------|-------|
| C1 | **DONE** | `SpawnPipeline` in `cri/spawn/` (10 files, ~1,113 LOC). Steps-based pipeline with `SpawnStep` trait. |
| C2 | **PARTIAL** | `image_store.rs` dispatches to overlay or copy. Overlay logic in `image.rs`. Config-driven via `--overlay-rootfs`. |
| C3 | **DONE** | `capability.rs` (78 LOC) with 5 profiles. Annotation-based. |
| — | **PARTIAL** | `probe_runner.rs` extracted. `exec_protocol.rs` and `exec_query.rs` extracted. But `exec.rs` is still 918 LOC monolithic. |

### Wave 8 — Optional & Hardening

| ID | Status | Notes |
|----|--------|-------|
| S3 | **NOT DONE** | No S3 StorageClass support anywhere. |
| Z1 | **DONE** | PID 1 auto-run: `main.rs` checks `getpid() == 1`, calls `run_daemon_server`. Full `InitHandler` in `init.rs`. |
| H1 | **NOT DONE** | No PID soak, D-state chaos, or partition tests. |
| H3 | **NOT DONE** | No nested z8s E2E test. |
| H2 | **NOT DONE** | LOC is 26,547 — far over the 5,500 target. |

---

## Critical Issues

### 1. EngineSet is Dead Code

`scheduler/engines.rs` defines `EngineSet` but it is **never constructed**. The orchestrator uses `ReconcileContext` from `components/mod.rs` instead. This is the planned dependency injection boundary that was never wired in.

**Impact:** Architecture violation — components still receive CRI/NetMux/storage directly.

### 2. Components Still Call CRI/NetMux/Storage Directly

The plan says: "If a package outside `scheduler/` imports `cri::`, `netmux::` (for reconcile), or `storage::Provisioner` for side effects — that is a **violation**."

Actual violations:

| Component | Violation | Severity |
|-----------|-----------|----------|
| `DeploymentResource` | Calls `scheduler::sync_pod::stop_pod_local` + `ctx.netmux` | **High** |
| `spec_builder.rs` | Imports CRI types, calls `cri::capability`, `cri::volumes` | **High** |
| `ServiceResource` / `NetworkManager` | Calls `netmux::sync_network::remove_service` | Medium |
| `IngressResource` | Directly mutates `ctx.netmux.ingress_state` / `dns_records` | **High** |
| `PvcResource` / `PvResource` | Calls `ctx.vol.deprovision_pv` | Low |

### 3. Scheduler Lease is Not Main-Only

The plan says: "Scheduler lease only on **main** redb; workers skip `assign` loop."

Currently, `run_lease_loop` runs on **every** node with competitive acquisition. Any node can become leader. This is standard distributed locking, but the plan explicitly wanted main-only assignment to avoid split-brain.

### 4. `netmux/mod.rs` is Monolithic

At 797 LOC, `mod.rs` still contains the `NetMux` struct with all facade methods, declarative types (`NftRule`, `NetworkState`, etc.), and veth/route helpers. The plan called for:
- `state.rs` for types (~140 LOC)
- `mod.rs` as thin facade (~80 LOC)

Current `state.rs` is only 53 LOC (just `RuleKey` and `TableId`), while `NetworkState` lives in `mod.rs`.

### 5. `jump_track` / `nodeport_jump_track` Still Exist

`nftables.rs` (479 LOC) still uses `jump_track` and `nodeport_jump_track` to track installed jump rules. The plan (N2) explicitly says "delete `jump_track`" — jumps should be derived from `RuleKey` set diff.

### 6. No NFT Applier Module

The plan called for `applier/nft.rs` (~220 LOC) to compile `NftRule` → `rustables::Rule` and batch-apply. This doesn't exist. All nft logic is in `nftables.rs`.

### 7. exec.rs is Still Monolithic

At 918 LOC, `exec.rs` was supposed to be split into:
- `exec/protocol.rs` (~200 LOC)
- `exec/session.rs` (~150 LOC)

Only `exec_protocol.rs` (101 LOC) and `exec_query.rs` (51 LOC) were extracted. The core PTY/pipe/namespace/WebSocket logic remains in one file.

### 8. `np_controller.rs` Still Uses Imperative Path

The plan says NSG/NetworkPolicy should go through the planner diff. Currently `np_controller.rs` (230 LOC) imperatively compiles NetworkPolicy to nft sets, separate from the planner.

---

## Dead Code / Unnecessary Files

| File | LOC | Status |
|------|-----|--------|
| `scheduler/engines.rs` | 17 | Dead — `EngineSet` never constructed |
| `scheduler/reconciler.rs` | 1 | Trivial re-export — could inline to `mod.rs` |
| `netmux/ipv6.rs` | 96 | Stub/partial — no production use |
| `netmux/netlink.rs` | 3 | Thin re-export shim — could remove |
| `components/network/subnet.rs` | 35 | No-op reconciler with dead `_netmux` param |
| `components/network/vnet.rs` | 35 | No-op reconciler with dead `_netmux` param |
| `components/network/networkpolicy.rs` | 36 | No-op reconciler with dead `_netmux` param |
| `components/network/routetable.rs` | 35 | No-op reconciler with dead `_netmux` param |
| `components/network/nsg.rs` | 35 | No-op reconciler with dead `_netmux` param |
| `components/storage/configmap.rs` | 40 | No-op stubs |
| `components/storage/secret.rs` | 40 | No-op stubs |
| `api/types.rs` | 5 | Trivial re-exports |

**Total dead/unnecessary:** ~472 LOC

---

## LOC Analysis

| Module | Current LOC | Plan Target | Status |
|--------|------------|-------------|--------|
| `api/` | ~3,500 | ~500 | 7× over — enrich/ still has per-kind handlers |
| `cri/` | ~5,274 | ~1,200 | 4.4× over — `rootfs.rs` (1,023) and `exec.rs` (918) unsplit |
| `netmux/` | ~3,881 | ~1,000 | 3.9× over — `mod.rs` (797) monolithic |
| `scheduler/` | ~1,391 | ~550 | 2.5× over — dual scheduler (tick + orchestrator) |
| `store/` | ~1,751 | ~590 | 3× over — `ws.rs` (389) large |
| `components/` | ~1,933 | ~150 | 12.9× over — most are no-op stubs |
| `storage/` | ~618 | ~350 | 1.8× over |
| `types/` | ~3,100 | N/A | Large but needed |
| `main.rs` | 960 | 200 | 4.8× over |
| `node.rs` | 561 | N/A | Wiring complexity |
| `config.rs` | 456 | N/A | — |
| `init.rs` | 97 | N/A | Clean |
| `bootstrap/` | ~264 | N/A | — |
| **Total** | **~26,547** | **≤5,500** | **4.8× over target** |

---

## What Matches the Plan Well

1. **Catalog-driven API** — `catalog.rs` + `resource_handler.rs` + `catalog_routes.rs` is clean and matches the plan's single-handler vision. 24 kinds registered.

2. **SpawnPipeline** — The `cri/spawn/` module (10 files) implements the step-based pipeline from the plan. `SpawnStep` trait, `SpawnState`, strategies for root_ns and user_ns.

3. **NetworkPlanner** — Pure-logic planner (`planner.rs`, 493 LOC) that reads `StoreSnapshot` and produces desired network intents. Matches the plan's L3 planner design.

4. **Join Tokens** — Full implementation: `JoinTokenRecord`, wire format `z8s.jt.*`, SHA-256 hashing, CLI, WS auth. Matches plan § Node join tokens exactly.

5. **Gossip Coalescing** — `GossipState.pending` coalesces by key, batches up to 32, flushes every 100ms. Matches plan § Gossip protocol v2 Phase 1.

6. **StoreSnapshot** — Exists with `from_trackers`, `by_kind`, `filter_uids`. Used by planner and orchestrator.

7. **Capability Profiles** — 5 profiles in `capability.rs`, annotation-based. Matches plan § 4.

8. **Z1 PID 1** — Full implementation with `InitHandler`, subreaper, zombie reaping. Matches plan § 7b.

9. **RBAC Auth** — SA tokens, kubeconfig mount, multi-doc apply authz, catalog path resolver. Partial but functional.

---

## What Doesn't Match / Is Missing

| Plan Requirement | Status | Gap |
|-----------------|--------|-----|
| Scheduler lease main-only | Not implemented | Competitive lease on all nodes |
| EngineSet wired in | Dead code | Orchestrator uses ReconcileContext |
| Components don't call CRI/NetMux | Violated | 5 high/medium violations |
| `applier/nft.rs` | Missing | NFT logic in monolithic `nftables.rs` |
| `jump_track` removed | Still exists | Plan N2 requires removal |
| `NetworkManager` deleted | Still exists | Stub in `components/network/service.rs` |
| `sync_services_for_labels` deleted | Still exists conceptually | Via component path |
| ProvisionerRegistry | Missing | Hardcoded match in `ProvisionerDispatcher` |
| S3 StorageClass | Not started | Plan Phase S |
| `test-rbac-cluster-dashboard.sh` | Not created | Plan R8 |
| Nested z8s E2E test | Not created | Plan H3 |
| Benchmark script | Not created | Plan E0 |
| 5,500 LOC target | 26,547 | 4.8× over |
| `exec.rs` split to session.rs | Not done | 918 LOC monolithic |
| `np_controller` via planner diff | Imperative path | Not integrated |

---

## Recommendations

### Immediate (unblock architecture)

1. **Wire `EngineSet` into orchestrator** — Construct in `node.rs`, pass to `Orchestrator`. Remove `ReconcileContext` from component trait surface.

2. **Remove component CRI/NetMux violations** — `DeploymentResource` should emit store events, not call `stop_pod_local`. `IngressResource` should not mutate `NetMux` internal state.

3. **Delete no-op network components** — `subnet.rs`, `vnet.rs`, `networkpolicy.rs`, `routetable.rs`, `nsg.rs` (176 LOC of stubs).

### Short-term (LOC reduction)

4. **Split `exec.rs`** into `exec/session.rs` (PTY + pipe + WebSocket loops) and keep `exec.rs` as thin handler.

5. **Extract `state.rs` types from `netmux/mod.rs`** — Move `NetworkState`, `NftRule`, `NftAction`, `VethSpec`, `RouteSpec` to `state.rs`.

6. **Delete dead code** — `EngineSet`, `ipv6.rs` stubs, `_netmux` params, trivial re-exports (~472 LOC).

7. **Remove dual scheduler** — Keep only `orchestrator.rs`. Remove `scheduler.rs` (tick-based legacy).

### Medium-term (plan completion)

8. **Implement `applier/nft.rs`** — Compile `NftRule` → `rustables::Rule`, batch-apply, remove `jump_track`.

9. **Make lease main-only** — Only main node runs `run_lease_loop`.

10. **Create benchmark and E2E tests** — `bench/schedule-50.sh`, `test-rbac-cluster-dashboard.sh`, `test-z8s-nested-pid1.sh`.

---

## Verdict

**The plan is correct and well-designed.** The implementation has built the right abstractions in the right order. The main gaps are:

- **Wiring** — `EngineSet` exists but isn't connected; components bypass the orchestrator
- **Cleanup** — Dead code, no-op stubs, and legacy dual scheduler add ~3,000 LOC
- **Completion** — N4–N6 (netmux), SC2–SC3 (storage), H1–H3 (hardening) not started
- **LOC** — At 26,547, the codebase is 4.8× over target; major deletion pass needed

The architecture is sound. The remaining work is primarily **deletion and rewiring**, not new feature design.
