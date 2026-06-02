# Module Plan: CRI (Container Runtime Interface)

> Part of [00-overview.md](./00-overview.md) · Current ~4,100 LOC (`cri/*`)

**Runtime entry:** only the [scheduler orchestrator](./05-scheduler.md) calls `RuntimeProvider` (`SyncPod`, shutdown). API does not spawn containers except optional exec subresource proxy.

## Current state

| File | LOC | Role |
|------|-----|------|
| `runtime.rs` | 1,396 | ProcessSupervisor, fork/spawn, port publish, restart policy |
| `rootfs.rs` | 1,023 | pivot_root, mounts, userns, landlock hooks |
| `exec.rs` | 915 | kubectl exec v4/v5 WebSocket |
| `image.rs` | 459 | OCI pull, layer extract |
| `volumes.rs` | 274 | emptyDir, hostPath, configMap, secret |
| `cgroup.rs` | 171 | cgroups v2 |
| `health.rs` | 122 | Probes |
| `oci.rs`, `spec.rs` | 144 | OCI config |

### Strengths

- `IsolationStrategy` enum (`RootNs`, `UserNs`) already sketched in `runtime.rs`.
- Sync pipe handshake between parent/child for namespace setup.
- NetMux integration: pod IP, veth ifindex on `ContainerInstance`.
- Graceful degradation when not root / AppArmor blocks userns.

### Problems

1. **Three near-duplicate spawn paths** — full root, userns, degraded native; ~70% shared setup duplicated.
2. **Image unpack copies full rootfs per container** — no overlay; replicas multiply disk I/O and spawn latency.
3. **Blocking work on async runtime** — mount, unpack sometimes in async context without consistent `spawn_blocking`.
4. **exec.rs is 915 LOC** — protocol state machine mixed with PTY setup.
5. **No capability profile registry** — `extra_caps` ad hoc; host services (dhcp, sshd) need declarative profiles.
6. **Landlock imported but underused** — opportunity for filesystem allowlists per profile.
7. **Logs** — in-memory `Vec` per container (1000 lines); no backpressure or shared ring.

## Target: production CRI (~1,200 LOC)

### Architecture

```
┌──────────────────────────────────────────────────┐
│ ContainerRuntime (facade, impl RuntimeProvider)   │
├────────────┬────────────┬────────────┬───────────┤
│  Spawner   │ ImageStore │ CgroupMgr  │ ExecSvc   │
│  (fork)    │ (overlay)  │ (v2)       │ (ws/pty)  │
├────────────┴────────────┴────────────┴───────────┤
│ rootfs::RootfsBuilder (pipeline of MountStep)     │
│ health::ProbeRunner (shared tokio tasks)            │
└──────────────────────────────────────────────────┘
```

### 1. Unified `Spawner` (Pipeline pattern)

```rust
pub struct SpawnPipeline {
    steps: Vec<Box<dyn SpawnStep>>,
}

#[async_trait]
trait SpawnStep {
    async fn apply(&self, ctx: &mut SpawnCtx) -> Result<()>;
}
```

Ordered steps (strategy selects subset):

1. `PrepareRootfs` — overlay lower from image cache
2. `CreateSyncPipes`
3. `Fork` — `IsolationStrategy` picks clone flags
4. `ChildSetup` — hostname, mounts, drop caps
5. `NetAttach` — NetMux veth + routes (if `isolate_net`)
6. `CgroupAttach`
7. `ExecProcess` — execvpe
8. `ParentWaitAck`

**Single fork implementation** — strategy only changes `clone_flags` and cap set.

**LOC:** ~280 (down from ~900 in runtime.rs).

### 2. OverlayFS image store (Builder pattern)

```rust
pub struct ImageStore {
    cache_dir: PathBuf,  // /var/lib/z8s/layers/<digest>
}

impl ImageStore {
    pub async fn prepare_rootfs(&self, image: &str, id: &str) -> Result<PathBuf> {
        // lower: merged layers (read-only, shared)
        // upper/work: per-container in /var/lib/z8s/overlay/<id>
    }
}
```

