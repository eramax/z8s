# Platform Overview

## Pelagos — v0.65.31

**Linux Container Runtime** — OCI-compatible, namespaces, cgroups v2, seccomp, networking, image management.

```
/home/abb/dev/pelagos/
```

### Features

| Category | Details |
|---|---|
| **Core isolation** | 6/7 Linux namespaces (UTS, Mount, IPC, User, Net, Cgroup), chroot + pivot_root, automatic /proc/sys/dev mounts |
| **Security** | Seccomp-BPF (Docker default + minimal profile), no-new-privs, read-only rootfs, masked paths, capability management, rlimits |
| **Interactive** | PTY via `openpty()`, `setsid()` + `TIOCSCTTY`, raw-mode relay, SIGWINCH forwarding, RAII terminal restore |
| **Resource mgmt** | cgroups v2 (memory, CPU shares, CPU quota, PIDs), coexists with rlimits, resource stats, automatic cleanup |
| **Filesystem** | Bind mounts (RW/RO), tmpfs, named volumes (`/var/lib/pelagos/volumes/`), overlayfs (CoW layered rootfs) |
| **OCI images** | Pull from any registry (`oci-client`), anonymous auth, content-addressable layer cache at `/var/lib/pelagos/layers/`, multi-layer overlay, whiteout support |
| **Networking** | N1 Loopback, N2 Bridge (veth + `pelagos0`, 172.19.0.x/24), N3 NAT (nftables MASQUERADE), N4 Port mapping (nftables DNAT + TCP proxy), N5 DNS (bind-mount resolv.conf), N6 Pasta (rootless user-mode net), N7 DNS service discovery (dual-backend: builtin `pelagos-dns` + dnsmasq), multi-network containers (eth1, eth2, ...) |
| **Image build** | `pelagos build -t <tag>` — Remfile parser (FROM, RUN, COPY, ADD, CMD, ENTRYPOINT, ENV, WORKDIR, EXPOSE, LABEL, USER, ARG), multi-stage builds, ARG substitution, ADD URL download + archive extraction, `.remignore`, build cache |
| **Container exec** | `pelagos exec <name> <command>` — namespace discovery, environment inheritance, PTY mode, user/workdir options |
| **Compose** | `pelagos compose up/down/ps/logs` — S-expression format, scoped networks/volumes, topo-sort dependency ordering, TCP readiness polling, supervisor |
| **OCI compliance** | `pelagos create/start/state/kill/delete` — config.json parsing, double-fork shim, exec.sock sync, state.json persistence |
| **CRI** | `pelagos-cri` — k3s/kubelet CRI runtime integration, pod sandbox with loopback netns, cgroup memory/CPU stats |
| **Rootless-first** | Auto rootless with fallback chain: kernel overlayfs+userxattr > fuse-overlayfs > error |

### CLI Commands

- `pelagos run` — build + launch containers
- `pelagos build` — build images from Remfiles
- `pelagos exec` — run command in running container
- `pelagos ps/stop/rm/logs` — container lifecycle
- `pelagos image pull/ls/rm/save/load/tag/login/logout` — image management
- `pelagos network create/ls/rm/inspect` — network management
- `pelagos volume create/ls/rm` — volume management
- `pelagos rootfs import/ls/rm` — rootfs management
- `pelagos compose up/down/ps/logs` — compose orchestration
- `pelagos create/start/state/kill/delete` — OCI lifecycle

---

## smolvm — v1.0.3

**OCI-native microVM runtime** — Ship and run software with hardware-level isolation. Cross-platform (macOS ARM/Intel, Linux x86_64/arm64).

```
/home/abb/dev/smolvm/
```

### Features

| Category | Details |
|---|---|
| **Hypervisor** | libkrun VMM — Hypervisor.framework (macOS) / KVM (Linux); custom kernel via libkrunfw |
| **Boot speed** | Sub-second cold start (<200ms), sub-100ms with packs |
| **Memory** | Elastic via virtio balloon — host commits only what guest uses; default 4 vCPU / 8 GiB |
| **GPU** | virtio-gpu / Venus (Vulkan-over-virtio), virglrenderer, MoltenVK bundled on macOS |
| **Rosetta 2** | x86_64 emulation on Apple Silicon guests |
| **OCI images** | Pull from any registry (Docker Hub, ghcr.io), same open format as Docker, no daemon required |
| **Networking** | Dual-backend: TSI (macOS) / VirtioNet (Linux), egress filtering (CIDR allowlist + DNS allowlist), port forwarding, DNS filter listener |
| **Packing** | `smolvm pack create/run` — single-file `.smolmachine` artifacts, self-contained, zero-dependency rehydration |
| **HTTP API** | Axum REST server, OpenAPI/Swagger docs, JSON-over-vsock protocol for agent communication |
| **Embedded SDK** | TypeScript SDK (`sdks/node/`), embedded runtime control over vsock |
| **Smolfile** | TOML-based machine-as-code (`image`, `net`, `volumes`, `init`, `gpu`, `ssh_agent`) |
| **SSH agent** | Forward host SSH agent into VM — private keys never enter guest |
| **Secrets** | Reference-only secrets (zeroized after use), never persisted |
| **Storage** | Sparse ext4 disk images, volume mounts, block device PVs |
| **Agent** | In-VM agent over vsock: boot config, state probing, terminal mux, log streaming |
| **Cross-platform** | macOS Apple Silicon + Intel, Linux x86_64 + aarch64 |
| **Workspace** | 7 crates: `smolvm`, `smolvm-agent`, `smolvm-network`, `smolvm-pack`, `smolvm-protocol`, `smolvm-registry`, `smolvm-smolfile` |

