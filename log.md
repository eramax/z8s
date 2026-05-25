# z8s Development Log

## Overall Progress

| Phase | Description | Status |
|---|---|---|
| 0 | Non-TTY exec fix + basic container execution | **Done** |
| 1 | Container isolation (user namespaces) | **Done** |
| 2 | Storage (volumes) | Not started |
| 3 | Networking (host network + port exposure) | Not started |
| 4 | Ingress controller | Not started |
| 5 | Hard resource control (cgroups v2) | Not started |
| 6 | Network isolation (optional) | Not started |
| 7 | Network policies | Not started |

---

## Phase 0 — Non-TTY Exec Fix + Basic Container Execution (Done)

### What was done
- `ExecParams` now parses `tty`, `stdin`, `stdout`, `stderr` from query string
- Added `spawn_with_pipes()` for non-TTY exec (regular stdin/stdout/stderr pipes)
- `spawn_with_pty()` path kept for TTY exec (`exec_ws_tty`)
- New `exec_ws_pipes()` uses mpsc channel + Arc<Mutex<SplitSink>> for stdout (channel 1) / stderr (channel 2)
- Added `build_command()` helper: uses `chroot` when root, `current_dir(rootfs)` when non-root
- Added `is_root()` using `nix::unistd::Uid::effective().is_root()` (no unsafe)
- Moved data dirs to `~/.local/share/z8s/{images,rootfs,containers}` when non-root (`z8s_base_dir()`)
- Added `"user"` feature to nix in Cargo.toml

### How it was tested
- All 23 integration tests pass (`./test.sh`)
- Non-TTY exec verified manually: `kubectl exec ubuntu -- /bin/echo hello`
- Non-TTY exec verified manually: `kubectl exec ubuntu -- /bin/ls /`
- Interactive TTY exec verified manually: `kubectl exec -it ubuntu -- bash`
- `cargo build` succeeds, no compilation errors

---

## Phase 1 — Container Isolation (User Namespaces) (Done)

