# DB Resource Store — Gossip over WebSocket

## Architecture: All Nodes Equal, Gossip Consensus

No leader. No raft. Every server node is equal. When one node writes to its local redb, it broadcasts the change to all peers via WebSocket. Peers apply and re-broadcast. Eventual consistency with periodic anti-entropy.

```
┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
│  Server Node A   │   │  Server Node B   │   │  Server Node C   │
│                  │   │                  │   │                  │
│  ┌────┐  ┌────┐  │   │  ┌────┐  ┌────┐  │   │  ┌────┐  ┌────┐  │
│  │redb│  │ WS │  │◀──▶│  │redb│  │ WS │  │◀──▶│  │redb│  │ WS │  │
│  │    │  │goss│  │ WS │  │    │  │goss│  │ WS │  │    │  │goss│  │
│  └────┘  └────┘  │   │  └────┘  └────┘  │   │  └────┘  └────┘  │
│                  │   │                  │   │                  │
│  API :6443       │   │  API :6443       │   │  API :6443       │
│  Scheduler       │   │  Scheduler       │   │  Scheduler       │
│  (lease)        │   │  (standby)       │   │  (standby)       │
└─────────────────┘   └─────────────────┘   └─────────────────┘
```

**Servers** = API + redb + gossip (full state, all equal)
**Workers** = connect to any server via WebSocket, run assigned work

## NetMux: Unchanged (Stays Local)

The gossip DB does NOT change NetMux. NetMux remains purely local — it creates veths, programs nftables, and manages networking on the node it runs on. What changes is the cluster-wide state that feeds into it:

| NetMux function | Multi-node change |
|---|---|
| `attach_pod` (veth, IP) | **None** — still per-node. Each node creates veths for its assigned pods |
| `configure_pod_netns` | **None** — `/proc/<pid>/ns/net` is always local |
| nftables DNAT | **Data changes, code doesn't.** Backend list now includes remote pods (from DB), but `add_dnat()` stays the same |
| nftables NSG/forward | **None** — same rules on every node, enforced locally |
| Pool allocator | **Per-node sub-CIDR** — each node owns a /24 from the pod CIDR (e.g. 10.42.0.0/24 for node A, 10.42.1.0/24 for node B). No cross-node coordination per pod. |
| Routes to remote pods | **New watcher** — watches `node_registry` in DB. When a new node joins, adds `ip route add <node-pod-cidr> via <node-host-ip>`. Not in NetMux — handled by a separate route controller. |
| Service backend resolution | **Data source changes** — reads full backend list from DB (all nodes) instead of only local ProcessTracker |

### What the gossiped DB adds

Three new controllers (not changes to NetMux):

1. **RouteController** — watches `node_registry`, adds/removes cross-node host routes
2. **ServiceBackendController** — watches services + pod assignments, maintains full backend list in DB
3. **DNATController** — reads full backend list from DB, calls existing `nft.add_dnat()` on the local node

NetMux itself: same functions, same calls, same nftables rules. Zero rewrite.

## Gossip Protocol

### Write Flow

```
Node A: writes to local redb
  → A sends via WS to all peers: { type: "gossip", key, value, term: 42 }
  → B receives → applies to local redb → broadcasts to its peers (C, ...)
  → C receives → applies to local redb → broadcasts to its peers (A, B, ...)
  → Each node deduplicates by <key, term> (ignore already-applied messages)
```

**Every node that receives a gossip message also re-broadcasts it.** This ensures delivery even if some WebSocket connections drop.

### Deduplication

Each gossip message has a `term` field — a composite `(node_id, local_seq)` tuple:

```rust
struct Term {
    node_id: u64,    // unique per node (derived from node name hash)
    local_seq: u64,  // monotonic per-node counter
}
```

Nodes track the last applied term per key. Comparison: higher `local_seq` wins; if equal, higher `node_id` wins (last-writer-wins). No global counter needed — each node's sequences are disjoint.

