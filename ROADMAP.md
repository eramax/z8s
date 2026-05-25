# z8s Architecture Roadmap

> Last updated: 2026-05-25
>
> z8s is a minimal, self-contained Kubernetes-compatible orchestrator and init process written in
> Rust. Target: PID 1 init system for a new Linux-based OS, strong enough to supervise and control
> all system processes/services, with kubectl-compatible API and full container lifecycle management.

---

## Current State (Done)

| Phase | What was built |
|---|---|
| Phase 0 | Non-TTY exec fix, PTY exec, `build_command()`, rootless base-dir |
| Phase 1 | User namespace isolation: `NEWUSER + NEWNS + NEWUTS` + `pivot_root`, UID/GID maps, bind-mounts for `/proc`, `/sys`, `/dev`, zombie reaping |

All 38 integration tests pass. Binary is ~4 MB release build.

---

## Design Principles

- **Async everywhere** — tokio, no blocking calls on the runtime thread
- **Lightweight** — small binary, idle CPU ≈ 0, fits embedded/minimal OS
- **kubectl-compatible** — works with unmodified `kubectl`, sticks with `k8s-openapi` structs
- **Rootless-first, root-capable** — works as a regular user in dev; runs as PID 1 (root) in production
- **Defense in depth** — every container gets: namespaces + Landlock + seccomp + capability drop
- **No escape** — a container reboot/crash never touches the host; privileged mode is explicit opt-in
- **Builder pattern (narrow scope)** — only for internal resource construction and tests; API handlers use `k8s-openapi` structs directly
- **serde stays** — `k8s-openapi` requires it; `serde_json` drives the HTTP API; `serde_yaml` parses manifests

---

## Isolation Stack (per container)

Each container will go through this layered isolation sequence in the child process, from
outermost to innermost:

```
┌─────────────────────────────────────────────────────────┐
│  Linux Namespaces                                        │
│  NEWUSER + NEWNS + NEWPID + NEWNET + NEWIPC + NEWUTS    │
│  ┌───────────────────────────────────────────────────┐  │
│  │  pivot_root into OCI rootfs (overlayfs)           │  │
│  │  ┌─────────────────────────────────────────────┐  │  │
│  │  │  Landlock (filesystem + network restrict)   │  │  │
│  │  │  ┌───────────────────────────────────────┐  │  │  │
│  │  │  │  seccomp-bpf (syscall allowlist)      │  │  │  │
│  │  │  │  ┌─────────────────────────────────┐  │  │  │  │
│  │  │  │  │  Capability drop                │  │  │  │  │
│  │  │  │  │  execvp(entrypoint)             │  │  │  │  │
│  │  │  │  └─────────────────────────────────┘  │  │  │  │
│  │  │  └───────────────────────────────────────┘  │  │  │
│  │  └─────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

### Namespace flags

| Flag | Why |
|---|---|
| `CLONE_NEWUSER` | Container root maps to host UID — no real privilege |
| `CLONE_NEWNS` | Private mount tree, pivot_root |
| `CLONE_NEWPID` | Container processes can't see host PIDs; container gets its own PID 1 |
| `CLONE_NEWNET` | Separate network namespace (Phase 6); host-net mode skips this |
| `CLONE_NEWIPC` | Separate System V IPC, prevents IPC escapes |
| `CLONE_NEWUTS` | Separate hostname — `hostname` in container doesn't change host |

> **Note:** All flags must be passed to `unshare()` in a single call. Separate calls fail with
> `EPERM` after the user namespace is created.

### Landlock (add in Phase 1.5 — before Phase 2)

`landlock` crate (`landlock = "0.4"`). Apply in child after namespace setup, before `execvp`.

```rust
// Allow container to access only its rootfs
let ruleset = Ruleset::create(AccessFs::from_all(ABI::V3))?
    .add_rules(path_beneath_rules(&[rootfs_path], AccessFs::from_all(ABI::V3))?)
    .create()?
    .restrict_self()?;