### What was done
- Combined `unshare(CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWUTS)` in child process
- Parent writes UID/GID maps (`0 <host_uid> 1`, `deny` setgroups)
- Child calls `mount(MS_PRIVATE|MS_REC)` to make mount tree private
- Bind-mount /proc, /sys, device nodes from host into rootfs BEFORE pivot_root
- Pre-create device files (`/dev/null`, etc.) as empty regular files for bind-mount targets
- `pivot_root()` into rootfs, detach old root
- Mount tmpfs on `/tmp`, `/run`, `/dev/shm`
- Mount devpts on `/dev/pts`
- `dup2 /dev/null` to stdin instead of `close(0)` (close(0) + pivot_root + userns causes SIGABRT in glibc)
- Zombie reaping in reconcile loop (`waitpid(-1, WNOHANG)`)
- Cascading deployment deletion (stops owned pods)
- Manifest fix: `sleep infinity` → `sleep 999999999` (uutils coreutils doesn't support "infinity")
- All 38 tests pass (35 core + 3 Phase 1 isolation tests)

### Key findings
- `unshare` must be combined (`NEWUSER|NEWNS|NEWUTS`) — separate calls fail with EPERM
- After `unshare(NEWUSER)`, calling process has UID 65534 (nobody) even after uid_map written — kernel only maps UID for new `clone()`'d processes, not the calling one. Capabilities (`=ep`) are still granted.
- `mknod` blocked for non-root even outside userns on this kernel — device nodes bind-mounted from host instead
- close(0) after pivot_root in userns causes ALL forked+exec'd child processes to SIGABRT — fixed by dup2 /dev/null to stdin
- `/dev/null` must exist as destination file before bind-mount — created as empty regular file in `prepare_rootfs`

### How it was tested
- `kubectl exec ubuntu -- whoami` returns `root`
- `kubectl exec ubuntu -- id` shows `uid=0(root) gid=0(root)`
- `kubectl exec ubuntu -- cat /proc/self/uid_map` shows root mapping
- `/proc/1/exe` in container is not host systemd — container has its own /proc
- No zombie children — container process stays alive (`sleep 999999999`)
- All 38 integration tests pass (`./test.sh`)

---

### Phase 2 — Storage (Volumes)

**Goal:** Support hostPath, emptyDir, ConfigMap, and Secret volumes.

**hostPath:**
- Bind-mount a host directory into the container's mount namespace
- Implementation: `mount --bind /host/path /rootfs/container/path` before `pivot_root`
- Works with user namespaces because we own both paths

**emptyDir:**
- Create a temporary directory on host, bind-mount into container
- Lifecycle: create on pod start, delete on pod stop
- Optionally backed by tmpfs for in-memory emptyDir

**ConfigMap (as environment variables):**
- No mount needed — inject as environment variables into the container process
- Already partially supported via container spec

**ConfigMap (as volumes):**
- Materialize ConfigMap data as files in a temporary directory
- Bind-mount that directory into the container at the specified mount path
- Read-only bind mount (`MS_BIND | MS_RDONLY`)

**Secret (as volumes):**
- Same approach as ConfigMap volumes
- Store secret data on host at `~/.local/share/z8s/secrets/<name>/`
- Bind-mount into container, ideally with `MS_RDONLY`

**Persistent Volume / Persistent Volume Claim:**
- For single-node z8s, PV is effectively a named hostPath
- PVC binds to a PV by matching size/access mode
- Implementation: PV path → hostPath → bind-mount

**Data directory layout:**
```
~/.local/share/z8s/
├── images/
├── rootfs/
├── containers/
├── volumes/          # NEW: persistent volume data
│   └── <pv-name>/
├── secrets/          # NEW: secret data
│   └── <secret-name>/
├── configmaps/       # NEW: configmap data
│   └── <configmap-name>/
└── emptydir/         # NEW: temporary volumes
    └── <pod-uid>-<volume-name>/
```

**How to test:**
```bash
# 1. hostPath volume test
kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: hostpath-test
spec:
  containers:
  - name: test
    image: ""
    command: ["/bin/sleep"]
    args: ["300"]
    volumeMounts:
    - name: host-vol
      mountPath: /data
  volumes:
  - name: host-vol
    hostPath:
      path: /tmp/z8s-test-hostpath
EOF

# Verify: write from container, read from host
kubectl exec hostpath-test -- sh -c 'echo hello > /data/test.txt'
cat /tmp/z8s-test-hostpath/test.txt
# Expected: hello

# 2. ConfigMap as env var test
kubectl apply -f - <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: test-config
data:
  key1: value1
  key2: value2
EOF

kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: configmap-env-test
spec:
  containers:
  - name: test
    image: ""
    command: ["/bin/sleep"]
    args: ["300"]
    envFrom:
    - configMapRef:
        name: test-config
EOF

kubectl exec configmap-env-test -- env | grep key1
# Expected: key1=value1

# 3. ConfigMap as volume test
kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: configmap-vol-test
spec:
  containers:
  - name: test
    image: ""
    command: ["/bin/sleep"]
    args: ["300"]
    volumeMounts:
    - name: config-vol
      mountPath: /etc/config
  volumes:
  - name: config-vol
    configMap:
      name: test-config
EOF

kubectl exec configmap-vol-test -- cat /etc/config/key1
# Expected: value1

# 4. Secret test (same pattern as ConfigMap)
kubectl apply -f - <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: test-secret
type: Opaque
data:
  password: c3VwZXJzZWNyZXQ=
EOF

kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: secret-test
spec:
  containers:
  - name: test
    image: ""
    command: ["/bin/sleep"]
    args: ["300"]
    volumeMounts:
    - name: secret-vol
      mountPath: /etc/secrets
  volumes:
  - name: secret-vol
    secret:
      secretName: test-secret
EOF

kubectl exec secret-test -- cat /etc/secrets/password
# Expected: supersecret

# 5. emptyDir test (shared between containers)
kubectl apply -f - <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: emptydir-test
spec:
  containers:
  - name: writer
    image: ""
    command: ["/bin/sh"]
    args: ["-c", "echo shared-data > /shared/file && sleep 300"]
    volumeMounts:
    - name: shared
      mountPath: /shared
  - name: reader
    image: ""
    command: ["/bin/sleep"]
    args: ["300"]
    volumeMounts:
    - name: shared
      mountPath: /shared
  volumes:
  - name: shared
    emptyDir: {}
EOF

kubectl exec emptydir-test -c reader -- cat /shared/file
# Expected: shared-data

# 6. PV/PVC test
kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolume
metadata:
  name: test-pv
spec:
  capacity:
    storage: 1Gi
  accessModes:
  - ReadWriteOnce
  hostPath:
    path: /tmp/z8s-test-pv
EOF

kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: test-pvc
spec:
  accessModes:
  - ReadWriteOnce
  resources:
    requests:
      storage: 1Gi
EOF

# Run integration tests
./test.sh
```

---

### Phase 3 — Networking (Host Network + Port Exposure)

**Goal:** Enable containers to communicate with the outside world and expose services.

**Strategy: Host network mode (no CLONE_NEWNET)**

This is the simplest and fastest approach for single-node. The container shares the host's network namespace. This is what single-node k3s does by default.

**Port exposure (Services):**
- z8s supervisor starts a TCP proxy on the host for each exposed port
- Proxy forwards host:port → container:port
- This replaces the need for iptables NAT rules
- Implementation: use `tokio::net::TcpListener` + `tokio::net::TcpStream` for proxying

**Service types:**
- `ClusterIP`: Virtual IP assigned by z8s, TCP proxy routes to container
- `NodePort`: Bind on host port (30000-32767 range), proxy to container
- `LoadBalancer`: For single-node, equivalent to NodePort (no external LB)
- `ExternalName`: DNS CNAME record (handled in DNS layer, not networking)

**DNS:**
- Run a simple DNS server (or use embedded CoreDNS) that resolves service names
- `<service-name>.<namespace>.svc.cluster.local` → ClusterIP
- For single-node, can use `/etc/hosts` injection or a local DNS resolver

**Ingress:**
- Run an HTTP reverse proxy (nginx, or built-in) that routes by Host header
- Ingress rules: `host → service → container port`
- For single-node z8s, can be a built-in layer-7 proxy using `hyper` or `tokio`

**Network mode flag:**
- `host` (default for single-node): share host netns, no isolation, best performance
- `none`: loopback only, no external connectivity
- `bridge` (future phase 6): isolated netns with pasta/slirp

---

### Phase 4 — Ingress Controller

**Goal:** Route external HTTP/HTTPS traffic to containers based on host/path rules.

**Ingress resource model (Kubernetes-compatible):**
```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-app
spec:
  rules:
  - host: myapp.example.com
    http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: my-app
            port:
              number: 8080
```

**Implementation options:**
1. **Built-in proxy (recommended for z8s):** Use `hyper` or `tokio` to implement a simple L7 reverse proxy. Route by Host header to backend service port. Keeps z8s self-contained.
2. **Sidecar nginx:** Generate nginx config from Ingress rules, run nginx as a container. Requires nginx binary in an image.
3. **External proxy:** User runs their own reverse proxy pointing to NodePort services.

**TLS:**
- Terminate TLS at the ingress proxy
- Store TLS certs as Kubernetes Secrets (tls.crt, tls.key)
- Support self-signed certs for development

**Default backend:**
- Return 404 for unmatched requests
- Optionally allow user to configure a default backend service

---

### Phase 5 — Hard Resource Control (cgroups v2)

**Goal:** Enforce RAM and CPU limits on containers as non-root.

**How cgroups v2 delegation works for non-root:**
1. systemd creates a cgroup subtree for each user session at login:
   `/sys/fs/cgroup/user.slice/user-<uid>.slice/session-<id>.scope/`
2. The user (us) can create sub-cgroups under this delegated subtree
3. We create one cgroup per container: `.../z8s/<container-id>/`
4. Write resource limits to the cgroup files:
   - `memory.max` — RAM limit in bytes
   - `cpu.max` — CPU quota (e.g., `100000 100000` = 1 CPU)
   - `pids.max` — max number of processes

**Requirements:**
- Kernel must be using cgroup v2 (unified hierarchy)
- systemd must delegate controllers to the user (`systemd-run --user` or manual delegation)
- Must have write access to `$XDG_RUNTIME_DIR` cgroup path

**API design:**
```rust
struct CgroupConfig {
    memory_max: Option<u64>,   // bytes
    cpu_max_quota: Option<u64>, // microseconds per period
    cpu_max_period: Option<u64>, // microseconds
    pids_max: Option<u64>,     // max processes
}

fn create_container_cgroup(container_id: &str, config: &CgroupConfig) -> Result<CgroupPath>
fn move_process_to_cgroup(pid: Pid, cgroup: &CgroupPath) -> Result<()>
fn destroy_cgroup(cgroup: &CgroupPath) -> Result<()>
```

**Fallback when cgroups unavailable:**
- When running as root: use system cgroup path directly
- When non-root without delegation: log warning, skip resource limits
- Detect at startup: check `/sys/fs/cgroup/cgroup.controllers` and delegation

**Kubernetes resource model mapping:**
- `resources.requests.memory` → advisory, used for scheduling (N/A for single-node)
- `resources.limits.memory` → `memory.max`
- `resources.limits.cpu` → `cpu.max` (e.g., `500m` → 50000/100000 = 0.5 CPU)

---

### Phase 6 — Network Isolation (Optional)

**Goal:** Optional per-container network namespace with outbound connectivity.

**Why optional:**
- Single-node z8s works perfectly with host network (Phase 3)
- Network isolation adds complexity and performance overhead
- Only needed when users want strong network isolation between containers

**Implementation: `CLONE_NEWNET` + pasta (or slirp4netns)**

When user opts in to network isolation:
1. `unshare(CLONE_NEWNET)` creates a new network namespace
2. Only loopback interface exists initially (no connectivity)
3. Launch `pasta` (preferred) or `slirp4netns` to provide outbound connectivity

**pasta (preferred):**
- Modern replacement for slirp4netns
- Faster performance, lower latency
- Used by Podman 5+ by default
- Provides: outbound TCP/UDP, DNS, DHCP-like addressing
- Command: `pasta --pid <container-pid> --config-net`

**slirp4netns (fallback):**
- More widely available in distro packages
- Userspace TCP/IP stack (translates Ethernet → socket syscalls)
- Command: `slirp4netns --mtu 65520 <container-pid> tap0`

**Network mode in container spec:**
- `host` (default): share host network namespace
- `none`: loopback only, no outbound
- `bridge` / `pasta`: isolated netns with pasta for outbound
- `slirp`: isolated netns with slirp4netns for outbound

**Port forwarding with isolated networks:**
- `pasta` supports `-t <host-port>:<container-port>` for TCP forwarding
- `slirp4netns` supports `--port-driver` + `slirp4netns.sh` for port forwarding
- Alternative: userspace TCP proxy on host side

---

### Phase 7 — Network Policies

**Goal:** Restrict network traffic between containers and to/from external hosts.

**Kubernetes NetworkPolicy model:**
```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: deny-all
  namespace: default
spec:
  podSelector: {}
  policyTypes:
  - Ingress
  - Egress
```

**Implementation with nftables:**
- Inside a user namespace with `CLONE_NEWNET`, we have `CAP_NET_ADMIN`
- This means we can run `nftables` inside our own network namespace
- Create nftables rules to filter traffic:
  - Ingress rules: allow/deny incoming connections by source IP/port
  - Egress rules: allow/deny outgoing connections by destination IP/port
- Apply rules before starting the container process

**Prerequisites:**
- Requires Phase 6 (network isolation) — need separate netns per container
- If using host network mode, network policies are not applicable (would affect host)

**Implementation approach:**
1. Parse NetworkPolicy resources
2. For each container with policies, generate nftables ruleset
3. Apply nftables rules in the container's network namespace
4. Use `nft` command or nftables netlink API (via `nftnl` crate or shell out)

**Policy types:**
- `Ingress`: filter incoming traffic (from other pods, external)
- `Egress`: filter outgoing traffic (to other pods, external, DNS)
- Default deny: start with deny-all, then allow specific exceptions

**Limitations for single-node:**
- No cross-node policy enforcement (N/A for single-node)
- DNS egress must always be allowed (otherwise pods can't resolve service names)

---

## Key Technical References

### User Namespace Setup
- `clone3()` or `fork()` + `unshare(CLONE_NEWUSER)`
- Write `/proc/self/uid_map`: `0 <host-uid> 1`
- Write `/proc/self/setgroups`: `deny`
- Write `/proc/self/gid_map`: `0 <host-gid> 1`
- For multi-UID mapping: read `/etc/subuid` and `/etc/subgid`

### Mount Namespace + pivot_root
```
mkdir /new-root
mount --bind /path/to/rootfs /new-root
mkdir /new-root/old-root
pivot_root /new-root /new-root/old-root
cd /
umount -l /old-root
rmdir /old-root
```

### OverlayFS (for layered images)
- Supported in user namespaces since kernel 5.11
- `mount -t overlay overlay -o lowerdir=...,upperdir=...,workdir=... /merged`
- Fallback: `fuse-overlayfs` for older kernels

### Rootless Networking
- slirp4netns: userspace TCP/IP, slower but universal
- pasta: modern replacement, faster, Podman default since v5
- Host network: simplest, best performance, no isolation
- lxc-user-nic: setuid binary for veth pairs (needs `/etc/lxc/lxc-usernet`)

### cgroups v2 Non-Root
- Delegated subtree: `/sys/fs/cgroup/user.slice/user-<uid>.slice/`
- Can create sub-cgroups and set limits
- Controllers: `memory`, `cpu`, `pids`
- Requires systemd delegation or manual `+x` write permission

---

## Relevant Files

| File | Purpose |
|---|---|
| `src/server/exec.rs` | Exec handler, PTY/pipe branching, `build_command()`, `is_root()`, `send_exit_status()` |
| `src/container/image.rs` | `z8s_base_dir()`, image pull/unpack, rootfs paths |
| `src/supervisor/process.rs` | Container spawning, `z8s_base_dir()`, cgroup integration |
| `Cargo.toml` | Dependencies including `nix` with `"user"` feature |

## Data Directory Layout

```
~/.local/share/z8s/           # non-root (or /var/lib/z8s/ as root)
├── images/                    # pulled image layers
├── rootfs/                    # extracted rootfs per image
├── containers/                # container state and metadata
├── volumes/                   # (Phase 2) persistent volumes
├── secrets/                   # (Phase 2) secret data
├── configmaps/                # (Phase 2) configmap data
└── emptydir/                  # (Phase 2) ephemeral volumes
```
