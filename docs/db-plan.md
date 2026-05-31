# DB Resource Store + Raft Cluster + Scheduler

## Architecture: Raft Cluster (All Servers Equal)

Like k3s embedded etcd. All server nodes form a raft cluster. Each has its own local redb. Writes go through raft consensus. Each server serves the full API + kubectl.

```
┌─────────────────┐   ┌─────────────────┐   ┌─────────────────┐
│  Server Node A   │   │  Server Node B   │   │  Server Node C   │
│                  │   │                  │   │                  │
│  ┌────┐  ┌────┐  │   │  ┌────┐  ┌────┐  │   │  ┌────┐  ┌────┐  │
│  │redb│  │raft│  │◀──▶│  │redb│  │raft│  │◀──▶│  │redb│  │raft│  │
│  │    │  │    │  │ WS │  │    │  │    │  │ WS │  │    │  │    │  │
│  └────┘  └────┘  │   │  └────┘  └────┘  │   │  └────┘  └────┘  │
│                  │   │                  │   │                  │
│  API :6443       │   │  API :6443       │   │  API :6443       │
│  WS  :9876       │   │  WS  :9876       │   │  WS  :9876       │
└────────┬─────────┘   └────────┬─────────┘   └────────┬─────────┘
         │                      │                      │
         └──────────────────────┼──────────────────────┘
                                │
                           ┌────┴────┐
                           │ Worker  │
                           │ (cache) │
                           └─────────┘
```

**Servers** = API + raft + redb (the full state machine)
**Workers** = connect to any server via WebSocket, run pods

## Raft Implementation: `openraft` + `redb`