```

- Kernel 5.13+: filesystem restrictions
- Kernel 6.4+: network port restrictions (bind/connect)
- Kernel 6.7+: abstract Unix socket + signal restrictions

Landlock is a last-resort backstop: even if a container escapes its namespace via a kernel bug,
Landlock blocks host filesystem access.

### seccomp

Use `syscallz` crate (`syscallz = "0.17"`) with Docker's default allowlist as the starting point.
Deny dangerous syscalls: `ptrace`, `kexec_load`, `create_module`, `mount` (outside ns), `reboot`,
`syslog`, `acct`, `pivot_root` (container shouldn't re-pivot), `settimeofday`, etc.

Apply in child after Landlock, before `execvp`.

### Capability drop

After `execvp`, keep only what is needed:
- `CAP_NET_BIND_SERVICE` if the pod spec declares ports < 1024
- `CAP_CHOWN`, `CAP_SETUID`, `CAP_SETGID` if the container needs `su`/`sudo` internally
- Everything else: dropped

`securityContext.privileged: true` in pod spec bypasses capability drop — explicit opt-in only.

### Pods that need root to install apps

This already works via the UID map: `0 <host_uid> 1`. The container sees itself as root; the host
sees host_uid. `apt install` works. The only limit is syscalls that need real kernel capabilities
(e.g., `CAP_SYS_ADMIN` for mounting inside the container) — allow via seccomp allowlist on a
per-pod basis when `securityContext.capabilities.add` is set in the spec.

### Sub-UID ranges (install `uidmap` package)

For containers that need more than one UID (e.g., multi-user images with `su`), read
`/etc/subuid` and `/etc/subgid` at startup and use `newuidmap`/`newgidmap` for multi-range
mapping. The `uidmap` package provides these setuid helpers.

---

## Image Layer Storage — Overlayfs

Currently z8s extracts images to a flat per-container rootfs directory. This wastes disk when
multiple containers share the same base image.

### Target layout

```
~/.local/share/z8s/           (or /var/lib/z8s/ as root)
├── images/
│   └── <digest>/             ← extracted layers (immutable lower dirs)
│       ├── layer-0/
│       ├── layer-1/
│       └── layer-2/
├── overlay/
│   └── <container-id>/
│       ├── upper/            ← container-writable layer
│       ├── work/             ← overlayfs workdir
│       └── merged/           ← final rootfs (mount point)
├── rootfs/                   ← (legacy, keep for native processes)
├── containers/
├── volumes/
├── secrets/
├── configmaps/
└── emptydir/
```

### Implementation

- **Primary:** native overlayfs — supported in user namespaces since kernel 5.11.
  `mount -t overlay overlay -o lowerdir=layer-2:layer-1:layer-0,upperdir=upper,workdir=work merged`
- **Fallback:** `fuse-overlayfs` for kernels < 5.11. Shell out to the binary; it's available in
  most distro packages.
- Detect at startup: try native overlayfs mount; if `EPERM`, fall back to fuse-overlayfs.

Layer sharing: multiple containers using `nginx:latest` share the same `images/<digest>/` lower
dirs. Only their `upper/` layers differ. Disk usage drops dramatically.

---

## Phase 2 — Storage (Volumes, ConfigMaps, Secrets)

**Goal:** Support `hostPath`, `emptyDir`, `configMap`, `secret` volume types. PV/PVC for single-node.

### Volume mount implementation

All bind-mounts happen **before `pivot_root`** in `child_enter_ns_fork`. The parent prepares the
volume directories; the child mounts them.

```
spawn_container()
  └── prepare_volumes()      ← parent: create dirs, write configmap/secret files
  └── child_enter_ns_fork()
        ├── unshare(namespaces)
        ├── bind_mount_volumes()  ← NEW: bind volumes into rootfs before pivot
        └── pivot_root()
