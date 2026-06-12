# Comprehensive Feature Inventory & Migration Guide

## Sources

| Codebase | Location | Role | Lines |
|----------|----------|------|-------|
| **Pelagos** | `/home/abb/dev/pelagos/src/` | Battle-tested container runtime (v0.65.31) | ~25,000 |
| **z8s-old (src2)** | `/home/abb/dev/z8s/src2/` | Monolithic orchestrator (previous iteration) | ~15,000 (149 files) |
| **z8s-new (src)** | `/home/abb/dev/z8s/src/` | Modular 8-crate orchestrator (in progress) | ~8,000 (49 files, mostly stubs) |
| **Refactoring plan** | `/home/abb/dev/z8s/docs/refactoring-plan.md` | Target architecture for z8s | 2,049 lines |

## Mode Legend

| Mode | Icon | Description | Requirements |
|------|------|-------------|-------------|
| **Root** | 👑 | Requires root / CAP_SYS_ADMIN | Namespace ops, cgroups, nftables |
| **Rootless** | 👤 | Works without root | User namespaces, fuse-overlayfs, pasta |
| **Init (PID 1)** | 🔱 | Runs as PID 1  | Signal handling, zombie reaping, subreaper |

> z8s operates primarily in **Root 👑** mode and **Init 🔱** mode (it is a PID 1 container
> init tool). Rootless mode was attempted but not successfully implemented — features
> marked 👤 are aspirational.

---

# PART 1: Feature Comparison by Domain

Each feature includes:
- ✅**Pelagos status** — is it implemented in Pelagos?
- ✅**z8s-old (src2) status** — was it implemented in the old codebase?
- ✅**z8s-new (src) status** — is it implemented in the new modular codebase?
- **Description** — what the feature does (for non-obvious features)
- **Benefits** — why we want it
- **Effort** — estimated implementation effort
- **Mode** — 👑 Root / 👤 Rootless / 🔱 Init

---

## 1. CONTAINER RUNTIME (z8s: `runtime/crate`, Pelagos: `container.rs`, src2: `cri/`)

This is the **highest priority** category. z8s-new's `runtime` crate has a
`ContainerSupervisor` that wraps Pelagos-like functionality but is missing 60+
features that both Pelagos and src2 have.

### 1.1 Namespace Isolation

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.1.1 | PID namespace | ✅ | ✅ | ✅ | 👑 | Done | Isolates process tree. Required for init mode. |
| 1.1.2 | Mount namespace | ✅ | ✅ | ✅ | 👑 | Done | Isolates filesystem mounts. Required for chroot. |
| 1.1.3 | UTS namespace | ✅ | ✅ | ✅ | 👑 | 1d | Isolates hostname + domain. Benefits: per-pod hostnames, DNS integration. |
| 1.1.4 | IPC namespace | ✅ | ✅ | ✅ | 👑 | 1d | Isolates System V IPC + POSIX message queues. Benefits: prevents container escape via /dev/shm, required for pod sandbox. |
| 1.1.5 | NET namespace | ✅ | ✅ | ✅ | 👑 | Done | Isolates network stack. Each pod gets own IP. |
| 1.1.6 | USER namespace | ✅ | ✅ | Partial | 👤 | 3d | Maps container uid 0 to unprivileged host uid. Benefits: rootless containers, reduced attack surface. z8s attempted but failed. |
| 1.1.7 | CGROUP namespace | ✅ | ❌ | ❌ | 👑 | 1d | Isolates cgroup hierarchy view. Benefits: prevents container from seeing host cgroups, required for CRI compliance. |
| 1.1.8 | Namespace joining (setns) | ✅ | ✅ | Partial | 👑 | 1d | Join existing namespaces instead of creating new ones. Benefits: container exec, pod sandbox, network namespace joining, debugging. |
| 1.1.9 | Rootless auto user-ns | ✅ | ❌ | ❌ | 👤 | 5d | Auto-create USER namespace when non-root. Benefits: rootless containers without `sudo`. Pelagos probes kernel 5.11+ support. z8s failed here before. |

### 1.2 Filesystem Isolation

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.2.1 | chroot | ✅ | ✅ | ✅ | 👑 | Done | Classic chroot(2) for rootfs. |
| 1.2.2 | pivot_root | ✅ | ✅ | Done | 👑 | Done | Modern pivot_root(2) with put_old MNT_DETACH. Benefits: properly detaches host filesystem, stronger isolation, required for OCI compliance. |
| 1.2.3 | Read-only rootfs | ✅ | ❌ | ❌ | 👑 | 1d | Remount rootfs MS_RDONLY after setup. Benefits: containers can't modify their own filesystem, defense-in-depth for immutable infrastructure. |
| 1.2.4 | Masked paths | ✅ | ❌ | ❌ | 👑 | 1d | Mask sensitive /proc files with /dev/null. Benefits: hides host kernel info (/proc/kcore, /sys/firmware) from container. |
| 1.2.5 | Readonly paths | ✅ | ❌ | ❌ | 👑 | 1d | Remount specific paths read-only (not entire rootfs). Benefits: /sys readonly inside container while rootfs stays writable. |
| 1.2.6 | /proc auto-mount | ✅ | ✅ | ✅ | 👑 | Done | Mount procfs after chroot. Required for ps, top, etc. |
| 1.2.7 | /sys auto-mount | ✅ | ✅ | ✅ | 👑 | 1d | Mount sysfs after chroot. Benefits: some apps need /sys for device detection. |
| 1.2.8 | /dev auto-mount | ✅ | ✅ | ✅ | 👑 | Done | Mount devtmpfs or bind-mount /dev. Required for device access. |
| 1.2.9 | Mount propagation control | ✅ | ❌ | ❌ | 👑 | 1d | MS_PRIVATE/MS_SLAVE/MS_SHARED flags on rootfs mount. Benefits: prevents mount events leaking between namespaces, required for OCI compliance. |
| 1.2.10 | Kernel mount (proc/sysfs/devpts) | ✅ | ✅ | ❌ | 👑 | 1d | Mount arbitrary kernel filesystems. Benefits: devpts for PTY allocation, mqueue for POSIX message queues. |

### 1.3 Overlay Filesystem

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.3.1 | Single overlay (upper+work) | ✅ | ❌ | ❌ | 👑 | 2d | Basic overlayfs: upper dir for writes, work dir for metadata. Benefits: copy-on-write rootfs, base image stays clean. |
| 1.3.2 | Multi-layer overlay (image layers) | ✅ | ❌ | ❌ | 👑 | 3d | Stack multiple lower dirs for OCI image layers. Benefits: OCI image support (each layer is a lower dir), dedup between images. |
| 1.3.3 | Rootless overlay (kernel userxattr) | ✅ | ❌ | ❌ | 👤 | 5d | Native overlayfs in user namespace with userxattr (kernel ≥ 5.11). Benefits: rootless CoW without FUSE overhead. |
| 1.3.4 | Rootless overlay (fuse-overlayfs) | ✅ | ❌ | ❌ | 👤 | 2d | Fallback when kernel doesn't support userxattr. Benefits: rootless CoW on any kernel. |
| 1.3.5 | Overlay auto-cleanup on wait() | ✅ | ❌ | ❌ | 👑 | 1d | Remove merged dir after container exits. Benefits: no tmpfs leak. |
| 1.3.6 | Wait preserving overlay | ✅ | ❌ | ❌ | 👑 | 1d | Keep overlay for build engine to inspect. Benefits: `pelagos build` can read intermediate layers. |
| 1.3.7 | Btrfs detection for overlay | ✅ | ❌ | ❌ | 👑 | 1d | Detect btrfs, fall back to fuse-overlayfs. Benefits: btrfs has 64-bit inodes that overflow in user namespace. |

### 1.4 Bind Mounts & Volumes

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.4.1 | Bind mount (RW) | ✅ | ✅ | ❌ | 👑 | 1d | Mount host dir into container read-write. Benefits: shared data between host and container. |
| 1.4.2 | Bind mount (RO) | ✅ | ✅ | ❌ | 👑 | 1d | Mount host dir read-only. Benefits: config injection without container modification. |
| 1.4.3 | tmpfs mount | ✅ | ✅ | ❌ | 👑 | 1d | In-memory writable filesystem. Benefits: /tmp, /run inside container, works with read-only rootfs. |
| 1.4.4 | Named volumes | ✅ | ❌ | ❌ | 👑 | 2d | Persistent host dir under /var/lib/pelagos/volumes. Benefits: data survives container restart, shareable between containers. |
| 1.4.5 | OCI-ordered mount list | ✅ | ❌ | ❌ | 👑 | 1d | Preserve mount order from OCI config.json. Benefits: /proc/mountinfo order matches OCI spec, required for compat tests. |
| 1.4.6 | Container links (--link) | ✅ | ❌ | ❌ | 👑 | 1d | /etc/hosts entry to another container by name. Benefits: legacy Docker compat, service discovery without DNS. |