### CLI Commands

- `smolvm machine run/create/start/stop/exec/shell/delete/status/ls/logs`
- `smolvm serve start` — HTTP API daemon
- `smolvm pack create/run` — portable artifact packaging
- `smolvm config set/get/registries` — configuration
- `smolvm smolfile validate` — validate Smolfile

---

## z8s (v1) — "zetes" · Old Architecture

**Minimal Kubernetes-compatible orchestrator + PID 1 init** — single-binary, single-node, no etcd, no kubelet.

```
/home/abb/dev/z8s/
```

### Current Architecture (`src/`)

| File | Purpose |
|---|---|
| `main.rs` | Entry point, daemon management, CLI dispatch |
| `init.rs` | PID 1 init — SIGCHLD zombie reaping, signal forwarding |
| `controller.rs` | DeploymentController — reconcile loop for replica counts |
| `api/mod.rs` | AnyResource enum (Pod, Service, ConfigMap, etc.) |
| `api/types.rs` | ResourceStore, ResourceTracker, ResourceState, YAML parsing |
| `server/api.rs` | Axum HTTP handlers for all k8s API endpoints |
| `server/exec.rs` | kubectl exec — WebSocket PTY + pipe execution |
| `server/proto.rs` | k8s protobuf body decoder |
| `supervisor/process.rs` | ProcessSupervisor: spawn/stop/reconcile containers |
| `supervisor/cgroup.rs` | cgroups v2 resource limits |
| `supervisor/health.rs` | Health probes: exec, httpGet, tcpSocket |
| `container/image.rs` | OCI image pull, layer extraction, rootfs cache |
| `container/rootfs.rs` | chroot/namespace helpers, UTS hostname isolation |
| `container/volumes.rs` | Volume mount resolution (hostPath, emptyDir, configMap, secret) |
| `container/oci_config.rs` | OCI config parsing |
| `container/port_publish.rs` | 127.0.0.1 port forwarding |
| `network/mod.rs` | NetworkManager: service proxy lifecycle, endpoints |
| `network/dns.rs` | In-cluster DNS server |
| `network/service_proxy.rs` | TCP proxy with round-robin load balancing |
| `network/port_publish.rs` | Unique host port allocation |
| `manifest/watcher.rs` | inotify-based manifest watcher |
| `builder/` | Rust builder API for k8s resources |

### Features (v1)

| Feature | Details |
|---|---|
| **kubectl-compatible API** | Full REST API on port 6443 — works with unmodified `kubectl` |
| **Pod lifecycle** | Create, list, describe, delete, watch |
| **Deployments** | Create, scale (`kubectl scale`), describe, delete |
| **Exec** | `kubectl exec` with PTY (`-it`) and pipe (`-i`) via WebSocket |
| **Logs** | `kubectl logs` from in-memory ring buffer (last 1000 lines) |
| **Namespaces** | Full CRUD with resource isolation |
| **Nodes** | Reports host as single node with real CPU/memory |
| **Events** | Pod lifecycle events |
| **Metrics** | `kubectl top pod/node` via cgroup v2 stats |
| **Services** | ClusterIP + NodePort — TCP proxy with round-robin load balancing |
| **Endpoints** | Endpoints + EndpointSlices (discovery.k8s.io/v1) |
| **ConfigMaps & Secrets** | Full CRUD, envFrom injection, volume mounts |
| **PersistentVolumes / PVCs** | Static PV provisioning, claim binding |
| **DNS** | In-cluster DNS on port 53, service resolution, resolv.conf injection |
| **OCI images** | Pull from any registry; layer cache at `/var/lib/z8s` |
| **Native processes** | `image: ""` runs host binary directly |
| **chroot containers** | OCI layers extracted + chroot execution |
| **Network isolation** | Per-pod network namespaces with port publishing |
| **Hostname isolation** | Per-pod UTS namespace with pod name as hostname |
| **Manifest watcher** | Auto-apply YAML from `/etc/z8s/manifests/` |
| **Health probes** | liveness, readiness, startup (exec, httpGet, tcpSocket) |
| **cgroups v2** | Memory/CPU limits (root only) |
| **PID 1 init** | Zombie reaping, signal forwarding |
| **Protobuf bodies** | Accepts protobuf-encoded `kubectl create` |
| **Watch support** | `kubectl get --watch` for pods, services, deployments |