[`openraft`](https://github.com/datafuselabs/openraft) is the most mature Rust raft crate (used by Databend, GreptimeDB, etc.). We implement:

```rust
// Storage backend — stores raft log + state machine in redb
impl RaftStorage<...> for RedbRaftStorage {
    // Raft log: append entries, read entries, delete from index
    // State machine: apply committed entries to redb tables
    // Snapshot: full snapshot of redb → install on new/failing nodes
}
```

**redb tables + raft log:**

| redb table | Content | Consensus |
|---|---|---|
| `raft_log` | Raft log entries (term, index, data) | Raft-controlled |
| `raft_meta` | Current term, voted for, commit index | Raft-controlled |
| `resources` | Actual resource state | Applied from raft log |
| `events` | Event records | Applied from raft log |
| `stats` | Computed aggregates | Applied from raft log |
| `node_registry` | Node list + heartbeats | Applied from raft log |
| `lease` | Scheduler lease + other leases | Applied from raft log |

**Write flow:**

```
kubectl apply → Node A's API (all nodes serve API)
  → Node A (or leader) proposes to raft:
      { key: "resources/Pod/default/my-pod", value: JSON }
  → Raft consensus: majority of nodes agree
  → Raft commits the entry
  → ALL nodes apply to their local redb (state machine)
  → ALL nodes broadcast watchEvent via local WebSocket
  → Workers + local scheduler pick up the change
```

**Read flow:**
```
kubectl get pods → Node C's API
  → Node C reads its local redb directly
  → No consensus needed (raft ensures all nodes have same state)
```

## Raft Network Layer

Server-to-server communication uses **WebSocket** (already axum-native):

```
Raft message types:
  { type: "raft:vote_req",     term, candidate_id, ... }
  { type: "raft:vote_resp",    term, vote_granted, ... }
  { type: "raft:append_req",   term, leader_id, entries, ... }
  { type: "raft:append_resp",  term, success, last_log_index, ... }
  { type: "raft:install_snap", term, snapshot_data, ... }
  { type: "raft:snapshot_resp", term, success, ... }
```

On startup, each server node connects via WebSocket to every other known peer:

```rust
// src/raft/network.rs
struct RaftNetwork {
    peers: HashMap<NodeId, WebSocketSender>,
}
impl RaftNetwork for RaftNetworkLayer {
    async fn send_append_entries(&self, target, msg) -> Result<...>;
    async fn send_request_vote(&self, target, msg) -> Result<...>;
    async fn send_install_snapshot(&self, target, msg) -> Result<...>;
}
```

## Scheduler: Leader-Elected via Raft Lease

A single active scheduler cluster-wide, exactly like k8s:

```
  raft lease key: "lease/scheduler"
  TTL: 15 seconds

  Node A: holds "lease/scheduler" → active scheduler
  Node B: can't get lease → standby
  Node C: can't get lease → standby

  If Node A dies → lease expires (15s)
  Node B: gets lease → becomes active scheduler
  Node C: can't get lease → standby
```

Implementation:

```rust
// src/scheduler/mod.rs
pub async fn try_become_scheduler(raft: &RaftCluster) -> bool {
    loop {
        let acquired = raft.try_lease("lease/scheduler", Duration::from_secs(15)).await;
        if acquired {
            return true; // I'm the scheduler!
        }
        tokio::time::sleep(Duration::from_secs(5)).await; // retry
    }
}
```

The active scheduler:

1. Watches all resources via local redb
2. For Pods without `assigned_to`: picks the best node, writes `assigned_to` to raft
3. For Services: allocates ClusterIP, writes to raft
4. All nodes see the assignment via raft consensus

Standby schedulers do nothing except watch the lease.

## Worker Nodes

Workers don't run raft. They connect to any server via WebSocket:

```
z8s join ws://10.0.0.1:9876 [--token xxx]
  → WebSocket to server A
  → Auth + receive syncFull (all state)
  → Enter watch loop (live watchEvent)

Worker has local cache-redb for fast reads.
Worker's scheduler is NOT a cluster scheduler.
Worker's scheduler only processes assignments for itself:
  WatchEvent → key "resources/Pod/default/my-pod"
    → assigned_to == this worker?
      → YES: execute (CRI + NetMux)
      → NO: skip (update local cache only)
```

## Startup Sequence

### First Server Node
```
1. Open/init redb (raft_log empty → I'm the first)
2. Start raft cluster (single node, wait for others)
3. Load manifests/ dir → apply to raft
4. Start WebSocket server :9876
5. Start HTTP API :6443
6. Try to become scheduler (raft lease)
```

### Second/Third Server Nodes
```
1. Open/init redb
2. Start raft → connect to existing peer
3. Join raft cluster → receive full snapshot
4. Apply snapshot to local redb
5. Start WebSocket server :9876
6. Start HTTP API :6443
7. Try to become scheduler (lease → likely lose)
```

### Worker Node
```
1. z8s join ws://server-a:9876
2. Receive syncFull → local cache-redb
3. Start watch loop
```

## Scaling

| # Server Nodes | Tolerates | Recommended |
|---|---|---|
| 1 | 0 failures (no HA) | Dev/test |
| 3 | 1 failure | Production minimum |
| 5 | 2 failures | Large production |
| 7+ | 3+ failures | Overkill |

## Data Flows

### kubectl apply (Pod)

```
kubectl apply -f pod.yaml → Node B's API :6443
  │
  ├── ① Node B: propose to raft
  │     { key: "resources/Pod/default/my-pod", value: JSON }
  │
  ▼
Raft: majority of {A, B, C} agree
  │
  ├── ② ALL servers apply to local redb (state machine)
  ├── ③ ALL servers broadcast watchEvent via WebSocket
  │
  ▼
Server A (active scheduler, holds lease):
  ④ Sees Pod without assigned_to
  ⑤ Picks best node → assigns to Node C
  ⑥ Proposes to raft: update Pod with assigned_to: node-c
  │
  ▼
Raft commits →
  ├── ALL servers apply
  ├── ALL servers broadcast watchEvent
  │
  ▼
Worker C (via WebSocket to any server):
  ⑦ Sees assigned_to: node-c
  ⑧ Executes: CRI + NetMux
  ⑨ Writes Pod status back to raft
```

### kubectl get pods (all servers serve reads)

```
kubectl get pods → Node C's API :6443
  → Node C reads local redb directly
  → Returns all Pod resources
  → Stats come from redb stats table
  → No raft involvement for reads
```

### Leader Node (Scheduler) Crashes

```
Node A (scheduler) crashes
  → Raft detects Node A is dead (no append_entries response)
  → Raft elects new leader (Node B)
  → 15s lease timeout on "lease/scheduler"
  → Node B acquires lease → becomes active scheduler
  → Node B re-reads all Pods, assigns unassigned ones
```
**Scheduler downtime: ~15 seconds.** Then normal operation resumes.

### Server Node Dies (Raft Minority)

```
Node A dies (3-node cluster)
  → Nodes B, C still have majority → raft continues
  → Workers still connected to B or C via WebSocket
  → Scheduler lease held by B (or C) → scheduler continues
```
**No downtime.** Remaining nodes serve all requests.

### Server Node Dies (Raft Majority)

```
Two nodes die in a 3-node cluster
  → Remaining node can't form majority
  → Raft stops accepting writes
  → Local reads still work (API reads)
  → Existing pods keep running (nftables, containers)
  → When one node recovers → raft resumes
```
**Read-only mode.** Writes resume when majority returns.

## Implementation Phases

| Phase | What | Key Files |
|---|---|---|
| **1** | redb local: replace ResourceStore HashMap, all 6 tables | `Cargo.toml` (+redb), `src/store/db.rs`, `src/types.rs` |
| **2** | openraft storage backend + raft cluster | `Cargo.toml` (+openraft), `src/raft/storage.rs`, `src/raft/network.rs`, `src/raft/mod.rs` |
| **3** | WebSocket for raft + worker connections | `src/store/ws.rs`, raft messages over WS |
| **4** | Raft-synced state machine (resources, events, stats) | `src/raft/state_machine.rs` |
| **5** | Leader-elected scheduler via raft lease | `src/scheduler/mod.rs`, lease on "lease/scheduler" |
| **6** | `z8s join` CLI for workers | `src/cli/join.rs` |
| **7** | Manifesto dir → raft apply on startup | `src/manifest/watcher.rs` |
| **8** | Remove old in-memory store, enrichment, Component hooks | Cleanup |
