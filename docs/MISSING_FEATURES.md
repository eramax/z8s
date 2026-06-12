# Missing Features vs src2 (old code)

Comparison of what the new controller/src2 crates are missing relative to the
old monolithic `src2/` codebase.

## Legend
- **Stub** — crate exists but is empty (`pub fn placeholder()`)
- **Missing** — no code exists at all
- **Partial** — basic impl exists, significant gaps

---

## 1. Core Reconciliation Architecture

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 1.1 | **Event-driven reconcile** — `StoreEventHub` subscriptions drive incremental reconciliation instead of polling | **Missing** | New controller polls on timer only |
| 1.2 | **Component Registry** — `Component` trait with `reconcile()`, `on_apply()`, `on_delete()` per resource kind | **Missing** | New controller handles only Pods directly |
| 1.3 | **ResourceState machine** — `Pending → Running → Succeeded/Failed/Terminated` per tracker | **Missing** | New controller only has `Phase` in status |
| 1.4 | **Periodic full sweep** — full reconcile every N ticks to catch drift | **Missing** | |
| 1.5 | **Graceful shutdown** — stop all pods, clean nftables, orphan veths on SIGTERM | **Missing** | |

## 2. Resource Components (registered in ComponentRegistry)

The old code had dedicated `Component` impls for each:

| # | Component | Status | Notes |
|---|-----------|--------|-------|
| 2.1 | **Pod** | **Partial** | New controller starts/stops pods but no restart policy, probes, logs |
| 2.2 | **Deployment** (ReplicaSet management, rolling updates, scaling) | **Missing** | Only logs a debug message |
| 2.3 | **Service** (ClusterIP VIP, pod backend resolution) | **Missing** | |
| 2.4 | **Ingress** (HTTP ingress with TLS) | **Missing** | |
| 2.5 | **NetworkPolicy** (nftables-based policy enforcement) | **Missing** | |
| 2.6 | **VNet** (virtual network CRD) | **Missing** | |
| 2.7 | **Subnet** (subnet CRD attached to VNets) | **Missing** | |
| 2.8 | **NSG** (network security group rules) | **Missing** | |
| 2.9 | **RouteTable** (custom route tables per VNet) | **Missing** | |
| 2.10 | **ConfigMap** | **Missing** | |
| 2.11 | **Secret** | **Missing** | |
| 2.12 | **PersistentVolume** | **Missing** | |
| 2.13 | **PersistentVolumeClaim** | **Missing** | |

## 3. Pod Lifecycle

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 3.1 | **Restart policies** (`Always`, `OnFailure`, `Never`) | **Missing** | |
| 3.2 | **CrashLoopBackOff** — exponential backoff (1s→2s→4s→...→300s max) | **Missing** | |
| 3.3 | **Zombie reaping** — `waitpid()` on SIGCHLD to prevent zombie accumulation | **Missing** | |
| 3.4 | **Readiness/liveness probes** — HTTP, TCP, exec probe runners | **Missing** | Only `is_pid_alive` |
| 3.5 | **Log buffering** — capture stdout/stderr per container | **Missing** | |
| 3.6 | **Parallel pod startup** — semaphore-limited concurrent container starts | **Missing** | Sequential |
| 3.7 | **Container exec** — `exec` into running containers | **Missing** | |
| 3.8 | **ServiceAccount token mount** — automount SA tokens into pods | **Missing** | |

## 4. Scheduling & Cluster Membership

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 4.1 | **Scheduler lease (leader election)** — epoch-based lease with auto-renewal | **Missing** | `set_leader(bool)` only |
| 4.2 | **Heartbeat / node liveness** — periodic heartbeat written to DB with 30s deadline | **Missing** | |
| 4.3 | **Gossip protocol** — WebSocket-based state sync between nodes | **Stub** | `sync` crate is placeholder |
| 4.4 | **Anti-entropy** — background task re-checks consistency | **Stub** | Part of gossip module |
| 4.5 | **WaitForFirstConsumer volume binding** — provision PVs on target node before scheduling | **Missing** | |