```

| Volume type | Implementation |
|---|---|
| `hostPath` | `mount --bind /host/path /rootfs/mount/path` |
| `emptyDir` | Create `emptydir/<pod-uid>-<name>/` on host, bind into container; delete on pod stop |
| `emptyDir (medium: Memory)` | Mount tmpfs on `emptydir/<pod-uid>-<name>/`, bind into container |
| `configMap` | Materialize data keys as files in `configmaps/<ns>/<name>/`, bind read-only |
| `secret` | Materialize base64-decoded data in `secrets/<ns>/<name>/`, bind read-only, `0400` perms |
| `persistentVolumeClaim` | PV = named `hostPath`; PVC binds to PV by size/access mode; bind-mount PV path |

### ConfigMap/Secret env injection

Already partially supported via `container.env`. Extend to support `envFrom.configMapRef` and
`envFrom.secretRef` by reading from the ResourceStore before spawning.

### ResourceStore extensions

Add stores for: `ConfigMap`, `Secret`, `PersistentVolume`, `PersistentVolumeClaim`. These are
simple in-memory maps like the existing pod/deployment stores. Persist to disk under
`configmaps/`/`secrets/` so they survive z8s restart.

### Test cases

```bash
# hostPath
kubectl exec hostpath-pod -- sh -c 'echo hello > /data/test.txt'
cat /tmp/z8s-test/test.txt   # → hello

# ConfigMap as volume
kubectl exec cm-pod -- cat /etc/config/key1   # → value1

# Secret as volume
kubectl exec secret-pod -- cat /etc/secrets/password   # → supersecret