### Anti-Entropy (Every 30s)

```
Node A → Node B: { type: "checksum", hash: xxhash3(key_terms) }
Node B → Node A: { type: "checksum", hash: xxhash3(key_terms) }
  → If hashes match → all good
  → If mismatch → exchange full key list, request missing keys
```

Implementation: compute an xxHash3 checksum over all redb resource keys + their terms. Exchange and compare. Request only the missing/different entries. xxHash3 is used instead of CRC32 to avoid collision risk in large clusters.

### Connection Management

```
On startup:
  1. Open/init local redb
  2. Connect via WebSocket to each known peer (from --peers or config)
  3. Request full sync from first peer: { type: "sync_request" }
  4. Peer responds: { type: "sync_full", entries: [{key, value, term}, ...] }
  5. Apply all to local redb
  6. Enter gossip + anti-entropy loop
```

### Message Types

```json
{ "type": "gossip",     "key": "...", "value": ..., "term": 42 }
{ "type": "sync_request" }
{ "type": "sync_full",  "entries": [{"key", "value", "term"}, ...] }  // streamed in chunks of 100
{ "type": "checksum",   "hash": "abc123", "keys": ["key1", "key2"] }
{ "type": "key_request", "keys": ["key1", "key3"] }
{ "type": "key_response", "entries": [{"key", "value", "term"}, ...] }
{ "type": "heartbeat" }
```

## Scheduler: Leader-Elected via Redb Lease

One active scheduler cluster-wide. Lease stored in redb (synced via gossip). Each lease acquisition creates a new **epoch** — this is the key to rejecting stale scheduler writes.

```
Key: "leases/scheduler"
Value: LeaseRecord { holder, epoch: u64, expires_at_ms }

Epoch: monotonic counter. Incremented each time a new node acquires the lease.
       Starts at 1. Never wraps.

Lease acquisition (with race prevention):
  All nodes: try to acquire lease at startup
  Before claiming expired lease: wait rand(0..5s) randomized backoff
  When acquiring: new_holder.epoch = old_lease.epoch + 1
  This prevents B and C from claiming simultaneously when A crashes

  Node A gets it → epoch=1, active scheduler. Refreshes every 5s.
  Nodes B, C fail → standby. Re-check every 5s.

On Node A crash:
  → 5s: no refresh
  → 15s: lease expires
  → Node B: rand(0..5s) wait → acquires → epoch=2 → scheduler
  → Node C: rand(0..5s) wait → sees B has it with epoch=2 → stays standby
```

**Active scheduler:**
- Watches Pods without `assigned_to` → picks best node
- Writes assignment to redb → gossiped to all nodes
- Allocates ClusterIPs for Services

**Standby schedulers:**
- Only monitor the lease
- Forward scheduling requests? No — the active scheduler handles everything via watching redb.

## Workers

```
z8s join ws://10.0.0.1:9876
  → Connect to any server
  → Receive sync_full (all state)
  → Keep local cache-redb
  → Watch for assigned work:
      assigned_to == this worker → execute (CRI + NetMux)
      assigned_to == other → skip, cache only
```

## Consistency Guarantees

| Scenario | Behavior |
|---|---|
| Two nodes write same key simultaneously | Last writer wins (term ordering) |
| Write arrives before gossip | Local read returns stale data (ms latency) |
| Gossip message lost | Anti-entropy catches within 30s |
| Network partition | Both sides continue independently. On reconnect, anti-entropy merges (last-writer-wins per key) |
| Node crash after write, before gossip | Anti-entropy recovers missed writes within 30s |
| Split-brain (both sides write during partition) | **Conflict resolution: higher `(scheduler_epoch, updated_by)` wins.** If two schedulers assign the same pod to different nodes, the write from the higher epoch wins. Same-epoch ties broken by node name. The losing scheduler's write is overwritten. Worker that lost the race cleans up any partially-started pod and releases the IP. |