- **First pull:** extract layers once to lowerdir.
- **Replica pods:** share lowerdir; only upper/work differs.
- **Fallback:** recursive copy if overlay mount fails (one code path, logged once).

Optional **io_uring** (feature `io-uring`): parallel layer extract via `tokio_uring` or `rio` — only for bulk extract, not required for MVP.

**LOC:** ~200 (image.rs + rootfs mount helpers shrink).

### 3. `RootfsBuilder` (compose mount steps)

```rust
enum MountStep {
    Bind { src, dst, readonly },
    Tmpfs { dst, size },
    ConfigMap { name, keys },
    Secret { name },
    ResolvConf { dns, search },
}
```

`rootfs.rs` becomes ~350 LOC of small functions, not nested 200-line functions.

### 4. Capability profiles (new: `cri/capability.rs` ~80 LOC)

```yaml
# z8s.io/v1 CapabilityProfile
name: host-sshd
spec:
  kind: host          # host | container | privileged
  bounding: [CHOWN, SETUID, NET_BIND_SERVICE, ...]
  ambient: []
  no_new_privs: false
```

Pod annotation `z8s.io/cap-profile: host-sshd` resolved at admission → `ContainerSpawnCtx`.

Predefined profiles:

| Profile | Use case |
|---------|----------|
| `container-minimal` | Default workload |
| `host-native` | `image: ""` on host |
| `host-dhcp` | DHCP client caps |
| `host-sshd` | sshd |
| `privileged` | maps to current `privileged: true` |

### 5. Exec service split

- `exec/protocol.rs` — frame encode/decode (~200 LOC)
- `exec/session.rs` — PTY + pipe (~150 LOC)
- Reuse sync pipe pattern only where needed

### 6. Health probes

- One `ProbeRunner` task per container with merged probe schedule (min heap by next fire time).
- HTTP/TCP probes use `tokio::net` with timeout; no blocking threads.

### 7. PID 1 interaction

When `pid == 1`:

- Init reaps zombies globally; CRI child processes in PID namespace don't double-reap.
- Host-native processes (`image: ""`) remain in init's namespace — document that they share init's signal disposition.
- **Supervision:** optional `z8s.io/init: true` on Pod → restart policy `Always` + ordered shutdown on SIGTERM (facade coordinates with `init.rs`).

### 7b. Nested z8s — PID 1 inside a z8s pod ([00-overview](./00-overview.md))

**Use case:** Prove z8s is a valid **container init** and nested control plane: host z8s runs pod `z8s-nested`; inner z8s is PID 1 and schedules child pods.

| Concern | Approach |
|---------|----------|
| **Entry** | `command: ["/z8s", "run", "--daemon"]` — server blocks in container; init handler on SIGTERM |
| **Z1 CLI** | If `pid==1` and no args → auto-run server (no help+exit) |
| **Privileges** | `securityContext.privileged: true` (or full cap set + `/dev/net/tun`) |
| **cgroups** | Mount host `/sys/fs/cgroup` read-write or use cgroup namespace + delegated cgroup subtree |
| **Storage** | `emptyDir` at `/var/lib/z8s` (images, redb, rootfs) — isolated from host paths |
| **Network** | Inner pod has its own netns; inner z8s creates veth for **its** pods inside that netns stack — document double-netns limits |
| **CIDR** | Args/env: `--pod-cidr 10.201.0.0/16 --service-cidr 10.111.0.0/16` to avoid clashing with host cluster |
| **Locks** | Use `/var/lib/z8s/run/*.lock` inside emptyDir (not host `/tmp/z8s.lock`) |