# emptyDir shared between containers
kubectl exec emptydir-pod -c reader -- cat /shared/file   # → shared-data
```

---

## Phase 3 — Networking (Services + Host Network)

**Goal:** Services with virtual IPs and port forwarding; in-cluster DNS.

### Strategy: host network + TCP/UDP proxy

Containers share the host network namespace by default (`hostNetwork: true` implicitly). A
tokio-based proxy inside z8s binds on host ports and forwards to container ports.

```
Client → host:80 → z8s TCP proxy → container:8080
```

### Service types

| Type | Implementation |
|---|---|
| `ClusterIP` | Virtual IP from `10.96.0.0/12` range; tokio proxy routes to container |
| `NodePort` | Bind on host port `30000–32767`; proxy to container port |
| `LoadBalancer` | Same as NodePort for single-node (no external LB) |
| `ExternalName` | DNS CNAME record; handled by embedded DNS |

### In-cluster DNS — `hickory-dns`

Embed `hickory-server` (the server half of hickory-dns) as a tokio task. It listens on
`127.0.0.53:53` (or a container-private address). Each container's `resolv.conf` is written with
`nameserver <z8s-dns-ip>`.

```toml
hickory-server = "0.24"
hickory-resolver = "0.24"
```

The DNS handler queries the `ResourceStore` to resolve:
- `<service>.<namespace>.svc.cluster.local` → ClusterIP
- `<pod-ip>` → reverse lookup

This replaces the current `/etc/resolv.conf` injection with public DNS (`1.1.1.1`). Pods resolve
service names correctly; external DNS falls through to upstream.

### Why not /etc/hosts injection

- Doesn't update dynamically when services are created/deleted
- Per-container hosts file rewrite is fragile
- hickory-dns handles it cleanly with a live view of the ResourceStore

---

## Phase 4 — cgroups v2 Resource Limits (Non-Root)

**Goal:** Enforce `memory.max`, `cpu.max`, `pids.max` on containers running as non-root.

### How delegation works

systemd creates a cgroup subtree for each user session:
```
/sys/fs/cgroup/user.slice/user-<uid>.slice/session-<id>.scope/
```

Enable controller delegation by writing
`/etc/systemd/system/user@.service.d/delegate.conf`:
```ini
[Service]
Delegate=memory cpu pids
```

z8s creates sub-cgroups under its session scope:
```
.../z8s/<pod-uid>/
```

Detect delegated path at startup by reading `/proc/self/cgroup` and checking write access.
Fall back to no-limits mode with a warning when not available.

### Kubernetes resource → cgroup mapping

| Spec field | cgroup file | Example |
|---|---|---|
| `limits.memory` | `memory.max` | `128Mi` → `134217728` |
| `requests.memory` | `memory.low` | `64Mi` → `67108864` |
| `limits.cpu` | `cpu.max` | `500m` → `50000 100000` |
| `limits.storage` | dir size quota | `project quota` on ext4/xfs or du-based enforcer |
| (always) | `pids.max` | Default `1024` unless overridden |

### When running as root (PID 1)

Use `/sys/fs/cgroup/z8s/<pod-uid>/` directly. No delegation needed.

---

## Phase 5 — Ingress Controller

**Goal:** Route external traffic to Services. Supports HTTP, HTTPS, WebSocket, and raw TCP/UDP
(custom ports). Works without nginx or any external binary.

### Why custom ports and WebSockets matter

Kubernetes Ingress is not just HTTP — real workloads need:
- **WebSocket** connections (long-lived HTTP/1.1 upgrades or HTTP/2 streams)
- **Custom TCP ports** (databases, game servers, MQTT, gRPC without HTTP routing)
- **UDP ports** (DNS, QUIC, game protocols)
- **TLS passthrough** (don't terminate, forward raw TLS to backend)

### Architecture

z8s runs an internal ingress controller with three listeners:

```
Port 80   → HTTP router (Host header + path matching)
Port 443  → TLS termination → HTTP router (same rules)
Port N    → Raw TCP/UDP proxy (for custom port rules)
```

All implemented as tokio tasks inside the z8s process, no external binary.

### Ingress resource model

Standard k8s `networking.k8s.io/v1` Ingress plus a z8s-specific annotation for custom protocols:

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: my-app
  annotations:
    z8s.io/protocol: "tcp"          # tcp | udp | ws | grpc | https-passthrough
    z8s.io/listen-port: "5432"      # for raw TCP/UDP rules
spec:
  tls:
  - hosts: [myapp.example.com]
    secretName: my-tls
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

### Protocol handling

| Protocol | How |
|---|---|
| HTTP | Parse `Host` + path, proxy via `hyper` to backend |
| HTTPS | TLS termination with `rustls`; forward decrypted HTTP |
| WebSocket | Detect `Upgrade: websocket` header; proxy as raw TCP tunnel after handshake |
| gRPC | HTTP/2 framing; route by `:authority` pseudo-header |
| TCP (custom port) | `TcpListener` on the declared port; forward bytes to backend service IP:port |
| UDP | `UdpSocket` on the declared port; stateful session table → backend |
| TLS passthrough | SNI-only routing; forward raw TLS bytes without decryption |

### TLS

Use `rustls` (pure Rust, no openssl). Certificates loaded from k8s `Secret` resources
(`tls.crt`, `tls.key` fields). ACME (Let's Encrypt) via the `instant-acme` crate for auto-renewal.

### Implementation approach

Rather than pulling in Pingora (a large framework) or rpxy (an external binary), build the
ingress routing table as a `DashMap<IngressKey, BackendAddr>` that the `ResourceStore` update
channel keeps warm. The proxy loop is ~200 lines of tokio code.

Reference: [Building a custom Kubernetes Ingress in Rust that outperforms NGINX by 11%](https://medium.com/@abdullrahmaneissa6/how-i-built-a-custom-kubernetes-ingress-in-rust-go-that-outperforms-nginx-by-11-ceb5e4b6b0a9)

---

## Phase 6 — Network Isolation (per-container netns + pasta)

**Goal:** Optional per-container network namespace with full outbound connectivity and port
forwarding. Host-network mode remains the default (best performance).

### pasta (preferred over slirp4netns)

pasta is the default rootless networking backend in Podman 5+ and RHEL 9.5. Key advantages over
slirp4netns:
- **No NAT** — pasta copies the host's network configuration into the container, containers see
  real IPs
- **Lower latency** — no userspace TCP/IP stack translation
- **Better performance** — outperforms slirp4netns up to ~8 parallel connections

```
parent                          child (CLONE_NEWNET)
  fork() ──────────────────────► loopback only
  write UID maps
  launch: pasta --pid <child>
  pasta sets up veth/tap ──────► eth0 with IP + routes
  write ack byte
                                 exec(entrypoint)