### API Groups

- **Core v1**: pods, services, endpoints, configmaps, secrets, namespaces, nodes, events, persistentvolumes, persistentvolumeclaims
- **Apps v1**: deployments, scale
- **Discovery v1**: endpointslices
- **Metrics v1beta1**: node/pod metrics
- **Authorization v1**: selfsubjectaccessreviews, subjectaccessreviews
- **OpenAPI v2/v3**: schema stubs
- **Health**: `/healthz`, `/readyz`, `/livez`

### Service Types

- **ClusterIP** — default, binds on loopback, TCP proxy to pods
- **NodePort** — host listener on `0.0.0.0:<nodePort>`, round-robin to pods

### Limitations (v1)

- Single node only — no clustering
- In-memory state (lost on restart)
- No RBAC/auth
- chroot requires root
- No pod-to-pod networking (host-level TCP proxy)
- No LoadBalancer, StatefulSet, DaemonSet, Job, CronJob
- Userspace TCP proxy adds latency (~200μs per connection)
- 19,451 LOC — god file `types.rs` (2,545 lines)
- Three redundant container spawn paths (~70% code duplication)

---

## z8s (v2) — "zetes" · Refactored Architecture

**Production-grade refactor** — target: ≤5,000 LOC, multi-node, true pod networking, OverlayFS, WAL-backed state.

```
/home/abb/dev/z8s/src2/
```

### Target Architecture (`src2/`)

```
src/
├── main.rs                  CLI dispatch (~80 lines)
├── node.rs                  Node lifecycle + DI (~100 lines)
├── config.rs                CLI parsing + global config (~120 lines)
├── init.rs                  PID 1 signal handling (~50 lines)
│
├── resource/                Generic resource framework
│   ├── mod.rs               Resource<S,T> wrapper, AnyResource
│   ├── meta.rs              ObjectMeta, ListMeta, Time, Quantity
│   ├── registry.rs          Type registry: kind → serializer
│   └── store.rs             StoreBackend trait + Memory + Redb
│
├── api/                     HTTP layer
│   ├── server.rs            Axum router (~60 lines)
│   ├── crud.rs              Generic CRUD handler (6 ops from 1 trait)
│   ├── watch.rs             SSE-based watch stream
│   ├── exec.rs              kubectl exec WebSocket
│   ├── proto.rs             Protobuf decoder
│   └── discovery.rs         /api, /apis, /version, /healthz
│
├── compute/                 Container runtime
│   ├── spawner.rs           Unified spawn pipeline
│   ├── rootfs.rs            Filesystem isolation
│   ├── image.rs             OCI pull + OverlayFS mount
│   ├── cgroup.rs            cgroups v2
│   ├── health.rs            Probes
│   └── lifecycle.rs         Restart policy
│
├── network/                 Network plane
│   ├── netlink.rs           Raw netlink
│   ├── nft.rs               nftables engine (DNAT, SNAT, NSG)
│   ├── veth.rs              veth pair lifecycle
│   ├── vnet.rs              VNet/Subnet IPAM
│   ├── dns.rs               In-cluster DNS
│   ├── ipv6.rs              IPv6 from host /64
│   └── policy.rs            NetworkPolicy + NSG enforcement
│
├── storage/                 Persistent storage
│   ├── overlay.rs           OverlayFS mount/unmount
│   ├── provision.rs         PV/PVC + loop provisioner
│   └── volumes.rs           Volume mount resolution
│
├── cluster/                 Multi-node coordination
│   ├── gossip.rs            Batched gossip (MessagePack)
│   ├── scheduler.rs         O(1) index-based scheduling
│   ├── leader.rs            Lease-based leader election
│   └── sync.rs              Anti-entropy reconciliation
│
└── reconcile/               Control plane
    ├── mod.rs               Reconciler loop + notify-driven wake
    ├── pipeline.rs          Resource lifecycle pipeline
    ├── deployment.rs        Deployment → Pod reconciliation
    └── service.rs           Service → nftables DNAT reconciliation
```

### Planned Improvements (v1 → v2)