This is eventual consistency — exactly like DNS, most K8s controllers, and gossip-based systems.

## Coupling Resolution

The new architecture replaces synchronous `on_apply`/`on_delete` hooks with async gossip + DB reads. Each coupling point must be explicitly loose:

### 1. API Server → Scheduler (was: synchronous callback)

**Before:** API writes → calls `component.on_apply()` → response.

**After:** API writes to redb → gossiped async → scheduler processes independently → response returns to user before scheduler acts.

**Fix:** API response means "write accepted by DB," not "applied everywhere." K8s model — `kubectl apply` returns before pod is running.

### 2. Scheduler → Worker (pod assignment)

**Before:** Same process (single node). No coupling.

**After:** Scheduler writes `pods/{ns}/{name}` with `assigned_node`. Gossip propagates. Worker reads and acts.

**Fix:** Worker periodically scans `pods/` for assignments matching its node name. No handshake needed. Also watches `pod_status/` — if overwritten by higher epoch (re-assignment), tears down pod.

### 3. Worker → Scheduler (status)

**After:** Worker writes `pod_status/{ns}/{name}` with `owner_epoch`. Scheduler reads.

**Fix:** Worker writes independently. If `owner_epoch != pods/{ns}/{name}.scheduler_epoch`, write is rejected (stale worker). Worker detects rejection → tears down pod.

### 4. Scheduler → NetMux (nftables)

**Before:** Direct call: `service.on_apply()` → `netmux.add_dnat()`.

**After:** Scheduler writes `service_backends/` to DB. Local DNAT controller on each node reads and calls `netmux.add_dnat()`.

**Fix:** DNAT controller watches `service_backends/` in local redb. On change, programs local nftables. NetMux is always local, driven by DB state. Not coupled to scheduler liveness.

### 5. Scheduler → Route Controller (cross-node routes)

**After:** Route controller watches `nodes/`. New node appears → reads host IP + pod CIDR → adds host route.

**Fix:** Purely DB-driven. Reads `nodes/`, compares with kernel routes, reconciles. On node death (heartbeat timeout) → removes route.

### 6. Manifesto Dir on Startup

No special coupling. Each manifest YAML becomes a `pods/` or `services/` entry with `assigned_node: None`. Scheduler picks them up same as `kubectl apply` path.

### 7. Gossip Delivery ≠ Local Read

**Fix:** Single serialized writer per node. On gossip receive:
1. Apply to local redb (write lock)
2. Broadcast locally (tokio) to trigger controllers
3. Controllers always read from redb, never from gossip message directly

No stale reads — by the time a controller processes an event, data is already in local redb.

### 8. Stale Scheduler Writes

Every scheduler write includes `scheduler_epoch`. Gossip receive path rejects writes where `incoming.epoch < local.leases/scheduler.epoch`. Stale writes are silently dropped before they hit redb.

## Scheduler: Pod Distribution Algorithm

### Scheduling Flow

```
Active scheduler watches for unscheduled Pods:
  → Query redb: prefix scan "resources/Pod/" where assigned_node == None
  → For each unscheduled Pod:

     ① Get node list from "nodes/" table (skip nodes with state != "ready")
     ② For each node, count assigned pods:
        prefix scan "resources/Pod/" with assigned_node == node_name
     ③ Pick node with fewest pods (round-robin for equal loads)
     ④ Write to redb: resources/Pod/{ns}/{name}
        → set assigned_node: "node-b"
        → set scheduler_epoch: current_lease.epoch
     ⑤ Gossip propagates to all nodes
     ⑥ Node B sees assigned_node = self → reads resources/Pod/{ns}/{name}
     ⑦ Node B creates container via CRI, attaches veth via NetMux
     ⑧ Node B writes to redb: resources/Pod/{ns}/{name}
        → status.phase: "Running"
        → status.pod_ip: "10.42.1.5"
        → scheduler_epoch unchanged (epoch check passes)
```