```

### Port forwarding

pasta supports port forwarding natively: `pasta -t <host-port>:<container-port>` for TCP,
`-u` for UDP. No separate proxy needed for simple cases. For complex cases (dynamic ports,
service load balancing), the Phase 3 tokio proxy handles it at the service level.

### Network mode in pod spec

```yaml
spec:
  hostNetwork: true          # default: share host netns (fastest)
  # OR
  hostNetwork: false         # isolated netns via pasta
  dnsPolicy: ClusterFirst    # use z8s embedded DNS
```

---

## Phase 7 — eBPF CNI + Network Policies

**Goal:** Pod-to-pod networking via veth pairs, network policies enforced via eBPF TC programs.

### CNI approach

```
pod-A (netns-A)              pod-B (netns-B)
  eth0 ─── veth-A ──┐  ┌── veth-B ─── eth0
                    bridge (z8s-cni0)
                    │
                  host network
```

1. On pod start: create a veth pair, one end in pod netns, one in host
2. Attach both to a bridge (`z8s-cni0`) on the host
3. Assign pod IP from the pod CIDR (`10.244.0.0/16`)
4. Write routes: pod-A can reach pod-B via bridge IP

### eBPF with `aya`

```toml
aya = "0.13"
aya-ebpf = "0.1"        # for the eBPF programs themselves
```

The `aya` crate is pure Rust — **no libbpf dependency**, no C toolchain for the loader. eBPF
programs are written in Rust with `aya-ebpf` and compiled to BPF bytecode at build time.

TC (traffic control) classifier programs attach to the ingress/egress of each veth:

```
pod-A → veth-A → [TC egress BPF] → bridge → [TC ingress BPF] → veth-B → pod-B
```

The BPF program implements network policy: for each packet, check the source/destination pod
labels against the `NetworkPolicy` resources in the store. Drop if denied, pass if allowed.

This is the same approach used by Cilium, but simpler (single-node, no cross-node routing).

### NetworkPolicy resource (standard k8s)

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: deny-all
  namespace: default
spec:
  podSelector: {}
  policyTypes: [Ingress, Egress]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: allow-frontend
spec:
  podSelector:
    matchLabels:
      app: backend
  ingress:
  - from:
    - podSelector:
        matchLabels:
          app: frontend
    ports:
    - port: 8080
```

### eBPF kernel requirements

| Feature | Min kernel |
|---|---|
| TC classifiers (basic) | 4.1 |
| BTF (compile-once run-everywhere) | 5.2 |
| aya full feature support | 5.8 |

Detect at startup. If kernel < 5.8, fall back to nftables inside each pod's network namespace
(requires `CLONE_NEWNET` which gives `CAP_NET_ADMIN` inside the ns).

---

## Phase 8 — Multi-Node Support

**Goal:** Multiple nodes on the same host or remote machines. Same kubectl interface.

### Architecture: agent model

```
Node 0 (primary)              Node 1 (agent)
┌─────────────────────┐       ┌──────────────────┐
│  z8s --primary      │       │  z8s --agent      │
│  API server :6443   │◄─────►│  gRPC :7443       │
│  scheduler          │       │  supervisor       │
│  controller         │       │  cgroups          │
│  DNS                │       │  metrics          │
│  ingress            │       │                  │
└─────────────────────┘       └──────────────────┘
```

The agent is z8s running with a subset of components:
- `ProcessSupervisor` — runs pods assigned to it
- `CgroupManager` — resource limits
- `HealthChecker` — probes
- gRPC server — receives pod assignments, reports state + metrics

The primary schedules pods by writing `nodeName` to the pod spec. The agent watches its own
assignment queue via a long-poll or gRPC stream.

### Same-host nodes (namespace isolation)

For nodes on the same machine (useful for testing multi-node locally):
- Each agent gets a separate port and data directory
- Communicate over Unix sockets instead of TCP

### Transport

Simple gRPC with mTLS (generated from a cluster CA at startup, like k3s does). The `tonic` crate
provides async gRPC on tokio.

---

## Key Crate Recommendations