**Container image (`deploy/z8s/Dockerfile`):**

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates iptables iproute2 \
    && rm -rf /var/lib/apt/lists/*
COPY target/release/z8s /z8s
# Optional: AppArmor profile, landlock off in nested mode
ENTRYPOINT ["/z8s"]
CMD ["run", "--daemon"]
```

**Test workload (applied to inner API):**

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: nested-nginx
  namespace: default
spec:
  replicas: 1
  selector:
    matchLabels:
      app: nested-nginx
  template:
    metadata:
      labels:
        app: nested-nginx
    spec:
      containers:
        - name: nginx
          image: docker.io/library/nginx:alpine
          ports:
            - containerPort: 80
```

**Pass criteria** ([tests/test-z8s-nested-pid1.sh](../../tests/test-z8s-nested-pid1.sh)):

1. Host pod `z8s-nested` reaches Running; logs show `z8s is ready`.
2. `curl http://127.0.0.1:16443/healthz` (hostPort) → `ok`.
3. `kubectl --server=http://127.0.0.1:16443 --insecure-skip-tls-verify apply` nested-nginx Deployment.
4. Inner `kubectl get pods` shows `nested-nginx-*` Running.
5. Cleanup; host pod terminates cleanly on delete (no D-state hang > 15s).

**Scheduler note:** Inner cluster uses same orchestrator rules ([05-scheduler.md](./05-scheduler.md)) — single-node inner needs no join token; store on emptyDir redb.

**Phase:** **Z1** (CLI) + **H3** (E2E); depends on **O2** (scheduler owns CRI) and **N2** (pod attach in inner netns).

### 8. True isolation checklist

| Layer | Current | Target |
|-------|---------|--------|
| Network | veth + netns | ✓ enforce `isolate_net` default true for `image != ""` |
| PID | optional PID ns | Default PID ns for containers; host profile skips |
| Mount | pivot_root / chroot | Overlay + read-only root where possible |
| cgroup | memory/cpu | Add `pids.max`, `io.max` when available |
| Disk | loop PV | xfs project quotas or loop mount size |
| MAC | partial landlock | Landlock rules per profile on `/etc`, `/proc` |

## LOC budget

| Area | Current | Target |
|------|---------|--------|
| runtime.rs | 1,396 | 280 (spawner + supervisor loop) |
| rootfs.rs | 1,023 | 350 |
| exec.rs | 915 | 350 |
| image.rs | 459 | 200 |
| other | 307 | 120 |
| **Total CRI** | **~4,100** | **~1,200** |

## Performance targets

| Metric | Single-node | 2-node (local pods only) |
|--------|-------------|---------------------------|
| Cold start 1 container (cached image) | < 200 ms | < 250 ms |
| 10-nginx replica rollout | < 5 s | < 6 s |
| Image pull nginx (cold) | network bound | same |

## ServiceAccount token mount (RBAC)

When `pod.spec.serviceAccountName` is set, CRI mounts after rootfs ready:

| Path | Purpose |
|------|---------|
| `/var/run/secrets/z8s.io/serviceaccount/token` | Bearer for API |
| `namespace`, `name` | SA identity |
| `kubeconfig` | Minimal config for in-pod `kubectl` (cluster-dashboard E2E — [07-rbac.md](./07-rbac.md)) |

Token issued by `auth` module; revoked on pod stop. API server URL → `https://kubernetes.default.svc.cluster.local` (in-cluster EdgeService).

See [07-rbac.md](./07-rbac.md). **~60 LOC** in `cri/sa_mount.rs` + **~40 LOC** `kubeconfig` template.

## Implementation order

1. **Z1** — PID1 with no argv → `run --daemon` foreground (unblocks container image).
2. Extract `SpawnPipeline` without changing behavior (refactor only).
2. Add overlay mount path behind config flag `--overlay`.
3. Capability profiles + admission default.
4. Split exec module.
5. Probe runner consolidation.
6. Remove dead code paths (legacy port publish on 127.0.0.1 if pod IP routing complete).

## Dependencies

- Keep `nix`, `caps`, `oci-distribution`, `flate2`, `tar`.
- Optional: `capsicum` if moving beyond landlock on FreeBSD targets (future).
- Drop unused imports after landlock strategy decided.

---

*Previous: [01-api.md](./01-api.md) · Next: [03-storage.md](./03-storage.md)*