**No separate tables.** Everything is on the Pod resource itself. Scheduler writes assignment, worker writes status. Epoch checks prevent stale writers.

### Node Health & Load Tracking

```rust
// Each node writes its own heartbeat + load every 5s
nodes/node-a → { node_name, host_ip, pod_cidr, state: "ready",
                 pod_count: 5, last_seen_ms: ..., version: ... }
```

Scheduler marks node as dead if `last_seen > 30s`. Dead node's pods are re-assigned.

### Conflict Resolution (Partition Heal)

If two schedulers assign the same pod during a partition:

```
Pod "my-pod" assigned_to: node-b  (scheduler on node-a, term: 42)
Pod "my-pod" assigned_to: node-c  (scheduler on node-c, term: 43)

After heal → last-writer-wins by term → assigned_to: node-c
Node B sees → "I was assigned but now it's node-c" → clean up if started
```

**Cleanup rule:** Each node watches ALL assigned_to changes. If its own assignment is overwritten, it kills any partially-started pod and releases the IP.

### Node Failure Re-Scheduling

```
Node B heartbeat stops (last_seen not updated >30s)
  → Scheduler sets node-b state = "dead" in nodes/node-b
  → Scheduler queries all pods with assigned_node: node-b from pods/
  → For each such pod:
     ① Set assigned_node: None (unassign)
     ② Re-enter scheduling loop → assign to another node
     ③ New assignment written to pods/{ns}/{name} → gossiped
     ④ Target node reads pods/, creates container
```

## DB Key Design for Multi-Node

Keys are designed so each record has **exactly one writer class**:

| Table | Key format | Value | Written by | Owner class |
|---|---|---|---|---|
| `resources` | `{kind}/{ns}/{name}` | bincode(StoredResource) | API (create), scheduler (assign), worker (status) | Varies by field |
| `nodes` | `{node_name}` | bincode(NodeRecord) | **Only that node** | Node |
| `leases` | `scheduler` | bincode(LeaseRecord) | **Active scheduler only** | Scheduler |
| `routes` | `{peer_node}` | bincode(RouteRecord) | Route controller (derived) | Controller |
| `events` | `{kind}/{uid}` | bincode(EventRecord) | Any actor (append-only) | Append-only |
| `meta` | schema_version | `u64` | System startup | System |

### Why a single `resources` table

Instead of separate `pods/`, `services/`, `configmaps/` tables, everything lives in one `resources` table keyed by `{kind}/{ns}/{name}`:

```
resources/Pod/default/my-nginx
resources/Service/default/my-svc
resources/ConfigMap/default/my-config
resources/Secret/default/my-secret
...
```

This preserves the existing `get_by_kind("Pod")` pattern via prefix scan (`"Pod/"`). The `StoredResource` enum wraps all types — same as the current `AnyResource` but with our own type definitions instead of k8s-openapi.

### Write ownership by field group

Within a single `resources` entry, different fields have different writers:

| Field | Written by | Enforced by |
|---|---|---|
| `.metadata`, `.spec.*` | API server (create/update) | API creates the entry |
| `.assigned_node`, `.scheduler_epoch` | **Scheduler only** | Epoch check on write |
| `.status.phase`, `.status.pod_ip` | **Worker only** (if assigned) | Assigned node match + epoch check |
| `.status.container_statuses` | **Worker only** | Same |

This is enforced in the gossip receive path: a write to `resources/Pod/default/my-nginx` from a non-scheduler that modifies `assigned_node` is rejected. A write to `.status` from a node that isn't `assigned_node` is rejected. The epoch check prevents stale writers from overwriting.

### Example record

```yaml
resources/Pod/default/my-nginx:
  metadata: { name: my-nginx, namespace: default, ... }
  spec:
    containers:
      - name: nginx
        image: nginx:latest
        ports: [{ container_port: 80 }]
  status:
    phase: Running
    pod_ip: 10.42.1.5
    container_statuses:
      - name: nginx
        ready: true
  assigned_node: node-b
  scheduler_epoch: 3
```