### 1.5 Security (Seccomp)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.5.1 | Docker default seccomp profile | ✅ | ❌ | ❌ | 👑👤 | 2d | Blocks ~44 dangerous syscalls (mount, ptrace, bpf, etc.). Benefits: defense-in-depth, industry standard, prevents container escape. |
| 1.5.2 | Minimal seccomp profile | ✅ | ❌ | ❌ | 👑👤 | 1d | ~40 essential syscalls only. Benefits: maximum restriction for untrusted code. |
| 1.5.3 | Docker + io_uring profile | ✅ | ❌ | ❌ | 👑👤 | 1d | Docker profile but allow io_uring syscalls. Benefits: database workloads need async I/O. |
| 1.5.4 | Custom BPF program | ✅ | ❌ | ❌ | 👑👤 | 1d | Apply arbitrary seccompiler BpfProgram. Benefits: fine-grained syscall whitelisting. |
| 1.5.5 | seccomp applied last in pre_exec | ✅ | ❌ | ❌ | 👑👤 | 1d | Seccomp filter installed after all setup (mount, setuid). Benefits: setup syscalls not blocked. |
| 1.5.6 | SECCOMP_RET_USER_NOTIF | ✅ | ❌ | ❌ | 👑 | 5d | Userspace syscall interception (Linux 5.0+). Benefits: proxy connect/mount without CAP_SYS_ADMIN, audit sensitive syscalls. |

### 1.6 Security (Capabilities & Privileges)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.6.1 | Capability bitflags | ✅ | ❌ | ❌ | 👑👤 | 1d | Full bitflags (0-40) matching Linux capability.h. Benefits: fine-grained privilege control. |
| 1.6.2 | Drop specific capabilities | ✅ | ❌ | ❌ | 👑👤 | 1d | Keep only specified caps, drop rest. Benefits: least-privilege principle. |
| 1.6.3 | Drop ALL capabilities | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Empty capability set. Benefits: maximum security for simple workloads. |
| 1.6.4 | DEFAULT_CAPS (Podman compat) | ✅ | ❌ | ❌ | 👑👤 | 1d | Safe default set: CHOWN, DAC_OVERRIDE, FOWNER, FSETID, KILL, NET_BIND_SERVICE, SETFCAP, SETGID, SETPCAP, SETUID, SYS_CHROOT. Benefits: matches Podman defaults, good balance of security and functionality. |
| 1.6.5 | Ambient capabilities | ✅ | ❌ | ❌ | 👑 | 1d | PR_CAP_AMBIENT_RAISE for non-root users. Benefits: capabilities survive exec() for non-root users. |
| 1.6.6 | No-new-privileges | ✅ | ❌ | ❌ | 👑👤 | 0.5d | PR_SET_NO_NEW_PRIVS. Benefits: prevents setuid/setgid escalation, required for seccomp. |
| 1.6.7 | Privileged mode | ✅ | ❌ | ❌ | 👑 | 1d | All capabilities + no seccomp + RW /sys. Benefits: needed for kubeadm, CNI, SPIRE. |
| 1.6.8 | OOM score adjustment | ✅ | ❌ | ❌ | 👑 | 0.5d | Write to /proc/self/oom_score_adj. Benefits: priority control when OOM killer fires. |

### 1.7 Security (LSM & Labels)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.7.1 | Landlock LSM rules | ✅ | ❌ | ❌ | 👤 | 3d | Path-based access control (Linux 5.13+). Benefits: sandbox filesystem without root, self-contained (no external profiles), survives exec. Landlock has 4 ABI versions (5.13, 5.19, 6.2, 6.7). |
| 1.7.2 | AppArmor profiles | ✅ | ❌ | ❌ | 👑 | 1d | Write profile name to /proc/self/attr/apparmor/exec. Benefits: MAC on top of DAC, matches Docker security model. |
| 1.7.3 | SELinux labels | ✅ | ❌ | ❌ | 👑 | 1d | Write label to /proc/self/attr/exec. Benefits: MAC for RHEL/CentOS systems. |

### 1.8 Resource Limits (rlimits)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.8.1 | RLIMIT_NOFILE (max fds) | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Limit open file descriptors. Benefits: prevent fd exhaustion. |
| 1.8.2 | RLIMIT_AS (address space) | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Limit virtual memory. Benefits: rough memory limit without cgroups. |
| 1.8.3 | RLIMIT_CPU (CPU time) | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Limit CPU seconds. Benefits: basic CPU enforcement. |
| 1.8.4 | All rlimit types | ✅ | ❌ | ❌ | 👑👤 | 1d | Generic interface for any rlimit. Benefits: RLIMIT_NPROC, RLIMIT_STACK, etc. |

### 1.9 Resource Limits (cgroups v2)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.9.1 | memory.max | ✅ | ✅ | ❌ | 👑 | 1d | Hard memory limit (OOM-kill when exceeded). Benefits: prevent runaway memory usage. |
| 1.9.2 | memory.low (reservation) | ✅ | ❌ | ❌ | 👑 | 0.5d | Soft memory guarantee. Benefits: best-effort protection from reclaim. |
| 1.9.3 | memory.swap.max | ✅ | ❌ | ❌ | 👑 | 0.5d | Swap limit (independent of memory). Benefits: prevent swap thrashing. |
| 1.9.4 | cpu.weight (shares) | ✅ | ❌ | ❌ | 👑 | 0.5d | Relative CPU weight 1-10000. Benefits: proportional CPU distribution. |
| 1.9.5 | cpu.max (quota/period) | ✅ | ❌ | ❌ | 👑 | 0.5d | Absolute CPU limit in µs. Benefits: guarantee max CPU usage. |
| 1.9.6 | pids.max | ✅ | ❌ | ❌ | 👑 | 0.5d | Limit number of processes/threads. Benefits: prevent fork bombs. |
| 1.9.7 | cpuset.cpus / cpuset.mems | ✅ | ❌ | ❌ | 👑 | 0.5d | Pin to specific CPUs/memory nodes. Benefits: latency-sensitive workloads. |
| 1.9.8 | hugetlb.<size>.max | ✅ | ❌ | ❌ | 👑 | 0.5d | Hugepage limits. Benefits: database workloads using hugepages. |
| 1.9.9 | io.weight (blkio) | ✅ | ❌ | ❌ | 👑 | 0.5d | Block I/O weight 10-1000. Benefits: proportional disk bandwidth. |
| 1.9.10 | io.max per-device BPS/IOPS | ✅ | ❌ | ❌ | 👑 | 1d | Per-device read/write rate limits. Benefits: prevent I/O noise from noisy neighbors. |
| 1.9.11 | Device rules (v1 only) | ✅ | ❌ | ❌ | 👑 | 1d | Allow/deny device access via cgroup. Benefits: restrict /dev access. |
| 1.9.12 | net_cls classid | ✅ | ❌ | ❌ | 👑 | 0.5d | Traffic classification tag. Benefits: integrate with tc/iptables. |
| 1.9.13 | net_prio ifpriomap | ✅ | ❌ | ❌ | 👑 | 0.5d | Per-interface network priority. Benefits: QoS for multi-net containers. |
| 1.9.14 | resource_stats() query | ✅ | ❌ | ❌ | 👑 | 1d | Read memory/CPU/PID stats from cgroup. Benefits: monitoring, autoscaling. |

