# Module Plan: Store & Sync

> Part of [00-overview.md](./00-overview.md) · Current ~1,050 LOC (`store/*`)

## Current state

```
store/
├── backend.rs      # StoreBackend trait
├── memory.rs       # 66 LOC — in-memory (main default without redb path)
├── db.rs           # 315 LOC — RedbBackend, spawn_blocking per op
├── gossip.rs       # 191 LOC — batch queue, broadcast
├── ws.rs           # 319 LOC — WebSocket client/server, message dispatch
├── anti_entropy.rs # 45 LOC — periodic checksum
├── leases.rs       # 137 LOC — scheduler lease + heartbeat
├── events.rs       # 68 LOC — Event recording
└── mod.rs
```

### Strengths

- `StoreBackend` async trait — clean abstraction.
- `GossipMessage::BatchGossip` — batched writes (good direction).
- Anti-entropy + `SyncRequest` / `SyncFull` for recovery.
- Workers filter gossip applies: only start pods assigned to local node (`ws.rs`).
- redb persistence optional per node.

### Problems

1. **JSON serialize entire `AnyResource` per key** — large payloads, slow apply.
2. **spawn_blocking per apply/delete** — serializes writes; no transaction batching API.
3. **Lease in local redb** — not cluster-authoritative (scheduler bug).
4. **Full sync on connect** — `SyncRequest` pulls everything; expensive at scale.
5. **No resourceVersion monotonic** — watchers and conflict detection weak.
6. **Memory backend on main** — if no `--data-dir`, cluster state lost on restart.
7. **Duplicate store reads** — no snapshot/cache for reconcile planners.

## Target store layer (~500 LOC)

### Architecture

```
┌─────────────────────────────────────────────┐
│ StoreFacade                                  │
│  · apply_batch(Vec<Op>)                      │
│  · snapshot() → StoreSnapshot                │
│  · watch(Kind) → Stream<Event>               │
├──────────────┬──────────────────────────────┤
│ RedbStore    │ GossipTransport (ws)         │
│ (local)      │ AntiEntropy                  │
├──────────────┴──────────────────────────────┤
│ ClusterMeta table (main only)                │
│  · scheduler_lease                           │
│  · cluster_config                            │
│  · join_tokens/{node_name}                   │
└─────────────────────────────────────────────┘
```

### 1. StoreBackend v2 (minimal breaking change)

Add to trait:

```rust
async fn apply_batch(&self, ops: Vec<StoreOp>) -> Result<()>;
async fn snapshot(&self) -> StoreSnapshot;
fn resource_version(&self) -> u64;
```

`StoreOp` = `Upsert(AnyResource) | Delete(uid)`.

Default impl loops apply — Redb overrides with single txn.

### 2. Redb optimizations

```rust
async fn apply_batch(&self, ops: Vec<StoreOp>) -> Result<()> {
    spawn_blocking(|| {
        let mut txn = db.begin_write()?;
        let mut table = txn.open_table(RESOURCES)?;
        for op in ops {
            match op { ... }
        }
        txn.commit()?;
    })
}
```

**Tables:**

| Table | Key | Value |
|-------|-----|-------|
| `resources` | uid | bincode or postcard blob |
| `nodes` | node_name | NodeRecord |
| `leases` | `scheduler` | LeaseRecord (cluster) |
| `join_tokens` | `{node_name}` | JoinTokenRecord (hash only) |
| `events` | event_uid | Event (ring buffer index) |
| `meta` | `resource_version` | u64 |

Consider **postcard** over JSON for 2–3× smaller/faster (keep JSON read compat migration pass).

### 3. Cluster scheduler lease (main redb only)

```rust
// leases.rs
const SCHEDULER_LEASE_KEY: &str = "scheduler";

// Only main node runs acquire_scheduler_lease()
// Workers: is_scheduler_leader() always false
```

Lease record includes `holder`, `epoch`, `expires_at_ms`.

Workers read lease via gossip **meta** message or query main API — not local acquire.

### 4. Gossip protocol v2

Keep existing message types; add:

```rust
enum GossipMessage {
    // ...
    Meta { scheduler_lease: LeaseRecord, resource_version: u64 },
    Delta { from_version: u64, ops: Vec<StoreOp> },  // future
}
```

**Phase 1 improvements (no protocol break):**

- Always use `queue_write` + periodic `flush_batch` (100ms or N=32).
- Coalesce duplicate keys in pending map before flush.
- Scheduler: one batch for N pod assignments.

**Phase 2:** Delta sync from `resource_version` instead of full `SyncFull`.

### 5. WebSocket layer (`ws.rs`)

Split:

| File | LOC | Role |
|------|-----|------|
| `ws/server.rs` | 80 | Axum upgrade, peer register |
| `ws/client.rs` | 80 | reconnect backoff |
| `ws/dispatch.rs` | 100 | handle_message |

On apply from gossip:

```rust
store.apply_batch(vec![op]).await?;
scheduler_notify.notify_one();  // scheduler enqueues tasks from StoreEvent
// NOT reconcile_all, NOT direct CRI/net/vol
```

See [05-scheduler.md](./05-scheduler.md) — store is the bus; scheduler is the sole executor.

### 6. Anti-entropy

Current 45 LOC — extend:

- Merkle or per-kind checksum (cheaper than full hash).
- Run every 30s; only sync differing keys.
- Cap bandwidth: max 100 keys per round.

### 7. Events (`events.rs`)

- Ring buffer in redb (max 1000 events) with TTL compaction task.
- Don't gossip events (local only) — reduces noise.

### 8. StoreSnapshot for NetMux / Scheduler

```rust
pub struct StoreSnapshot {
    pub version: u64,
    pub pods: Arc<[PodView]>,
    pub services: Arc<[ServiceView]>,
    // built in one read txn
}
```

Built every reconcile cycle **once** or on change notification — shared `Arc` to planner + scheduler index update.

### 9. Main vs worker persistence

| Node | DB | Contents |
|------|-----|----------|
| Main | `/var/lib/z8s/cluster.redb` | Full cluster state |
| Worker | optional cache | Local node metadata + last sync snapshot |

Workers without DB: memory + gossip hydrate (current) but **must not** run scheduler lease.

Default main to **always** use redb (warn if missing).

## Node join tokens (CLI + cluster meta)

Remote workers must authenticate to the **main** node before gossip sync. Operators obtain tokens on the main with:

```bash
z8s node <node_name> token
```

### CLI surface

| Command | Role | Runs on |
|---------|------|---------|
| `z8s node <NAME> token` | Print join token for node `NAME` (create if missing) | Main cluster host |
| `z8s node token` | Same as above for **main** node name (from global lock / config) | Main |
| `z8s node <NAME> token --rotate` | Invalidate old token, print new secret once | Main |
| `z8s node list` | List nodes + whether token exists (never print secret in list) | Main |

Extend `main.rs` / `config.rs` help:

```text
  node <NAME> token [--rotate]   Show (or create) join token for worker NAME
  join <ws-url> --token <TOKEN> Join cluster using token from main
  node start ... --join-token <TOKEN>   Pass token when starting local worker process
```

**Output format** (script-friendly):

```text
# stdout only the token on success (for $(z8s node worker-1 token))
z8s.jt.a1b2c3d4e5f6....

# stderr human hint
Join with:
  z8s join ws://<main-ip>:6443/ws/gossip --token z8s.jt....
  z8s node start --port 6444 --peers main=<host>:6443 --join-token z8s.jt....
```

### Storage model (main redb `cluster_meta` table)

```rust
pub struct JoinTokenRecord {
    pub node_name: String,           // pre-registered worker identity
    pub token_id: String,            // public id (prefix in presented token)
    pub secret_hash: [u8; 32],       // SHA-256 of raw secret; never store plaintext
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>, // None = no expiry until rotate
    pub used_at_ms: Option<i64>,     // set on first successful join (optional one-shot)
    pub revoked: bool,
}

// key: join_tokens/{node_name}
// special key: join_tokens/_cluster bootstrap (optional generic enroll token)
```

| Field | Purpose |
|-------|---------|
| `node_name` | Must match worker `--node-name` on join (prevents token reuse across names) |
| `token_id` | Allows revocation by id without scanning |
| `secret_hash` | Constant-time compare on handshake |

**Token wire form:**

```text
z8s.jt.<token_id>.<secret_base64url>
```

Generate `secret` with `getrandom` (32 bytes); show **once** on `token` / `--rotate`; DB holds hash only.

### Bootstrap (main first start)

1. Main opens `/var/lib/z8s/cluster.redb`.
2. If no `join_tokens/{main_node_name}` → create token automatically (log path to run `z8s node token`).
3. Seed `Node` object in store for main (existing behavior).

### Join authentication flow

```text
Worker                              Main
  │                                   │
  │  WS GET /ws/gossip                │
  │  Authorization: Bearer z8s.jt...  │
  ├──────────────────────────────────►│
  │                                   │ lookup hash by token_id
  │                                   │ verify secret, !revoked, !expired
  │                                   │ verify node_name claim in token or ?name=
  │◄──────────────────────────────────┤
  │  101 Switching Protocols          │
  │  SyncFull + heartbeat             │
```

**Gossip handshake** (`ws/server.rs`):

```rust
async fn authenticate_join(headers: &HeaderMap, db: &RedbBackend) -> Result<JoinIdentity> {
    let token = parse_bearer(headers)?;
    let (id, secret) = parse_join_token(&token)?;
    let rec = db.read_join_token_by_id(&id).await?;
    verify_secret(&rec.secret_hash, &secret)?;
    if rec.revoked { bail!("token revoked"); }
    Ok(JoinIdentity { node_name: rec.node_name })
}
```