| Area | v1 (Current) | v2 (Target) |
|---|---|---|
| **Code size** | 19,451 LOC | ≤ 5,000 LOC |
| **Types** | `types.rs` god file (2,545 lines) | Generic `Resource<S,T>` (400 lines) |
| **API handlers** | 23 files, 3,200 lines (CRUD boilerplate) | Generic `CrudHandler<R>` (300 lines) |
| **Container spawn** | 3 redundant paths (2,099 lines) | Unified `Spawner` (600 lines) |
| **Networking** | Host TCP proxy + messy nftables (1,600 lines) | Pure nftables DNAT, VNet/Subnet/NSG (700 lines) |
| **TCP proxy** | Userspace copy_bidirectional (~200μs latency) | Kernel nftables DNAT (zero userspace) |
| **Rootfs** | Full copy per container (N×image_size I/O) | OverlayFS (O(1) mount, shared lower) |
| **Scheduling** | O(pods²) table scan | O(nodes) index-based |
| **Gossip** | O(N) sends, individual JSON | Batched MessagePack, O(1) sends |
| **State** | In-memory (lost on restart) | WAL-backed with snapshot recovery |
| **Pod networking** | Host proxy — no pod-to-pod | veth L3 routing, pod IPs |
| **IPv6** | None | Dual-stack from host /64 |
| **RBAC** | None | Namespace-scoped roles + service accounts |
| **PID 1** | Basic signal forwarding | Full init + capability-aware supervision |
| **Deps** | `chrono`, `uuid`, `notify`, `ipnetwork`, `async-trait`, `tokio-tungstenite`, `futures-util` | All removed (6 fewer crates) |
| **I/O** | epoll (tokio default) | io_uring feature flag for image unpack |

### New CRDs (v2)

- **VNet** (`z8s.io/v1`) — overlay network with CIDR
- **Subnet** (`z8s.io/v1`) — subnet within a VNet with NSG link
- **NSG** (Network Security Group) — firewall rules (inbound/outbound, protocol, ports, CIDR)
- **RouteTable** — routing rules
- **LoadBalancer** services — public IP pool + nftables DNAT

### Performance Targets (v2)

| Metric | Target |
|---|---|
| Pod startup (cached image) | < 200ms |
| Multi-node scheduling | < 50ms per pod |
| Gossip convergence (3 nodes) | < 500ms |
| NodePort latency overhead | < 10μs |
| Binary size (static, stripped) | ≤ 8MB |
| Code size | ≤ 5,000 LOC |

### Implementation Phases

1. **Phase 0 — Foundation**: Generic resource framework + CRUD handler (Week 1)
2. **Phase 1 — Type System**: All resources to generic `Resource<S,T>` (Week 2)
3. **Phase 2 — Runtime**: Unified spawner + OverlayFS (Week 3)
4. **Phase 3 — Network**: Pure DNAT, IPv6, NSG, delete TCP proxy (Week 4)
5. **Phase 4 — Cluster**: O(1) scheduler, batched gossip, hardened leader election (Week 5)
6. **Phase 5 — Polish**: io_uring, dependency removal, benchmarks (Week 6)

### Already Implemented (src2/)

- Generic resource CRUD handlers
- VNet/Subnet/NSG/RouteTable types with `z8s.io/v1` API group
- Generic `Resource<S,T>` wrapper
- Reformatted API enrichment layer for networking resources
- nftables table naming: `z8s_nat_{node}`, `z8s_filter_{node}`
- Redb-backed state, WAL-based persistence

---

## Other Repositories

| Repo | Path | Description |
|---|---|---|
| **libnftnl** | `/home/abb/dev/libnftnl/` | Low-level nftables netlink library in Rust |
| **monobash** | `/home/abb/dev/monobash/` | Single-file bash orchestration utilities |

---

## Summary Comparison

| Dimension | Pelagos | smolvm | z8s (v1) | z8s (v2 target) |
|---|---|---|---|---|
| **Type** | Container runtime | MicroVM runtime | K8s-compatible orchestrator | Production orchestrator |
| **Isolation** | Namespace (shared kernel) | Hardware VM (hypervisor) | Namespace (chroot) | Namespace (pivot_root + overlay) |
| **API** | CLI + library | CLI + HTTP API + SDK | kubectl-compatible REST | kubectl-compatible REST |
| **Rootless** | Yes (pasta, fuse-overlay) | N/A (KVM requires root) | Partial (native only) | Partial (overlayfs fallback) |
| **Platform** | Linux only | Linux + macOS | Linux only | Linux only |
| **Startup** | ~100ms | <200ms | ~500ms | <200ms |
| **OCI images** | Yes | Yes | Yes | Yes (overlayfs) |
| **Networking** | N1-N7, multi-attach | TSI/VirtioNet, egress filter | TCP proxy, port publish | DNAT, VNet/NSG, IPv6 |
| **Multi-node** | No | No | Gossip-based | Gossip + leader election |
| **State** | Stateless (ephemeral) | Persistent (disk images) | In-memory (lost on restart) | WAL-backed + snapshot |
| **Code size** | ~5,000 LOC | ~15,000 LOC | 19,451 LOC | ≤ 5,000 LOC |
| **Language** | Rust | Rust | Rust | Rust |