### 1.10 Process Execution

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.10.1 | PTY/Interactive session | ✅ | ❌ | ❌ | 👑👤 | 3d | Full pty.rs: openpty, raw mode relay, SIGWINCH, TerminalGuard RAII. Benefits: `kubectl exec -it`, interactive shells. |
| 1.10.2 | stdio capture (stdout/stderr) | ✅ | ✅ | ❌ | 👑👤 | 1d | Pipe stdout/stderr to parent. Benefits: logging, `kubectl logs`. |
| 1.10.3 | Working directory (chroot-relative) | ✅ | ✅ | ❌ | 👑👤 | 0.5d | Set CWD inside container. Benefits: OCI process.cwd support. |
| 1.10.4 | Hostname (sethostname) | ✅ | ✅ | Partial | 👑 | 0.5d | Set hostname via UTS namespace. Benefits: pod identity. |
| 1.10.5 | UID/GID inside container | ✅ | ✅ | ❌ | 👑👤 | 1d | Run as specific uid/gid. Benefits: security (don't run as root). |
| 1.10.6 | Supplementary groups | ✅ | ❌ | ❌ | 👑 | 0.5d | AdditionalGIDs from OCI spec. Benefits: group-based file access. |
| 1.10.7 | umask | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Set file creation mask. Benefits: OCI process.user.umask. |
| 1.10.8 | Environment variable control | ✅ | ✅ | ❌ | 👑👤 | 1d | env_clear(), env(), merge patterns. Benefits: OCI ENV support. |
| 1.10.9 | Sysctl (kernel params) | ✅ | ❌ | ❌ | 👑 | 1d | Write /proc/sys values. Benefits: networking tuning per container. |
| 1.10.10 | Device nodes (mknod) | ✅ | ❌ | ❌ | 👑 | 1d | Create device nodes inside container. Benefits: /dev/fuse, /dev/net/tun. |
| 1.10.11 | /dev symlinks | ✅ | ❌ | ❌ | 👑 | 0.5d | Create symlinks in /dev. Benefits: compatibility with apps expecting specific device paths. |

### 1.11 Wasm/WASI Runtime

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.11.1 | Wasm binary detection | ✅ | ❌ | ❌ | 👑👤 | N/A | Magic bytes (\0asm) detection. z8s doesn't need Wasm support. |
| 1.11.2 | wasmtime/WasmEdge backend | ✅ | ❌ | ❌ | 👑👤 | N/A | Wasm subprocess execution. z8s doesn't need Wasm support. |

### 1.12 OCI Lifecycle (config.json format)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 1.12.1 | OCI config.json parser | ✅ | ✅ | ❌ | 👑 | 2d | Parse OCI Runtime Spec config.json. Benefits: runc-compatible bundles. |
| 1.12.2 | OCI create (fork + shim) | ✅ | ✅ | ❌ | 👑 | 3d | Double-fork, exec.sock sync. Benefits: OCI lifecycle compliance. |
| 1.12.3 | OCI start (socket connect) | ✅ | ✅ | ❌ | 👑 | 1d | Send byte to unblock pre_exec. Benefits: OCI lifecycle compliance. |
| 1.12.4 | OCI state (JSON output) | ✅ | ✅ | ❌ | 👑 | 0.5d | Print container state as JSON. Benefits: `runc state` compat. |
| 1.12.5 | OCI kill (signal) | ✅ | ✅ | ❌ | 👑 | 0.5d | Send signal to container. Benefits: `runc kill` compat. |
| 1.12.6 | OCI delete (cleanup) | ✅ | ✅ | ❌ | 👑 | 0.5d | Remove state directory. Benefits: `runc delete` compat. |
| 1.12.7 | OCI hooks (prestart/poststop) | ✅ | ❌ | ❌ | 👑 | 2d | Execute hook binaries at lifecycle events. Benefits: OCI compliance, CNI plugin integration. |
| 1.12.8 | Console socket (SCM_RIGHTS) | ✅ | ❌ | ❌ | 👑 | 2d | Send PTY master fd via Unix socket. Benefits: OCI terminal support. |
| 1.12.9 | PTY slave wiring in pre_exec | ✅ | ❌ | ❌ | 👑 | 1d | setsid() + dup2 + TIOCSCTTY. Benefits: proper terminal session. |
| 1.12.10 | OCI annotations | ✅ | ❌ | ❌ | 👑 | 0.5d | Pass-through key-value metadata. Benefits: CRI uses annotations for pod metadata. |

---

## 2. NETWORKING (z8s: `network/crate`, Pelagos: `network.rs`, `netlink.rs`, `nfnetlink.rs`, `dns.rs`, src2: `netmux/`, `network/`)

z8s-new has a `Netmux`-based networking crate but it's simpler than both Pelagos and src2.

### 2.1 Core Networking

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 2.1.1 | Loopback (lo up, 127.0.0.1) | ✅ | ✅ | ✅ | 👑👤 | Done | Bring up loopback via ioctl. Required for localhost. |
| 2.1.2 | veth pair creation | ✅ | ✅ | ✅ | 👑 | Done | Create virtual ethernet pair. Required for bridge networking. |
| 2.1.3 | Bridge create/manage | ✅ | ✅ | ✅ | 👑 | Done | Linux bridge via netlink. Required for pod networking. |
| 2.1.4 | Named network namespace | ✅ | ✅ | ❌ | 👑 | 1d | Named netns at /run/netns/{name}. Benefits: debuggable via `ip netns list`, no PID race. |
| 2.1.5 | Default bridge (pelagos0) | ✅ | ❌ | ❌ | 👑 | 1d | Auto-bootstrap default bridge network. Benefits: out-of-box networking. |
| 2.1.6 | Network definition persistence | ✅ | ✅ | ❌ | 👑 | 2d | NetworkDef with name/subnet/gateway saved to JSON. Benefits: named networks survive reboot. |
| 2.1.7 | Auto subnet allocation from pool | ✅ | ✅ | ❌ | 👑 | 2d | Carve /24 from 10.99.0.0/16 pool. Benefits: automatic network creation. |
| 2.1.8 | Per-network IPAM (flock) | ✅ | ✅ | ❌ | 👑 | 2d | File-locked next-IP file per network. Benefits: no IP conflicts between concurrent spawns. |

### 2.2 Advanced Networking

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 2.2.1 | IPv6 dual-stack (ULA /64) | ✅ | ✅ | ❌ | 👑 | 5d | Deterministic fd00::/8 per network. Benefits: IPv6-only clusters, dual-stack workloads. |
| 2.2.2 | IPv6 NDP pre-seed | ✅ | ❌ | ❌ | 👑 | 1d | Pre-seed neighbor cache for bridge MAC. Benefits: prevents first-packet loss on IPv6. |
| 2.2.3 | NAT (MASQUERADE) nftables | ✅ | ✅ | ❌ | 👑 | 3d | nftables MASQUERADE rule. Benefits: internet access from containers. |
| 2.2.4 | Reference-counted NAT | ✅ | ❌ | ❌ | 👑 | 1d | Nat refcount per network, removed when last container exits. Benefits: no stale nftables rules. |
| 2.2.5 | Port forwarding (DNAT) | ✅ | ✅ | ❌ | 👑 | 3d | nftables PREROUTING DNAT. Benefits: external → container port mapping. |
| 2.2.6 | Userspace TCP port proxy | ✅ | ❌ | ❌ | 👑 | 3d | Tokio async TCP relay for localhost DNAT. Benefits: nftables DNAT only works on PREROUTING (not localhost). |
| 2.2.7 | Userspace UDP port proxy | ✅ | ❌ | ❌ | 👑 | 2d | Std thread UDP relay. Benefits: UDP port forwarding. |
| 2.2.8 | Multi-network (eth1, eth2) | ✅ | ❌ | ❌ | 👑 | 2d | Secondary bridge attachments. Benefits: multi-homed pods. |
| 2.2.9 | Container network namespace join | ✅ | ✅ | ❌ | 👑 | 1d | Join another container's netns via named netns. Benefits: shared networking (like Docker --network=container:). |
| 2.2.10 | Pod sandbox / pause container | ✅ | ❌ | ❌ | 👑 | 3d | Pause process holds namespaces open. Benefits: Kubernetes pod sandbox model, containers share NET/IPC/UTS. |
| 2.2.11 | Pasta rootless networking | ✅ | ❌ | ❌ | 👤 | 5d | TAP-based user-mode networking. Benefits: full internet without root. z8s attempted rootless before and failed. |

### 2.3 DNS

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 2.3.1 | /etc/resolv.conf injection (bind-mount) | ✅ | ✅ | ✅ | 👑 | 1d | Write per-container resolv.conf. Benefits: container DNS config. |
| 2.3.2 | DNS search domains | ✅ | ✅ | ❌ | 👑 | 0.5d | `search` line in resolv.conf. Benefits: short-name resolution like `svc.cluster.local`. |
| 2.3.3 | DNS resolver options | ✅ | ❌ | ❌ | 👑 | 0.5d | `options` line (ndots:5, etc.). Benefits: DNS resolution tuning. |
| 2.3.4 | Host upstream DNS resolution | ✅ | ❌ | ❌ | 👑 | 1d | Read real upstream DNS from /run/systemd/resolve/. Benefits: avoids systemd-resolved stub (127.0.0.53). |
| 2.3.5 | DNS daemon (builtin A-record) | ✅ | ✅ | ❌ | 👑 | 5d | UDP DNS server for container name resolution. Benefits: service discovery by name. |
| 2.3.6 | DNS daemon (dnsmasq backend) | ✅ | ❌ | ❌ | 👑 | 3d | dnsmasq as DNS backend option. Benefits: production-grade DNS with caching/EDNS/DNSSEC. |
| 2.3.7 | DNS INPUT firewall rules | ✅ | ❌ | ❌ | 👑 | 1d | nftables + iptables-nft compat INPUT rules. Benefits: containers can reach DNS daemon. |
| 2.3.8 | Per-network DNS config files | ✅ | ❌ | ❌ | 👑 | 1d | DNS config per network name. Benefits: multi-tenant DNS. |

### 2.4 Netlink & nftables (low-level)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 2.4.1 | Netlink bridge management | ✅ | ✅ | ✅ | 👑 | Done | Direct rtnetlink sockets. Benefits: no `ip` CLI dependency. |
| 2.4.2 | Netlink veth management | ✅ | ✅ | ✅ | 👑 | Done | |
| 2.4.3 | Netlink addr add/remove | ✅ | ✅ | ✅ | 👑 | Done | |
| 2.4.4 | Netlink route add/remove | ✅ | ✅ | ✅ | 👑 | Done | |
| 2.4.5 | Netlink link up/down | ✅ | ✅ | ✅ | 👑 | Done | |
| 2.4.6 | Netlink setns/in_netns | ✅ | ❌ | ❌ | 👑 | 1d | Execute callback inside named netns. Benefits: configure interfaces inside netns. |
| 2.4.7 | Netlink IPv6 addr/route | ✅ | ❌ | ❌ | 👑 | 2d | IPv6 address assignment and routing. Benefits: IPv6 dual-stack. |
| 2.4.8 | Netlink neighbor/NDP | ✅ | ❌ | ❌ | 👑 | 1d | Neighbor table management. Benefits: IPv6 NDP pre-seed. |
| 2.4.9 | nftables via nfnetlink | ✅ | ❌ | ❌ | 👑 | 5d | Direct nfnetlink (no `nft` CLI). Benefits: no forking, faster, more reliable. |
| 2.4.10 | nftables MASQUERADE NAT | ✅ | ❌ | ❌ | 👑 | 3d | |
| 2.4.11 | nftables DNAT port forward | ✅ | ❌ | ❌ | 👑 | 3d | |
| 2.4.12 | nftables DNS INPUT rules | ✅ | ❌ | ❌ | 👑 | 1d | |
| 2.4.13 | Veth name derivation (FNV-1a) | ✅ | ❌ | ❌ | 👑 | 1d | Deterministic veth names from hash. Benefits: no collisions, no counter races. |
| 2.4.14 | Netns teardown with retry | ✅ | ❌ | ❌ | 👑 | 1d | 50 retries with 100ms gaps, fallback to MNT_DETACH. Benefits: handles EBUSY race in kernel veth teardown (issue #183 pattern). |
| 2.4.15 | Netns process killer (orphans) | ✅ | ❌ | ❌ | 👑 | 1d | Kill processes still in target netns before deletion. Benefits: prevents EBUSY on netns deletion. |

### 2.5 Network Policy & Services

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 2.5.1 | Service (ClusterIP) | ❌ | ✅ | ❌ | 👑 | 5d | Virtual IP with pod backend resolution. Benefits: stable service endpoint. |
| 2.5.2 | TCP service proxy | ❌ | ✅ | ❌ | 👑 | 3d | Userspace TCP proxy for ClusterIP. Benefits: service backend load balancing. |
| 2.5.3 | Endpoints/EndpointSlices | ❌ | ✅ | ❌ | 👑 | 3d | Track healthy pod backends per service. Benefits: dynamic routing. |
| 2.5.4 | NetworkPolicy (nftables) | ❌ | ✅ | ❌ | 👑 | 5d | K8s NetworkPolicy → nftables sets. Benefits: pod-level firewall. |
| 2.5.5 | Ingress (HTTP reverse proxy) | ❌ | ✅ | ❌ | 👑 | 5d | HTTP ingress with optional TLS termination. Benefits: external access to services. |
| 2.5.6 | VNet CRD | ❌ | ✅ | ❌ | 👑 | 5d | Virtual network abstraction. Benefits: multi-tenant networking. |
| 2.5.7 | Subnet CRD | ❌ | ✅ | ❌ | 👑 | 3d | Subnet attached to VNet. Benefits: network segmentation. |
| 2.5.8 | NSG (security group rules) | ❌ | ✅ | ❌ | 👑 | 5d | Firewall rules per VNet/Subnet. Benefits: cloud-style security groups. |
| 2.5.9 | RouteTable CRD | ❌ | ✅ | ❌ | 👑 | 3d | Custom route tables per VNet. Benefits: traffic steering. |
| 2.5.10 | NP controller | ❌ | ✅ | ❌ | 👑 | 5d | Dedicated controller for NetworkPolicy → nftables compilation. Benefits: policy management. |
| 2.5.11 | Container IP resolution by name | ✅ | ❌ | ❌ | 👑 | 1d | Lookup container bridge IP by name from state.json. Benefits: container linking. |

---

## 3. IMAGE MANAGEMENT (z8s: `core/src/` has ImageConfig, Pelagos: `image.rs`, `build.rs`, src2: `cri/image.rs`)

### 3.1 OCI Image Pull

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 3.1.1 | OCI registry pull (anonymous) | ✅ | ✅ | ❌ | 👑👤 | 3d | Pull OCI images via oci-client. Benefits: `pelagos image pull alpine`. |
| 3.1.2 | OCI registry pull (auth) | ✅ | ✅ | ❌ | 👑👤 | 1d | Authenticated pulls (docker login). Benefits: private registries. |
| 3.1.3 | Registry login/logout | ✅ | ❌ | ❌ | 👑👤 | 1d | Write/read ~/.docker/config.json. Benefits: credential management. |
| 3.1.4 | Insecure registry (HTTP) | ✅ | ❌ | ❌ | 👑👤 | 1d | Allow cleartext HTTP registries. Benefits: local/dev registries. |
| 3.1.5 | OCI image push | ✅ | ❌ | ❌ | 👑👤 | 3d | Push local image to registry. Benefits: share built images. |
| 3.1.6 | OCI image tag | ✅ | ❌ | ❌ | 👑👤 | 0.5d | Tag local image with new reference. Benefits: version management. |
| 3.1.7 | OCI image save/load | ✅ | ❌ | ❌ | 👑👤 | 2d | OCI Image Layout tar export/import. Benefits: air-gapped transfer. |
| 3.1.8 | Registry mirror support | ✅ | ❌ | ❌ | 👑👤 | 2d | Configurable registry mirrors (config.toml). Benefits: air-gapped deployments. |

### 3.2 Layer Management

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 3.2.1 | Content-addressable layer store | ✅ | ✅ | ❌ | 👑👤 | 3d | SHA-256 keyed /var/lib/pelagos/layers. Benefits: dedup between images. |
| 3.2.2 | Layer extraction (tar+gzip) | ✅ | ✅ | ❌ | 👑👤 | 2d | Extract compressed tar layers. Benefits: OCI image runtime support. |
| 3.2.3 | Layer extraction (bzip2, xz) | ✅ | ❌ | ❌ | 👑👤 | 1d | Also support bz2 and xz compressed layers. Benefits: rare but spec-compliant. |
| 3.2.4 | OCI whiteout files | ✅ | ✅ | ❌ | 👑 | 2d | .wh.* → overlayfs char device (0,0); .wh..wh..opq → opaque xattr. Benefits: correct layer deletion semantics. |
| 3.2.5 | Layer diff_id tracking | ✅ | ❌ | ❌ | 👑👤 | 1d | Track uncompressed sha256 per layer. Benefits: OCI manifest chain verification. |
| 3.2.6 | Multi-layer overlay mount from store | ✅ | ❌ | ❌ | 👑👤 | 3d | Stack all image layers as overlay lower dirs. Benefits: run pulled images. |

### 3.3 Image Build

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 3.3.1 | Remfile parser (Dockerfile-like) | ✅ | ❌ | ❌ | 👑 | 5d | Parse Remfile with FROM, RUN, COPY, ADD, CMD, ENTRYPOINT, ENV, WORKDIR, EXPOSE, LABEL, USER, ARG, HEALTHCHECK. |
| 3.3.2 | RUN instruction execution | ✅ | ❌ | ❌ | 👑 | 3d | Run commands in overlay snapshot. Benefits: build layers. |
| 3.3.3 | COPY instruction | ✅ | ❌ | ❌ | 👑 | 2d | Copy files from context into image. Benefits: add application code. |
| 3.3.4 | Multi-stage builds | ✅ | ❌ | ❌ | 👑 | 3d | FROM ... AS / COPY --from=. Benefits: smaller final images. |
| 3.3.5 | ADD (URL + archive extraction) | ✅ | ❌ | ❌ | 👑 | 3d | Download URLs, auto-extract archives. Benefits: fetch dependencies. |
| 3.3.6 | Build cache (SHA256 keyed) | ✅ | ❌ | ❌ | 👑 | 3d | Cache layers by (parent_hash + instruction). Benefits: incremental builds. |
| 3.3.7 | ARG with variable substitution | ✅ | ❌ | ❌ | 👑 | 2d | Build-time variables with $VAR / ${VAR}. Benefits: configurable builds. |
| 3.3.8 | .remignore (gitignore patterns) | ✅ | ❌ | ❌ | 👑👤 | 1d | Exclude files from build context via ignore crate. Benefits: smaller context. |
| 3.3.9 | Layer creation from dir (tar+gzip) | ✅ | ❌ | ❌ | 👑 | 2d | Create and store layer tar.gz from directory. Benefits: build output persistence. |
| 3.3.10 | Image config (Env, Cmd, Entrypoint, WorkingDir, Labels, ExposedPorts) | ✅ | ✅ | Partial | 👑 | 2d | Apply image config as container defaults. Benefits: `CMD` and `ENTRYPOINT` from image. |

---

## 4. STORAGE (z8s: none, Pelagos: minimal (volumes only), src2: `storage/`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 4.1 | StorageClass | ❌ | ✅ | ❌ | 👑 | 3d | Storage class definitions with provisioner reference. Benefits: dynamic provisioning. |
| 4.2 | StorageClass default seeding | ❌ | ✅ | ❌ | 👑 | 1d | Auto-seed "standard" StorageClass on first run. Benefits: out-of-box storage. |
| 4.3 | Hostpath provisioner | ❌ | ✅ | ❌ | 👑 | 2d | Provision PVs from host directories. Benefits: local storage. |
| 4.4 | Loop device provisioner | ❌ | ✅ | ❌ | 👑 | 3d | Provision PVs from loopback devices (sparse files). Benefits: disk-backed PVCs without real disks. |
| 4.5 | ProvisionerDispatcher | ❌ | ✅ | ❌ | 👑 | 2d | Route PVCs to correct provisioner based on StorageClass. Benefits: multiple storage backends. |
| 4.6 | WaitForFirstConsumer binding | ❌ | ✅ | ❌ | 👑 | 3d | Delay PV provisioning until pod scheduled on node. Benefits: provision on correct NUMA node. |
| 4.7 | PVC → PV automatic binding | ❌ | ✅ | ❌ | 👑 | 3d | Match PVC requests with available PVs. Benefits: automatic storage allocation. |
| 4.8 | ConfigMap resource | ❌ | ✅ | ❌ | 👑 | 2d | Key-value config data. Benefits: env vars, volume mounts. |
| 4.9 | Secret resource | ❌ | ✅ | ❌ | 👑 | 2d | Sensitive data (base64 encoded). Benefits: passwords, tokens, certs. |
| 4.10 | EmptyDir volumes | ❌ | ✅ | ❌ | 👑 | 1d | Ephemeral pod-scoped volumes. Benefits: scratch space. |
| 4.11 | VolumeMount resolution | ❌ | ✅ | ❌ | 👑 | 2d | Resolve volume mounts from ConfigMaps/Secrets/PVCs. Benefits: k8s-compatible volume system. |

---

## 5. CONTROLLER / SCHEDULER (z8s: `controller/crate`, Pelagos: minimal (compose only), src2: `scheduler/`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 5.1 | Event-driven reconciliation | ❌ | ✅ | ❌ | 👑 | 5d | StoreEventHub subscriptions react to changes instantly. Benefits: no polling delay. |
| 5.2 | Poll-based tick loop | ❌ | ✅ | ✅ | 👑 | Done | Timer-based reconciliation. Benefits: simple, works. |
| 5.3 | Component Registry trait | ❌ | ✅ | ❌ | 👑 | 3d | Component trait with reconcile/on_apply/on_delete. Benefits: pluggable resource controllers. |
| 5.4 | Pipeline Stage trait | ❌ | ✅ | ❌ | 👑 | 3d | PipelineStage with interests() + on_created/on_deleted. Benefits: composable resource processing. |
| 5.5 | Pod assignment (least-loaded) | ❌ | ✅ | ✅ | 👑 | Done | Pick least-loaded node for pod. |
| 5.6 | Reassign on node death | ❌ | ✅ | ✅ | 👑 | Done | Detect dead nodes, reassign pods. |
| 5.7 | Deployment controller | ❌ | ✅ | ❌ | 👑 | 5d | ReplicaSet management, rolling update, scaling. Benefits: declarative app management. |
| 5.8 | WaitForFirstConsumer scheduling | ❌ | ✅ | ❌ | 👑 | 3d | Schedule pod then provision volumes on target node. Benefits: local storage placement. |
| 5.9 | IP-based scoring | ❌ | ✅ | Partial | 👑 | 2d | Score nodes by IP load. Benefits: network-aware scheduling. |
| 5.10 | Scheduler lease (leader election) | ❌ | ✅ | ✅ | 👑 | 2d | Epoch-based lease with auto-renewal. Benefits: HA controller. |
| 5.11 | Heartbeat / node liveness | ❌ | ✅ | ❌ | 👑 | 2d | Periodic heartbeat to store, 30s deadline. Benefits: detect dead nodes. |

### 5.1 Pod Lifecycle

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 5.1.1 | Restart policy Always | ❌ | ✅ | ❌ | 🔱 | 2d | Always restart container on exit. Benefits: long-running services. |
| 5.1.2 | Restart policy OnFailure | ❌ | ✅ | ❌ | 🔱 | 2d | Restart only on non-zero exit. Benefits: batch jobs. |
| 5.1.3 | Restart policy Never | ❌ | ✅ | ❌ | 🔱 | 1d | Never restart. Benefits: one-shot tasks. |
| 5.1.4 | CrashLoopBackOff (exponential) | ❌ | ✅ | ❌ | 🔱 | 3d | Backoff: 1s→2s→4s→...→300s max, reset after successful run. Benefits: prevents restart loop. |
| 5.1.5 | Zombie reaping | ❌ | ✅ | ❌ | 🔱 | 2d | waitpid() on SIGCHLD to prevent zombies. Benefits: PID 1 responsibility, prevents process table exhaustion. |
| 5.1.6 | Readiness probe (HTTP/TCP/exec) | ❌ | ✅ | ❌ | 👑 | 3d | Periodic check if container is ready to serve. Benefits: service only routes to ready pods. |
| 5.1.7 | Liveness probe (HTTP/TCP/exec) | ❌ | ✅ | ❌ | 👑 | 3d | Periodic check if container is alive. Benefits: restart unhealthy containers. |
| 5.1.8 | Startup probe | ❌ | ✅ | ❌ | 👑 | 2d | Delayed startup check (for slow-starting apps). Benefits: doesn't kill slow starters. |
| 5.1.9 | Probe runner (exec) | ❌ | ✅ | ❌ | 🔱 | 2d | Run command inside container namespace for exec probes. Benefits: in-band health checking. |
| 5.1.10 | Probe runner (TCP) | ❌ | ✅ | ❌ | 👑 | 2d | TCP connect to container IP:port. Benefits: network-level health checking. |
| 5.1.11 | Container log buffering | ❌ | ✅ | ❌ | 🔱 | 2d | Capture stdout/stderr per container. Benefits: `kubectl logs` support. |
| 5.1.12 | Parallel pod startup (semaphore) | ❌ | ✅ | ❌ | 👑 | 1d | Semaphore-limited concurrent starts. Benefits: controlled boot storm. |
| 5.1.13 | Container exec (enter namespace) | ❌ | ✅ | ❌ | 👑 | 3d | exec into running container namespace. Benefits: `kubectl exec`, debugging. |
| 5.1.14 | ServiceAccount token mounting | ❌ | ✅ | ❌ | 👑 | 2d | Automount projected SA tokens in pods. Benefits: pod identity for API auth. |
| 5.1.15 | Pod status (Phase/Ready/Reason) | ❌ | ✅ | ✅ | 👑 | 1d | Track PodRunning, PodPending, PodSucceeded, PodFailed. |

---

## 6. API SERVER (z8s: `api/crate` stub, Pelagos: none, src2: `api/`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 6.1 | REST CRUD for all resources | ❌ | ✅ | ❌ | 👑 | 10d | Full HTTP API for Pod, Deployment, Service, ConfigMap, Secret, PV, PVC, etc. Benefits: kubectl compatibility. |
| 6.2 | Watch / SSE endpoint | ❌ | ✅ | ❌ | 👑 | 5d | Server-sent events for resource changes. Benefits: real-time kubectl get -w. |
| 6.3 | Protobuf decoder | ❌ | ✅ | ❌ | 👑 | 5d | K8s protobuf format support. Benefits: kubectl uses protobuf by default. |
| 6.4 | Resource table (list response) | ❌ | ✅ | ❌ | 👑 | 2d | Table-formatted API responses. Benefits: `kubectl get` display. |
| 6.5 | Discovery API (API groups/versions) | ❌ | ✅ | ❌ | 👑 | 3d | /api/v1, /apis, OpenAPI discovery. Benefits: kubectl compatibility. |
| 6.6 | Admission control | ❌ | ✅ | ❌ | 👑 | 5d | Mutating/validating admission webhooks. Benefits: policy enforcement before persistence. |
| 6.7 | API compat layer (k8s API) | ❌ | ✅ | ❌ | 👑 | 10d | Match K8s API conventions. Benefits: drop-in kubectl replacement. |
| 6.8 | Metrics endpoints | ❌ | ✅ | ❌ | 👑 | 3d | /metrics, top pods/nodes. Benefits: monitoring integration. |
| 6.9 | System endpoints (healthz, readyz) | ❌ | ✅ | ❌ | 👑 | 1d | Health check endpoints. Benefits: kubelet integration. |

---

## 7. AUTH & RBAC (z8s: none, Pelagos: none, src2: `bootstrap/`, `api/auth/`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 7.1 | Role / ClusterRole | ❌ | ✅ | ❌ | 👑 | 5d | RBAC role definitions. Benefits: access control. |
| 7.2 | RoleBinding / ClusterRoleBinding | ❌ | ✅ | ❌ | 👑 | 3d | Bind roles to users/groups/SAs. Benefits: authorization. |
| 7.3 | ServiceAccount management | ❌ | ✅ | ❌ | 👑 | 3d | Create/manage ServiceAccounts. Benefits: pod identity. |
| 7.4 | TokenRegistry | ❌ | ✅ | ❌ | 👑 | 3d | ServiceAccount token management + pod mounting. Benefits: API auth from pods. |
| 7.5 | Admin ServiceAccount bootstrap | ❌ | ✅ | ❌ | 👑 | 2d | Create admin SA on first start. Benefits: out-of-box admin access. |
| 7.6 | RBAC mode (enforce/permissive) | ❌ | ✅ | ❌ | 👑 | 2d | Configurable enforcement mode. Benefits: gradual RBAC adoption. |
| 7.7 | TLS cert auto-generation | ❌ | ✅ | ❌ | 👑 | 3d | Self-signed certs for API server. Benefits: HTTPS without manual config. |
| 7.8 | In-cluster API discovery | ❌ | ✅ | ❌ | 👑 | 2d | Discover API server from inside a pod. Benefits: pod → API communication. |

---

## 8. DISTRIBUTED STORE (z8s: `core/src/store/`, Pelagos: none, src2: `store/`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 8.1 | In-memory store | ❌ | ✅ | ✅ | 👑 | Done | MemoryBackend for testing. |
| 8.2 | Redb persistent store | ❌ | ❌ | ✅ | 👑 | 3d | Redb-backed persistent storage. Benefits: data survives restart. |
| 8.3 | StoreBackend trait | ❌ | ✅ | ✅ | 👑 | Done | Generic backend interface. |
| 8.4 | StoreEventHub (event bus) | ❌ | ✅ | Partial | 👑 | 3d | Event subscription for resource changes. Benefits: event-driven reconcile. |
| 8.5 | Store watch/subscribe | ❌ | ✅ | ✅ | 👑 | 2d | Watch resource changes via channel. |
| 8.6 | Resource snapshots (full state) | ❌ | ✅ | ✅ | 👑 | 1d | Get full state snapshot. Benefits: anti-entropy. |
| 8.7 | Gossip protocol (WebSocket) | ❌ | ✅ | Stub | 👑 | 10d | WebSocket-based state sync between nodes. Benefits: multi-node HA. |
| 8.8 | Anti-entropy (background sync) | ❌ | ✅ | ❌ | 👑 | 5d | Background consistency checking. Benefits: self-healing store. |
| 8.9 | Lease records (leader election) | ❌ | ✅ | ✅ | 👑 | 2d | Epoch-based leases for leader election. Benefits: HA controller. |
| 8.10 | Join tokens | ❌ | ✅ | ❌ | 👑 | 3d | Token-based cluster auth. Benefits: secure node joining. |
| 8.11 | apply/write_status/assign_node split | ❌ | ✅ | ❌ | 👑 | 1d | Separate spec write from status write. Benefits: clear resource lifecycle. |

---

## 9. INIT MODE (PID 1) (z8s: `src2/init.rs`, `src2/node.rs`, Pelagos: none)

These are features unique to z8s's role as PID 1.

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 9.1 | prctl subreaper | ❌ | ✅ | ❌ | 🔱 | 1d | Set PR_SET_CHILD_SUBREAPER. Benefits: collect orphaned grandchildren. |
| 9.2 | signalfd signal handling | ❌ | ✅ | ❌ | 🔱 | 2d | signalfd for reliable signal delivery. Benefits: non-lossy signal handling. |
| 9.3 | SIGCHLD → waitpid reaper | ❌ | ✅ | ❌ | 🔱 | 2d | Reap all children on SIGCHLD. Benefits: no zombie processes. |
| 9.4 | SIGTERM → graceful shutdown | ❌ | ✅ | ❌ | 🔱 | 2d | Forward signal, wait for exit, SIGKILL after timeout. Benefits: clean shutdown. |
| 9.5 | Grace period / stop timeout | ❌ | ✅ | ❌ | 🔱 | 1d | Configurable SIGTERM→SIGKILL window. Benefits: app-driven shutdown. |
| 9.6 | Shutdown orchestration (reverse order) | ❌ | ✅ | ❌ | 🔱 | 3d | Stop pods in reverse dependency order. Benefits: clean teardown. |
| 9.7 | D-state watchdog (emergency exit) | ❌ | ✅ | ❌ | 🔱 | 2d | Force _exit() if shutdown stalls in D-state. Benefits: prevents hung system. |
| 9.8 | Cleanup orphan veths on start | ❌ | ✅ | ❌ | 👑🔱 | 1d | Remove dangling veth from previous crashes. Benefits: no interface leak. |
| 9.9 | Cleanup nftables on shutdown | ❌ | ✅ | ❌ | 👑🔱 | 1d | Remove NAT/filter tables. Benefits: no stale firewall rules. |
| 9.10 | Manifest watcher (directory watch) | ❌ | ✅ | ❌ | 🔱 | 3d | Watch /etc/z8s/manifests/ for YAML changes. Benefits: file-based resource definition. |

---

## 10. CONFIG & BOOTSTRAP (z8s: `config.rs` both old and new, Pelagos: `config.rs`, `paths.rs`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 10.1 | TOML config file | ✅ | ✅ | ✅ | 👑👤 | Done | Config file with [network] section, defaults. |
| 10.2 | Rootless-aware paths | ✅ | ❌ | ❌ | 👤 | 3d | data_dir() vs runtime_dir() split, XDG compliance. Benefits: rootless operation. |
| 10.3 | Install validation (pre-flight) | ✅ | ❌ | ❌ | 👑 | 2d | Check directory existence/permissions at startup. Benefits: clear "run setup.sh" error. |
| 10.4 | Network config (default_subnet, auto_alloc_pool, default_dns) | ✅ | ❌ | ❌ | 👑 | 2d | Configurable networking defaults. Benefits: no hardcoded subnets. |
| 10.5 | Environment variable overrides | ✅ | ❌ | ❌ | 👑👤 | 1d | PELAGOS_DEFAULT_DNS, etc. Benefits: config without file. |
| 10.6 | Container stats (live resource usage) | ✅ | ❌ | ❌ | 👑 | 3d | `pelagos stats` — memory/CPU/PID per container. Benefits: monitoring. |
| 10.7 | Container logs (with follow) | ✅ | ✅ | ❌ | 👑🔱 | 2d | `pelagos logs -f`. Benefits: live log streaming. |
| 10.8 | Container inspect (JSON) | ✅ | ❌ | ❌ | 👑 | 1d | `pelagos container inspect`. Benefits: detailed state. |
| 10.9 | Container prune (remove all stopped) | ✅ | ❌ | ❌ | 👑 | 1d | `pelagos prune`. Benefits: cleanup. |
| 10.10 | Stale artifact cleanup | ✅ | ❌ | ❌ | 👑 | 2d | Remove stale netns/overlay/hosts dirs. Benefits: no tmpfs leaks. |
| 10.11 | Subscribe (NDJSON state events) | ✅ | ❌ | ❌ | 👑 | 3d | Stream container state events. Benefits: TUI/monitoring integration. |
| 10.12 | Node agent (daemon lifecycle) | ❌ | ✅ | ❌ | 🔱 | 5d | z8s node start/stop, PID tracking, lock files. Benefits: daemon management. |
| 10.13 | Reset command (wipe all state) | ❌ | ✅ | ❌ | 👑🔱 | 2d | Stop all, unmount, remove state. Benefits: clean slate. |

---

## 11. COMPOSE (z8s: none, Pelagos: `compose.rs`, `sexpr.rs`)

| # | Feature | Pelagos | z8s-old | z8s-new | Mode | Effort | Desc & Benefits |
|---|---------|---------|---------|---------|------|--------|-----------------|
| 11.1 | S-expression parser | ✅ | ❌ | ❌ | 👑👤 | 3d | Zero-dependency recursive descent parser. Benefits: compose file format. |
| 11.2 | Compose model (ServiceSpec, NetworkSpec) | ✅ | ❌ | ❌ | 👑👤 | 3d | Typed compose file representation. |
| 11.3 | Topological sort (Kahn's) | ✅ | ❌ | ❌ | 👑👤 | 1d | Dependency-ordered start with cycle detection. Benefits: correct service startup order. |
| 11.4 | TCP readiness polling | ✅ | ❌ | ❌ | 👑 | 2d | Connect to port with 250ms interval, 60s timeout. Benefits: wait for service before proceeding. |
| 11.5 | Supervisor (start/stop/log relay) | ✅ | ❌ | ❌ | 👑 | 5d | Manage service lifecycle with log multiplexing. Benefits: multi-service orchestration. |

---

# PART 2: Priority Migration Roadmap

Organized by impact and dependency order. Each phase can be done independently.

## Phase 0: Foundation (2-4 weeks)

Features that unblock everything else. These are the core container runtime
capabilities that every other feature depends on.

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P0 | Seccomp Docker profile | Pelagos | 2d | 👑👤 | Security foundation, unblocks secure containers |
| P0 | Capability management (bitflags + DEFAULT_CAPS) | Pelagos | 1d | 👑👤 | Security foundation |
| P0 | cgroups v2 (memory, CPU, PIDs) | Pelagos | 3d | 👑 | Resource tracking, scheduling decisions |
| P0 | cgroups ResourceStats | Pelagos | 1d | 👑 | Monitoring, autoscaling |
| P0 | Bind mounts (RW + RO) | Pelagos | 1d | 👑 | Volume mounts for pods |
| P0 | tmpfs mounts | Pelagos | 1d | 👑 | /tmp, /run, EmptyDir |
| P0 | Named volumes | Pelagos | 2d | 👑 | Persistent storage |
| P0 | PTY/interactive session | Pelagos | 3d | 👑👤 | kubectl exec -it |
| P0 | Container stdout/stderr capture | src2 | 1d | 🔱 | kubectl logs |
| P0 | Read-only rootfs + masked paths | Pelagos | 1d | 👑 | Security hardening |
| P0 | No-new-privileges | Pelagos | 0.5d | 👑👤 | Security, required for seccomp |

**Total Phase 0**: ~17 days

## Phase 1: Networking (2-4 weeks)

Build on Phase 0 to add full container networking.

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P1 | Named netns (debuggable) | Pelagos | 1d | 👑 | Replace PID-race approach |
| P1 | Per-network IPAM (flock) | Pelagos | 2d | 👑 | Reliable IP allocation |
| P1 | NAT (MASQUERADE) nftables | Pelagos | 3d | 👑 | Internet access |
| P1 | Port forwarding (DNAT + userspace proxy) | Pelagos | 5d | 👑 | Service exposure |
| P1 | Multi-network (eth1, eth2) | Pelagos | 2d | 👑 | Pod multi-homing |
| P1 | DNS search domains + options | Pelagos | 1d | 👑 | Service discovery |
| P1 | Pod sandbox / pause container | Pelagos | 3d | 👑 | K8s pod model |

**Total Phase 1**: ~17 days

## Phase 2: Image Management (2-3 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P2 | OCI registry pull (auth + anonymous) | Pelagos | 3d | 👑👤 | Run container images |
| P2 | Content-addressable layer store | Pelagos | 3d | 👑👤 | Image dedup |
| P2 | Multi-layer overlay mount | Pelagos | 3d | 👑👤 | Run multi-layer images |
| P2 | Image config (Cmd, Entrypoint, Env) | Pelagos | 2d | 👑👤 | OCI-compatible execution |
| P2 | OCI whiteout handling | Pelagos | 2d | 👑 | Correct layer deletion semantics |

**Total Phase 2**: ~13 days

## Phase 3: Pod Lifecycle (2-3 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P3 | Restart policies (Always/OnFailure/Never) | src2 | 2d | 🔱 | Core pod behavior |
| P3 | CrashLoopBackOff | src2 | 3d | 🔱 | Prevent restart loops |
| P3 | Zombie reaping | src2 | 2d | 🔱 | PID 1 responsibility |
| P3 | Health probes (liveness + readiness) | src2 | 5d | 👑 | Self-healing |
| P3 | Container exec (namespace join) | src2 | 3d | 👑 | Debugging |
| P3 | Log buffering | src2 | 2d | 🔱 | kubectl logs |
| P3 | ServiceAccount token mounting | src2 | 2d | 👑 | Pod identity |

**Total Phase 3**: ~19 days

## Phase 4: Controller Architecture (2-3 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P4 | Component trait + Registry | src2/refactor-plan | 3d | 👑 | Pluggable controllers |
| P4 | PipelineStage trait | src2/refactor-plan | 3d | 👑 | Composable resource processing |
| P4 | Deployment controller | src2 | 5d | 👑 | Declarative app management |
| P4 | Heartbeat / node liveness | src2 | 2d | 👑 | Detect dead nodes |
| P4 | Event-driven reconcile | src2 | 5d | 👑 | Instant reaction, not polling |

**Total Phase 4**: ~18 days

## Phase 5: Service & Network Resources (3-4 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P5 | Service (ClusterIP) | src2 | 5d | 👑 | Stable service endpoints |
| P5 | TCP service proxy | src2 | 3d | 👑 | Service load balancing |
| P5 | Endpoints/EndpointSlices | src2 | 3d | 👑 | Dynamic backend routing |
| P5 | NetworkPolicy (nftables) | src2 | 5d | 👑 | Pod firewall |
| P5 | VNet + Subnet CRDs | src2 | 5d | 👑 | Virtual networking |
| P5 | NSG (security groups) | src2 | 5d | 👑 | Cloud-style firewalls |

**Total Phase 5**: ~26 days

## Phase 6: Storage (2-3 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P6 | StorageClass | src2 | 3d | 👑 | Dynamic provisioning |
| P6 | Hostpath provisioner | src2 | 2d | 👑 | Local storage |
| P6 | PV/PVC binding | src2 | 3d | 👑 | Persistent storage claims |
| P6 | ConfigMap + Secret | src2 | 3d | 👑 | Config data |
| P6 | EmptyDir volumes | src2 | 1d | 👑 | Ephemeral scratch |

**Total Phase 6**: ~12 days

## Phase 7: API Server & Auth (3-4 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P7 | REST CRUD for all resources | src2 | 10d | 👑 | kubectl compatibility |
| P7 | Watch/SSE endpoint | src2 | 5d | 👑 | Real-time updates |
| P7 | RBAC (Role/ClusterRole/Binding) | src2 | 5d | 👑 | Access control |
| P7 | TLS certs | src2 | 3d | 👑 | HTTPS API |
| P7 | ServiceAccount + TokenRegistry | src2 | 3d | 👑 | Pod identity |

**Total Phase 7**: ~26 days

## Phase 8: HA & Distributed Store (3-4 weeks)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P8 | Gossip protocol | src2 | 10d | 👑 | Multi-node state sync |
| P8 | Anti-entropy | src2 | 5d | 👑 | Self-healing store |
| P8 | Join tokens | src2 | 3d | 👑 | Secure cluster joining |
| P8 | Graceful shutdown + cleanup | src2 | 3d | 🔱👑 | Clean teardown |

**Total Phase 8**: ~21 days

## Phase 9: Polish & Advanced (ongoing)

| Priority | Feature | From | Effort | Mode | Why |
|----------|---------|------|--------|------|-----|
| P9 | Landlock LSM | Pelagos | 3d | 👤 | Additional sandboxing |
| P9 | OOM score adjustment | Pelagos | 0.5d | 👑 | OOM priority |
| P9 | Sysctl (kernel params) | Pelagos | 1d | 👑 | Networking tuning |
| P9 | Device nodes (mknod) | Pelagos | 1d | 👑 | Device access |
| P9 | AppArmor profiles | Pelagos | 1d | 👑 | MAC on Ubuntu/Debian |
| P9 | SELinux labels | Pelagos | 1d | 👑 | MAC on RHEL |
| P9 | Mount propagation | Pelagos | 1d | 👑 | Volume mount semantics |
| P9 | Ingress controller | src2 | 5d | 👑 | HTTP ingress |
| P9 | RouteTable CRD | src2 | 3d | 👑 | Traffic steering |
| P9 | Image build (Remfile) | Pelagos | 5d | 👑 | Build images |
| P9 | OCI save/load/tag | Pelagos | 2d | 👑👤 | Image transfer |
| P9 | Container prune + cleanup | Pelagos | 2d | 👑 | Housekeeping |
| P9 | Stats + subscribe | Pelagos | 3d | 👑 | Monitoring |

**Total Phase 9**: ~28 days

---

# PART 3: Architecture Decision Record

## 3.1 Dependency Strategy: Direct Import vs Crate Extraction

For each feature, we have three options:

| Option | Pros | Cons | Best For |
|--------|------|------|----------|
| **Copy code** | No git dependency, full control | Maintenance burden | Simple features (<200 lines) |
| **Git dependency** | Automatic updates, less code | Version skew, build complexity | Stable features that change rarely |
| **Crate publish** | Semver, ecosystem contribution | Publishing overhead | General-purpose libraries |

**Recommendation**: Copy Pelagos code directly into z8s initially. z8s and Pelagos
have different enough goals (orchestrator vs runtime) that direct copy + adapt is
cleaner than a shared dependency. Document the source in comments.

## 3.2 Mode Decision: Root vs Rootless

z8s is primarily a **Root 👑** and **Init 🔱** tool. Rootless mode is aspirational.

| Mode | Current status | When to switch |
|------|---------------|----------------|
| 👑 Root | Primary mode | All current operations |
| 🔱 Init (PID 1) | Active | Container lifecycle management |
| 👤 Rootless | Failed in previous attempt | When USER namespace + overlayfs is reliably available |

## 3.3 Component Architecture

The refactoring plan at `/home/abb/dev/z8s/docs/refactoring-plan.md` defines the
target architecture. Key decisions:

1. **Component trait** — every resource kind has a Component impl
2. **PipelineStage trait** — cross-cutting concerns (DNS, NSG, IPAM) are stages
3. **ContainerSpec** — pure-data contract between components and CRI
4. **RuntimeProvider trait** — CRI behind a trait for testability
5. **NetworkEngine trait** — networking behind a trait for swapability

---

# PART 4: Quick Reference

## Feature Count Summary

| Domain | Total Features | Done | Partial | Missing |
|--------|---------------|------|---------|---------|
| 1. Container Runtime | 106 | 12 | 4 | 90 |
| 2. Networking | 63 | 7 | 0 | 56 |
| 3. Image Management | 22 | 0 | 1 | 21 |
| 4. Storage | 11 | 0 | 0 | 11 |
| 5. Controller/Scheduler | 33 | 3 | 2 | 28 |
| 6. API Server | 9 | 0 | 0 | 9 |
| 7. Auth & RBAC | 8 | 0 | 0 | 8 |
| 8. Distributed Store | 11 | 5 | 1 | 5 |
| 9. Init Mode (PID 1) | 10 | 0 | 0 | 10 |
| 10. Config & Bootstrap | 13 | 1 | 0 | 12 |
| 11. Compose | 5 | 0 | 0 | 5 |
| **Total** | **291** | **28** | **8** | **255** |

## Effort Summary

| Phase | Days | Features | Key Deliverables |
|-------|------|----------|------------------|
| Phase 0: Foundation | ~17d | 12 | Seccomp, cgroups, capabilities, mounts, PTY, logs |
| Phase 1: Networking | ~17d | 7 | IPAM, NAT, ports, multi-net, sandbox |
| Phase 2: Images | ~13d | 5 | OCI pull, layer store, multi-layer overlay |
| Phase 3: Pod Lifecycle | ~19d | 7 | Restart policies, probes, exec, crashloop |
| Phase 4: Controller | ~18d | 5 | Component/Pipeline traits, deployment, events |
| Phase 5: Services | ~26d | 6 | ClusterIP, NP, VNet, NSG |
| Phase 6: Storage | ~12d | 5 | StorageClass, PV/PVC, ConfigMap, Secret |
| Phase 7: API & Auth | ~26d | 5 | REST API, RBAC, TLS, SA |
| Phase 8: HA | ~21d | 4 | Gossip, anti-entropy, join tokens, shutdown |
| Phase 9: Polish | ~28d | 12 | Landlock, ingress, build, prune, stats |
| **Total** | **~197 days** | **68** | **(plus 187 lower-priority features)** |

## Source Code Reference

| Feature area | Best source to copy from | File(s) |
|-------------|-------------------------|---------|
| Container builder | Pelagos | `container.rs` (8,415 lines) |
| Seccomp profiles | Pelagos | `seccomp.rs` (~400 lines) |
| Capability bitflags | Pelagos | `container.rs` lines 676-760 |
| cgroups v2 | Pelagos | `cgroup.rs` (~500 lines) |
| cgroups ResourceStats | Pelagos | `cgroup.rs` lines 410+ |
| PTY/interactive | Pelagos | `pty.rs` (204 lines) |
| Named volumes | Pelagos | `container.rs` Volume struct |
| Networking stack | Pelagos | `network.rs` (3,000+ lines) |
| Netlink bridge/veth | Pelagos | `netlink.rs` (~1,500 lines) |
| nftables | Pelagos | `nfnetlink.rs` (~1,500 lines) |
| DNS daemon | Pelagos | `bin/pelagos-dns.rs`, `dns.rs` |
| OCI lifecycle | Pelagos | `oci.rs` (2,542 lines) |
| OCI image pull/store | Pelagos | `image.rs` (730 lines) |
| Build engine (Remfile) | Pelagos | `build.rs` (3,137 lines) |
| Landlock LSM | Pelagos | `landlock.rs` (259 lines) |
| Config + paths | Pelagos | `config.rs`, `paths.rs` |
| OCI pull (auth + layers) | src2 | `cri/image.rs`, `cri/image_store.rs` |
| Pod lifecycle (restarts) | src2 | `scheduler/process.rs` |
| Deployment controller | src2 | `components/compute/deployment.rs` |
| Service ClusterIP proxy | src2 | `netmux/network.rs` |
| NetworkPolicy controller | src2 | `netmux/np_controller.rs` |
| VNet/Subnet/NSG CRDs | src2 | `components/network/*.rs` |
| StorageClass + provisions | src2 | `storage/*.rs` |
| Gossip protocol | src2 | `store/gossip.rs`, `store/ws.rs` |
| Leader election (leases) | src2 | `store/leases.rs` |
| StoreEventHub | src2 | `store/hub.rs` |
| REST API handlers | src2 | `api/server.rs`, `api/handlers/` |
| RBAC (Role/Binding) | src2 | `types/rbac.rs`, `api/auth/` |
| TLS + bootstrap | src2 | `bootstrap/tls.rs`, `bootstrap/admin_sa.rs` |
| Init (PID 1) handler | src2 | `init.rs`, `node.rs` |
| Manifest watcher | src2 | `manifest/watcher.rs` |
| Health probes | src2 | `cri/health.rs`, `cri/probe_runner.rs` |
| Container exec | src2 | `cri/exec.rs`, `cri/exec_protocol.rs` |
| Controller architecture | refactoring plan | `docs/refactoring-plan.md` (sections 2-14) |

---

*Generated: 2026-06-12*
*Sources: Pelagos v0.65.31, z8s src2 (old), z8s src (new), z8s refactoring plan*