## Multi-Node Test Scenarios

### Setup: Three Nodes on Same Machine

```bash
# Terminal 1: Node A
z8s --port 6443 --data-dir /tmp/z8s-a \
    --peers node-b=127.0.0.1:7443,node-c=127.0.0.1:8443

# Terminal 2: Node B
z8s --port 7443 --data-dir /tmp/z8s-b \
    --peers node-a=127.0.0.1:6443,node-c=127.0.0.1:8443

# Terminal 3: Node C
z8s --port 8443 --data-dir /tmp/z8s-c \
    --peers node-a=127.0.0.1:6443,node-b=127.0.0.1:7443
```

### Test 1: Gossip Sync

```
1. kubectl apply -f vnet.yaml → --server :6443 (Node A)
2. kubectl get vnets          → --server :7443 (Node B)
   → VNet "test-vnet" should appear (gossiped within ms)
3. kubectl get vnets          → --server :8443 (Node C)
   → Same
```

### Test 2: Cross-Node Pod Scheduling

```
1. Create a Pod on Node A (kubectl apply → :6443)
2. Scheduler (active on whichever node holds lease) picks a node
3. kubectl get pods --server :6443 → Pod shows assigned_to: node-x
4. kubectl get pods --server :7443 → Same (gossiped)
5. Check which node actually runs it (curl :6443/pods → .status.phase)
```

**Expected:** Pod is scheduled to ONE node. All three see it.

### Test 3: Scheduler Failover

```
1. Kill the node holding the scheduler lease (Ctrl+C)
2. Wait ≤20s (15s lease expiry + 5s randomized backoff)
3. Another node acquires the lease
4. Create a new Pod → new scheduler assigns it
```

### Test 4: Node Failure + Pod Re-Assignment

```
1. Three nodes running. Pods scheduled across them.
2. Kill Node B (runs 2 pods).
3. Wait 30s (heartbeat timeout)
4. Scheduler detects node-b dead, re-assigns its pods to A and C
5. kubectl get pods → previously-running pods now assigned to A/C
```

### Test 5: Gossip Partition Heal

```
1. Start Node A + Node B.
2. Create Pod on A → gossiped to B.
3. Kill both, restart with new peers (simulate partition).
4. Create another Pod on B during partition.
5. Heal: restart full cluster with all three peers.
6. Wait for anti-entropy (30s).
7. Both Pods should appear on all nodes.
```

### Test 6: Conflicting Writes (Race)

```
1. Two terminals, each with kubectl pointed to different servers:
   Terminal 1: kubectl apply -f pod.yaml --server :6443
   Terminal 2: kubectl apply -f pod.yaml --server :7443 (same name!)
2. Both write the same pod name simultaneously.
3. Gossip propagates both → last-writer-wins by term.
4. kubectl get pods → one version (eventual consistency).
5. No crash, no duplicate containers. The losing node's write is silently dropped.
```

### Test 7: Scheduler Distribution Fairness

```
1. Three nodes (A, B, C). Scheduler lease on A.
2. Create 9 pods sequentially.
3. kubectl get pods -o wide → each node should get ~3 pods.
4. If uneven, scheduler adjusts on next assignment.
```

## Comparison: Gossip vs Raft

| | Raft | Gossip (this) |
|---|---|---|
| Consistency | Strong | Eventual (~ms convergence) |
| Complexity | 2000+ lines, subtle bugs | ~300 lines, simple |
| Split-brain | Impossible | Possible (auto-heals) |
| Write speed | Must reach majority | Write locally, broadcast async |
| Read speed | Local | Local |
| Anti-entropy | Not needed (raft log) | Needed (30s checksum) |
| Code to write | RaftStorage + RaftNetwork + snapshot | WS broadcast + checksum diff |