## 5. Network Resources

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 5.1 | **VNet reconciliation** | **Missing** | |
| 5.2 | **Subnet reconciliation** | **Missing** | |
| 5.3 | **NSG (security group rules)** | **Missing** | |
| 5.4 | **RouteTable** | **Missing** | |
| 5.5 | **NetworkPolicy** (nftables enforcement) | **Missing** | |
| 5.6 | **Service (ClusterIP)** — virtual IP with backend resolution | **Missing** | |
| 5.7 | **Ingress** — HTTP ingress with TLS termination | **Missing** | |
| 5.8 | **DNS / service discovery** — in-cluster DNS server | **Missing** | |

## 6. Storage

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 6.1 | **StorageClass** — default SC seeding, class resolution | **Missing** | |
| 6.2 | **Hostpath provisioner** | **Missing** | |
| 6.3 | **Loop device provisioner** | **Missing** | |
| 6.4 | **PV/PVC binding** — automatic provisioning on PVC create | **Missing** | |
| 6.5 | **ProvisionerDispatcher** — routes PVCs to the right provisioner | **Missing** | |

## 7. API & Auth

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 7.1 | **REST API server** — full CRUD HTTP API for all resource types | **Stub** | `api` crate is placeholder |
| 7.2 | **Watch endpoints** — streaming resource changes | **Missing** | |
| 7.3 | **RBAC** — Role/ClusterRole/ServiceAccount/ClusterRoleBinding enforcement | **Missing** | |
| 7.4 | **Admission control** — validate/mutate webhooks | **Missing** | |
| 7.5 | **TLS certs** — auto-generation for API server | **Missing** | |
| 7.6 | **Kubeconfig generation** — `z8s set kubeconfig` CLI command | **Missing** | |
| 7.7 | **Discovery API** — API version discovery (kubectl compat) | **Missing** | |

## 8. CLI & Bootstrap

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 8.1 | **Manifest watcher** — watch directory for file-based resource definitions | **Missing** | |
| 8.2 | **Join tokens** — token-based cluster auth for worker nodes | **Missing** | |
| 8.3 | **Daemon lifecycle** — `z8s node start/stop/list`, daemon spawning, lock files, PID tracking | **Missing** | |
| 8.4 | **Reset command** — `z8s reset` stops all, unmounts, wipes state | **Missing** | |
| 8.5 | **Admin ServiceAccount bootstrap** | **Missing** | |
| 8.6 | **Bootstrap resources** — kubernetes Service, default SA, RBAC roles | **Missing** | |

## 9. Configuration

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 9.1 | **Config struct** — CLI flags + env vars parsing | **Missing** | |
| 9.2 | **OverlayFS rootfs** — configurable `--overlay-rootfs` flag | **Missing** | |
| 9.3 | **Pod start parallelism** — configurable concurrent pod starts | **Missing** | |
| 9.4 | **RBAC mode** — `enforce` vs `permissive` | **Missing** | |

## 10. Cleanup & Safety

| # | Feature | Status | Notes |
|---|---------|--------|-------|
| 10.1 | **Shutdown pod cleanup** — stop all pods on SIGTERM | **Missing** | |
| 10.2 | **D-state watchdog** — force-exit if cleanup stalls in uninterruptible sleep | **Missing** | |
| 10.3 | **Orphan veth cleanup** — remove dangling veth pairs on start | **Missing** | |
| 10.4 | **nftables cleanup** — remove NAT/filter tables on shutdown | **Missing** | |
| 10.5 | **Zombie process reaping** (PID 1 responsibilities) | **Missing** | |

---

## Summary

- **Full implementations**: 0
- **Partial implementations**: 1 (Pod start/stop in controller)
- **Stubs**: 4 (`sync`, `api`, `z8s`, `z8s-node`)
- **Missing**: ~40 features
