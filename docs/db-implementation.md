# z8s DB Architecture — Implementation Summary

## What Was Achieved

### 1. Custom Type System (~2500 lines in `src/types.rs`)
- Replaced entire `k8s-openapi` dependency with hand-written Kubernetes-compatible types
- All standard resource types: Pod, Service, ConfigMap, Secret, PV, PVC, Deployment, Ingress, NetworkPolicy, Namespace, Node, Endpoints, EndpointSlice, Event, StorageClass
- Custom CRD types: VNet, Subnet, NSG, RouteTable (moved from `netmux::crds`)
- Scheduler extensions: `assigned_node`, `scheduler_epoch` on Pod
- Proper serde annotations for kubectl-compatible JSON (`clusterIP`, `podIP`, `hostIP`, `podCIDR`, etc.)
- `#[serde(tag = "resourceType")]` for correct enum deserialization
- `kind`/`apiVersion` fields with proper defaults on all resource types

### 2. Store Backend Abstraction (`src/store/backend.rs`)
- `StoreBackend` trait with 6 async methods: `apply`, `delete`, `get_all`, `get_by_kind`, `get`, `update_state`
- `MemoryBackend` — in-memory HashMap implementation (refactored from legacy `ResourceStore`)
- `RedbBackend` — persistent implementation using [redb](https://github.com/cberner/redb)
- Tables: `resources`, `nodes`, `leases`, `events`
- `Database::open()` reads existing DB; `Database::create()` creates new (prevents data loss on restart)
- JSON serialization for storage (replaced bincode due to untagged enum issues)

### 3. Multi-Node Gossip (`src/store/ws.rs`, `src/store/gossip.rs`)
- WebSocket server/client using axum + tokio-tungstenite
- Gossip endpoint at `/ws/gossip` on the API server port
- Initial sync: `SyncRequest` → `SyncFull` exchange on connect
- Real-time broadcast: per-peer `mpsc::unbounded_channel` fan-out
- Dedup tracking via `HashMap<String, u64>` (term-based, `>=`)
- Lifetime gossip client with auto-reconnect and heartbeat

### 4. Scheduler (`src/scheduler/scheduler.rs`)
- Lease-based leader election via `leases` table
- One active scheduler cluster-wide; lease refresh every 10s before expiry
- Schedule loop: every 3s picks unscheduled pods, assigns to least-loaded node (by pod count)
- Re-assigns pods from dead nodes (heartbeat >30s stale)
- Writes `assigned_node` + `scheduler_epoch` with each assignment

### 5. Heartbeat + Node Management (`src/store/leases.rs`)
- Per-node heartbeat loop writing `NodeRecord` every 5s to the `nodes` table
- Scheduler lease acquisition with randomized backoff
- Lease renewal and re-acquisition on expiry
- Node state tracking (`Active` / `Dead`)

### 6. Database Persistence & CLI
- `--data-dir <PATH>` — directory for redb database file
- `--db-path <PATH>` — exact path to database file
- Namespaces, pods, services, deployments, CRDs all survive restart
- Node resources created on startup and synced via gossip

### 7. Events (`src/store/events.rs`)
- Event recording via store backend (replaced in-memory `EventStore`)
- Automatic pruning when exceeding 1000 events

### 8. Node Table (`kubectl get nodes -o wide`)
- Shows: Name, Status, Roles, Age, CPU, RAM, Pods (ready/total), Services
- Uses process tracker for accurate ready counts
- Reads gossiped Node resources from all cluster members

### 9. Removed Dependencies
- Removed `k8s-openapi` (~63 types replaced)
- Removed `bincode` (switched to `serde_json`)

## Test Infrastructure

### Test Scripts

| Script | Purpose |
|---|---|
| `tests/netmux/test_hub_spoke.sh` | Full wrapper: setup + verify + restart persistence |
| `tests/netmux/test_hub_spoke_setup.sh` | Creates VNet, Subnets, NSG, RouteTable, Deployments, Services, Ingress |
| `tests/netmux/test_hub_spoke_verify.sh [--restart]` | Runs 9 connectivity tests pre/post restart |
| `tests/cluster/start-3-nodes.sh` | Launches 3-node gossip cluster |
| `tests/cluster/kc.sh` | Sets up kubectl contexts for cluster nodes |
| `tests/run-tests.sh` | Full regression suite (~180 tests) |

### How Tests Work

**Unit tests** (`cargo test`):
- 39 tests covering store backends, type serialization, API handlers
- RedbBackend: apply/get, get_by_kind prefix scan, delete, overwrite

**Hub-Spoke integration:**
1. Start z8s with `--data-dir /tmp/z8s-test-db`
2. Create VNet (10.200.0.0/16), 3 Subnets, NSG, RouteTable
3. Deploy 3 deployments (hub + 2 spokes) with services
4. Verify: hub→spoke1, hub→spoke2, hub→internet connectivity
5. Verify: spokes blocked from hub and internet (NSG enforcement)
6. Verify: NodePort 30005, Ingress hub1.local.cluster
7. Restart z8s, verify all resources persisted

**Multi-node clique:**
1. Start 3 nodes (ports 6443, 7443, 8443) with full mesh `--peers`
2. Verify gossip sync: create resource on node A, check nodes B and C
3. `kubectl config use-context z8s-[a|b|c]` to switch between nodes

**Persistence:**
- Resources created with `--data-dir` survive process restart
- Verified by restarting and checking deployments, services, ClusterIPs, CRDs

## Remaining / Low Priority

| Feature | Status | Notes |
|---|---|---|
| Anti-entropy (checksum exchange) | 🟡 Placeholder | `anti_entropy.rs` computes hash but doesn't exchange |
| Worker auth token validation | 🟡 Basic wiring | `--join-token` in config, join CLI in main.rs |
| Full anti-entropy diff sync | ❌ | Request missing keys between peers |
| Service proxy nftables restore | ❌ | Rules recreated by reconcile after restart |
| `z8s join` CLI as separate binary | ❌ | Currently embedded in main binary |
| `ResourceState` persistence | ❌ | Ephemeral — reset on restart (acceptable) |
| End-to-end encryption for gossip | ❌ | Unauthenticated WebSocket |
| Horizontal pod autoscaling | ❌ | Requires metrics pipeline |

## Known Limitations

- **ResourceState not persisted**: Pod state (Pending/Running) is in-memory only. On restart, state resets but containers are recreated by reconciler
- **nftables rules ephemeral**: Firewall rules are lost on restart and recreated by reconciliation (may cause brief connectivity gap)
- **Single scheduler**: Lease-based but only one scheduler runs at a time
- **All nodes equal**: No special control-plane role; every node runs API + scheduler + worker
- **No TLS**: All communication (API + gossip) is plain HTTP/WebSocket