Reject with **401** before `SyncFull` if invalid — worker retries with backoff (existing `join_cluster` loop).

**Optional:** query param `?node_name=worker-1` must match record when using name-bound tokens.

### Operator workflow (document in README)

```bash
# ── On main (6443) ──
sudo z8s run --daemon --port 6443
z8s node worker-east token          # creates + prints z8s.jt....
# copy token to remote host

# ── On remote worker ──
export Z8S_JOIN_TOKEN=$(ssh main 'z8s node worker-east token')
z8s node start --port 6444 \
  --node-name worker-east \
  --peers main=10.0.0.1:6443 \
  --join-token "$Z8S_JOIN_TOKEN"
# or standalone join client:
z8s join ws://10.0.0.1:6443/ws/gossip --token "$Z8S_JOIN_TOKEN"
```

`--node-name` on worker **must** match the name passed to `z8s node <NAME> token` on main.

### API (optional, admin only)

Not required for MVP if CLI reads redb locally on main host. Later:

| Method | Path | Notes |
|--------|------|-------|
| `POST` | `/api/v1/nodes/{name}/token` | Create/rotate (RBAC: cluster-admin) |
| `GET` | `/api/v1/nodes/{name}/token` | Returns token object **without** secret after creation |

Prefer **CLI-only** for secret display to avoid tokens in audit logs.

### Relation to RBAC ([07-rbac.md](./07-rbac.md))

| Token type | Purpose |
|------------|---------|
| **Join token** (`z8s.jt.*`) | Cluster membership — gossip WS only |
| **ServiceAccount token** | In-cluster Kubernetes API access |

Do not reuse join tokens for `kubectl` / pod API calls.

### Implementation phases

| Step | Deliverable |
|------|-------------|
| J0 | `JoinTokenRecord` + redb table + hash/verify helpers |
| J1 | CLI `z8s node <name> token` / `--rotate` (read main redb or `--db-path`) |
| J2 | WS auth in `handle_gossip_ws` before peer register |
| J3 | Wire `z8s join` + `node start --join-token` to send Bearer header |
| J4 | Auto-create main node token on first boot |
| J5 | `z8s node list` shows Registered / Joined / Expired |

### Testing

- Create token for `worker-a`; join succeeds with correct `--node-name`.
- Wrong secret → 401, no store writes on worker.
- Wrong `node_name` → 403.
- `--rotate` invalidates old token.
- Token not printed on second `token` call unless `--show` (security: only at create/rotate).

---

## Sync topology

```text
         ┌─────────────┐
         │  Main node  │
         │  redb (SoT) │
         │ join_tokens │
         └──────┬──────┘
                │ WebSocket gossip + Bearer z8s.jt.*
     ┌──────────┼──────────┐
     ▼          ▼          ▼
 Worker 1   Worker 2   Worker N
 (local CRI only)
```

**Join flow (`z8s join` / `z8s node start --join-token`):**

1. Operator runs `z8s node <NAME> token` on main → copies `z8s.jt.*`.
2. Worker opens WS with `Authorization: Bearer <token>`.
3. Main validates token → registers peer → `SyncFull`.
4. Worker heartbeat `NodeRecord`; scheduler assigns pods with `assigned_node=<NAME>`.
5. Scheduler on worker reconciles local pods ([05-scheduler.md](./05-scheduler.md)).

## LOC budget

| File | Current | Target |
|------|---------|--------|
| db.rs | 315 | 140 |
| gossip.rs | 191 | 100 |
| ws.rs | 319 | 180 |
| leases.rs | 137 | 60 |
| anti_entropy.rs | 45 | 40 |
| memory.rs | 66 | 40 |
| facade.rs (new) | 0 | 80 |
| join_token.rs (new) | 0 | 90 |
| **Total store/** | **~1,050** | **~590** |

## Implementation order

1. Main-only scheduler lease table + stop worker scheduler spawn.
2. **Join tokens:** `JoinTokenRecord` + `z8s node <name> token` + WS auth (J0–J3).
3. `apply_batch` for deployment scale + scheduler assign burst.
4. Gossip coalesce pending by key.
5. `StoreSnapshot` + wire to netmux planner (one read path).
6. Notify granular reconcile (with scheduler module).
7. postcard encoding (optional feature flag).
8. Delta sync protocol.

## Consistency model

Document **eventual consistency**:

- Writes visible locally immediately; peers within ~100ms (LAN).
- Scheduler leader is single writer for `assigned_node`.
- Conflict: higher `term` wins (gossip); same term → compare `resource_version`.

## Testing

- Split-brain test: two mains — second fails lease acquire.
- Partition: worker continues running local pods; resync on reconnect.
- Bench: 100 apply_batch ops < 50ms on SSD.

---

*Previous: [05-scheduler.md](./05-scheduler.md) · Back to [00-overview.md](./00-overview.md)*