## Redb Tables — Canonical Schema

### Single Type System: Our Own Types for Everything

Define our own types in `src/store/types.rs` — ~600 lines of hand-written structs with serde annotations. These types serve **both** the API layer (HTTP responses) and the storage layer (redb). No k8s-openapi dependency at all.

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pod {
    pub metadata: ObjectMeta,
    pub spec: PodSpec,
    pub status: PodStatus,
    pub assigned_node: Option<String>,
    pub scheduler_epoch: u64,
}
```

`#[serde(rename_all = "camelCase")]` ensures kubectl gets valid Kubernetes JSON. Optional fields use `Option` so they're absent from JSON when `None` (kubectl requirement). The scheduler-extension fields (`assigned_node`, `scheduler_epoch`) are native to the Pod struct — no separate tables.

### Why Not k8s-openapi

k8s-openapi types are deeply nested, hard to extend, and pull in a large dependency graph. Our own types are:

- **~600 lines** vs tens of thousands
- **Scheduler fields native** — `assigned_node`, `scheduler_epoch` live directly on `Pod`
- **No conversion layer** — the same struct serializes to redb bytes and to HTTP JSON
- **Drop-in replacement** — existing handlers that match on `json!()` patterns need updating, but the serde output is identical

### Single `resources` Table

One table for all resource types:

| Table | Key | Value | Writer class |
|---|---|---|---|
| `resources` | `{kind}/{ns}/{name}` | `bincode(StoredResource)` | Varies by field (see above) |
| `nodes` | `{node_name}` | `bincode(NodeRecord)` | Only that node |
| `leases` | `scheduler` | `bincode(LeaseRecord)` | Active scheduler only |
| `routes` | `{peer_node}` | `bincode(RouteRecord)` | Route controller (derived) |
| `events` | `{kind}/{uid}` | `bincode(EventRecord)` | Any actor (append-only) |
| `meta` | schema_version | `u64` | System |

Key format: `"Pod/default/my-nginx"`, `"Service/default/my-svc"`, etc. Prefix scan `"Pod/"` gives all pods — same as current `get_by_kind("Pod")`.

### Partial Updates: Read-Modify-Write

A worker updating pod status reads the full record, changes one field, writes it back:

```rust
let mut pod = db.get("resources", "Pod/default/my-nginx").await?;
pod.status.phase = "Running".into();
pod.status.pod_ip = Some("10.42.1.5".into());
db.apply("resources", &pod).await?;
```

A Pod is ~2KB. Read + write costs <1ms. No contention because only the assigned worker writes status for a given pod. The scheduler only writes assignment. No two actors write the same Pod's overlapping fields.

### Partial Update Safety: Epoch Check

When gossip delivers a write, the receiver:
1. Deserializes the incoming and current `StoredResource`
2. If the incoming modified `assigned_node` and `incoming.scheduler_epoch < local.leases/scheduler.epoch` → reject (stale scheduler)
3. If the incoming modified `status.*` and `incoming.source_node ≠ local.pods/*.assigned_node` → reject (wrong worker)
4. Otherwise → apply

### redb Concurrency Model

redb supports **a single writer + multiple concurrent readers** via MVCC:

- Read transactions get a snapshot — never block, never stale within the txn
- Write transactions are serialized — only one at a time
- Our architecture matches this perfectly: gossip delivers writes sequentially, API handlers and controllers read concurrently

All writes use `tokio::task::spawn_blocking` because redb's API is synchronous and `commit()` calls `fsync`. Reads can use `block_in_place` or also `spawn_blocking` — whichever is simpler.

redb already uses XXH3_128 internally for page checksums (same algorithm we planned for anti-entropy). We may reuse it rather than adding a separate xxhash crate.

### Table Definitions

