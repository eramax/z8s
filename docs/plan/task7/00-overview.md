# z8s modernization roadmap (task7 index)

> Requested in `tasks/task7.md`: production-grade, faster than k3s, cleaner NetMux, ~5k LOC, unified VNet, PID 1, extended CRDs.

This directory indexes the **module plans** already written under `docs/plan/`. Implementation proceeds in [wave order](../00-overview.md#implementation-order); do not duplicate full specs here.

## Module plans

| Module | Document | Primary goals |
|--------|----------|----------------|
| API | [01-api.md](../01-api.md) | Single catalog + generic handler; compat routes; ~500 LOC API |
| CRI | [02-cri.md](../02-cri.md) | SpawnPipeline, overlay, capability profiles, exec split |
| Storage | [03-storage.md](../03-storage.md) | StorageClass registry, WFFC, optional S3 |
| NetMux | [04-netmux.md](../04-netmux.md) | Declarative `NetworkState`, planner/applier, clean nft chains |
| Scheduler | [05-scheduler.md](../05-scheduler.md) | Sole CRI/net/storage driver; fix 2-node 5× slowdown |
| Store & sync | [06-store-sync.md](../06-store-sync.md) | Gossip, join tokens, snapshot reconcile |
| RBAC | [07-rbac.md](../07-rbac.md) | Deny default, SA tokens, cluster-dashboard E2E |

## Cross-cutting themes (task7)

1. **Performance** — Leader-only scheduler; snapshot reconcile; parallel store reads; overlay rootfs; optional io_uring for image layers.
2. **Isolation** — Default VNet; cgroup limits; capability profiles; Landlock for container-minimal; network policies.
3. **Architecture** — API → store → scheduler → engines; no component calls CRI directly.
4. **Unified networking CRDs** — Service / EdgeService / route table convergence ([04-netmux.md](../04-netmux.md)).
5. **PID 1 & nested z8s** — Host daemons + privileged nested control plane ([02-cri.md](../02-cri.md) § Nested).

## Current status

See [08-completion-status.md](../08-completion-status.md) for what landed on branch `v10`.

## Suggested next execution order

1. Close **P0 scheduler** (leader lease, reduce full scans).
2. **R8** + **H3** E2E scripts on CI with `z8s reset` bootstrap.
3. **NetMux N2** planner-only reconcile (remove duplicate imperative paths).
4. **exec/session** extraction; **LOC** audit with deleted dead handlers.