| Crate | Use | Add in |
|---|---|---|
| `landlock` | Filesystem + network isolation per container | Phase 1.5 |
| `syscallz` | seccomp-bpf syscall filter | Phase 1.5 |
| `hickory-server` + `hickory-resolver` | Embedded cluster DNS | Phase 3 |
| `aya` + `aya-ebpf` | eBPF CNI + network policies | Phase 7 |
| `instant-acme` | ACME/Let's Encrypt for ingress TLS | Phase 5 |
| `tonic` | gRPC for multi-node agent protocol | Phase 8 |
| `dashmap` | Lock-free concurrent routing table for ingress | Phase 5 |

### Crates to study (not add as deps)

| Crate / Project | Why to study |
|---|---|
| `hakoniwa` | Complete reference for the full isolation stack (namespaces + Landlock + seccomp + pasta + cgroups) |
| `youki/libcontainer` | OCI runtime edge cases, cgroup delegation logic |
| `rpxy` / `sozu` | Ingress proxy architecture patterns |
| Pingora | Large-scale proxy design patterns |

---

## Packages to Install on the Host

| Package | Provides | Needed for |
|---|---|---|
| `uidmap` | `newuidmap`, `newgidmap` | Sub-UID range mapping (multi-user containers) |
| `fuse-overlayfs` | `fuse-overlayfs` binary | Overlay image layers on kernels < 5.11 |
| `passt` | `pasta` binary | Per-container network namespace |
| `iptables` / `nftables` | Firewall | Network policies fallback (pre-eBPF) |
| `iproute2` | `ip` command | veth pair setup for CNI |

---

## Refactoring Notes

### Builder pattern (narrow scope)

Keep `src/builder/` for internal resource construction and tests. Do not wrap `k8s-openapi` types
in builder wrappers — they are already good typed structs.

Fix the `#[allow(clippy::too_many_arguments)]` on `spawn_userns_container` by consolidating
parameters into a `ContainerSpawnCtx` struct:

```rust
struct ContainerSpawnCtx<'a> {
    entrypoint: &'a str,
    args: &'a [String],
    env: &'a [(String, String)],
    rootfs: &'a str,
    container_id: &'a str,
    pod_uid: &'a str,
    image: &'a str,
    spec: &'a k8s_openapi::api::core::v1::Container,
    volumes: &'a [ResolvedVolume],   // Phase 2
}
```

### Known bug — manifest watcher

`watcher.rs:123` skips file changes for files already loaded at startup, which breaks live
reloading. The `processed` HashSet should only deduplicate within the startup scan pass, not
filter subsequent `Modify` events.

### Namespace flags — add NEWPID and NEWIPC now

`child_enter_ns_fork` currently uses `NEWUSER | NEWNS | NEWUTS`. Add:

```rust
let flags = CloneFlags::CLONE_NEWUSER
    | CloneFlags::CLONE_NEWNS
    | CloneFlags::CLONE_NEWPID    // ← add
    | CloneFlags::CLONE_NEWIPC    // ← add
    | CloneFlags::CLONE_NEWUTS;
```

`CLONE_NEWPID` means the container's init process is PID 1 in its own namespace. Host can't be
seen from `ps` inside the container. `CLONE_NEWIPC` prevents shared memory and semaphore escapes.

---

## Phase Summary

| Phase | Goal | Key additions |
|---|---|---|
| **1.5** | Strengthen isolation | `NEWPID` + `NEWIPC` + Landlock + seccomp |
| **2** | Storage | Volumes, ConfigMaps, Secrets, PV/PVC, overlayfs |
| **3** | Networking | Service proxy, ClusterIP, NodePort, hickory-dns |
| **4** | Resource limits | cgroup v2 delegation for non-root |
| **5** | Ingress | HTTP/S + WebSocket + TCP/UDP custom ports, TLS, ACME |
| **6** | Network isolation | `CLONE_NEWNET` + pasta per container |
| **7** | eBPF CNI | veth+bridge, NetworkPolicy via aya TC programs |
| **8** | Multi-node | Primary + agent model, gRPC, tonic |