| Table | Key format | Value type | Written by | Owner |
|---|---|---|---|---|
| `meta` | `schema_version`, `cluster_id`, `local_node_id` | `u64` / `Vec<u8>` | System startup | Cluster |
| `nodes` | `nodes/{node_name}` | `NodeRecord` | **Only that node's heartbeat loop** | Node |
| `leases` | `leases/scheduler` | `LeaseRecord` | **Active scheduler only** | Scheduler |
| `pods` | `pods/{ns}/{name}` | `PodRecord` | **Scheduler only** | Scheduler |
| `pod_status` | `pod_status/{ns}/{name}` | `PodStatusRecord` | **Only the assigned worker** | Worker |
| `services` | `services/{ns}/{name}` | `ServiceRecord` | **Scheduler or API only** | Scheduler |
| `service_backends` | `service_backends/{ns}/{name}` | `BackendSetRecord` | **Scheduler only** | Scheduler |
| `routes` | `routes/{node_name}` | `RouteRecord` | **Route controller only** | Controller |
| `events` | `events/{kind}/{uid}` | `EventRecord` | Any actor (append-only) | Append-only |
| `stats` | `stats/{scope}/{name}` | `u64` | Owner-local or derived | Varies |

### Rust Structs

```rust
// ── Cluster ──────────────────────────────────────────────
struct NodeRecord {
    node_name: String,
    host_ip: String,
    pod_cidr: String,
    capacity_cpu: u32,
    capacity_mem_mb: u32,
    pod_count: u32,
    state: String,                    // "ready" | "dead"
    last_seen_ms: u64,
    version: u64,
}

struct LeaseRecord {
    holder: String,                   // node name
    epoch: u64,                       // ↑ each time a new scheduler takes over
    expires_at_ms: u64,
    version: u64,
}

// ── Resources (our own types, not k8s-openapi) ──────────
// Pod has assigned_node + scheduler_epoch natively.
// Service has cluster_ip, selector, ports natively.
// No separate pod_status/backend_set tables needed.
// See the "Redb Tables — Canonical Schema" section above
// for the full type definitions.

// ── Network (derived — always rebuildable) ───────────────
struct RouteRecord {
    node_name: String,
    host_ip: String,
    pod_cidr: String,
    installed: bool,
    version: u64,
}

// ── Events (append-only) ─────────────────────────────────
struct EventRecord {
    kind: String,
    uid: String,
    timestamp_ms: u64,
    event_type: String,
    reason: String,
    message: String,
    source_node: String,
}
```

### Derived State: Routes and Backends

`routes/` and `service_backends/` are **indexes, not primary truth**. They are always rebuilt from the canonical tables:

- **`routes/{node_name}`** — rebuilt from `nodes/`. Each node runs a route controller that reads all healthy nodes and writes host routes for their pod CIDRs.
- **`service_backends/{ns}/{name}`** — rebuilt from `pods/` + `services/`. The scheduler reads all pods matching a service's selector and writes the backend set.

On restart or after partition heal: delete all entries in `routes/` and `service_backends/`, then rebuild from scratch. No data loss — they are purely derived.

### Event Retention

`events/` is append-only with a cap. Keep the last 1000 events total (oldest dropped on insert). This prevents unbounded growth in the gossip stream and redb file.

| Key pattern | Allowed writer | Code enforcement |
|---|---|---|
| `nodes/{node_name}` | Only `node_name == self` | Reject writes where `node_name ≠ local_node_id` |
| `leases/scheduler` | Anyone (but epoch must increase) | `incoming.epoch > current.epoch` only. Reject if ≤. |
| `pods/{ns}/{name}` | Scheduler only | Reject if `updated_by` is not the current lease holder |
| `pod_status/{ns}/{name}` | Only the assigned worker | Check `pod_status.node == self` AND `owner_epoch == pods/{ns}/{name}.scheduler_epoch`. Epoch mismatch rejects stale status. |
| `services/{ns}/{name}` | Scheduler only | Same as pods |
| `service_backends/{ns}/{name}` | Scheduler only | Same as pods (derived, rebuilt from pods+services) |
| `routes/{node_name}` | Route controller only | Derived from `nodes` table — rebuilt on restart |
| `events/*` | Any (append) | UIDs are unique |
| `stats/*` | Owner-local | Per-key rule |

### Versioning

Each record carries `version: u64` — a per-record monotonic counter. On write:

```
if incoming.version <= local.version → discard (stale)
if incoming.version == local.version + 1 → apply
if incoming.version > local.version + 1 → anti-entropy needed (gap)
```

Tiebreaker when versions match: higher `updated_by` (node name) wins. No global term — each writer increments its own records independently.

## Startup Sequence

### First Server
```
z8s --peers ""  (no peers yet)
  → Open/init redb
  → Become first peer (wait for others)
  → Load manifests/ dir → apply to redb
  → Start WS server :9876
  → Start API :6443
  → Acquire scheduler lease → active scheduler
```

### Second Server
```
z8s --peers node-a=10.0.0.1:9876
  → Open redb
  → WS connect to node-a
  → Request sync_full → apply to local redb
  → Start WS server :9876
  → Start API :6443
  → Try scheduler lease → Node A has it → standby
```

### Worker
```
z8s join ws://10.0.0.1:9876
  → Connect, receive sync_full, cache locally
  → Watch for assigned work
```

### Known Gap: No Gossip Authentication (Phases 1-7)

Until Phase 8, gossip between nodes is **unauthenticated**. Any peer that can reach the WebSocket port can inject or read cluster state. This is acceptable for development/testing but **not production-safe**. Phase 8 adds `--join-token` validation (already in `config.rs`).

## Phases

| Phase | What | Key Files | Risk |
|---|---|---|---|
| **1a** | `StoreBackend` trait + `MemoryBackend` (refactor from `ResourceStore`). API handlers unchanged behind the trait. | `src/store/mod.rs`, `src/store/backend.rs`, `src/types.rs`, `src/api/server.rs`, `src/main.rs` | Medium — mechanical refactor |
| **1b** | `RedbBackend` + single `resources` table + `nodes` + `leases`. Switchable at startup via config. All redb writes use `spawn_blocking`. | `Cargo.toml` (+redb, +bincode), `src/store/db.rs`, `src/config.rs` (`--data-dir`) | Low — new code, old path unchanged |
| **1c** | Define own types in `src/store/types.rs`. `StoredResource` enum replaces `AnyResource`. Replace k8s-openapi types in all handlers with own types. | `src/store/types.rs`, all handlers (~33 files) | High — coordinated change across many files |
| **2** | `leases` table + heartbeat + scheduler lease acquisition with randomized backoff + epoch | `src/store/leases.rs`, `src/store/scheduler.rs` | Medium |
| **3** | WebSocket gossip: broadcast writes to peers, receive + apply + dedup | `Cargo.toml` (+tokio-tungstenite), `src/store/ws.rs`, `src/store/gossip.rs` | Medium |
| **4** | Anti-entropy: periodic xxHash3 checksum + diff exchange | `src/store/anti_entropy.rs` | Low |
| **5** | Scheduler: watch unscheduled pods, pick node, write assignment. Deschedule on node death. | `src/store/scheduler.rs` (extend) | Medium |
| **6** | Workers: watch `assigned_node == self`, execute via CRI + NetMux, write status. Route controller. | Worker controller, route controller | Medium |
| **7** | Events table + retention. Integration tests (3 nodes on same machine). | `src/store/events.rs`, `tests/gossip/` | Low |
| **8** | Worker auth (token validation on WS connect). `z8s join` CLI. | `src/cli/join.rs`, `src/store/ws.rs` | Low |
| **9** | Remove old `ResourceStore` in-memory, old `ProcessTracker` status, old `AnyResource`. Final cleanup. | Cleanup sweep (only after tests pass) | Low |
