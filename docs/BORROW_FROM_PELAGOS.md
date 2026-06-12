# z8s vs Pelagos — Feature Comparison & Borrow Analysis

Comparison between:
- **Pelagos** (`/home/abb/dev/pelagos`) — mature container runtime, v0.65.31, ~28K LoC tests
- **z8s** (`/home/abb/dev/z8s`) — nascent container orchestrator with 8 crates, most stubs

---

## Legend

- **Full** — complete implementation in pelagos, directly usable by z8s
- **Partial** — exists but needs adaptation
- **Missing** — not in pelagos either (unique z8s concern)
- **N/A** — not relevant across projects

---

## 1. Container Runtime (z8s: `runtime/`, Pelagos: `container.rs`)

Pelagos has a far richer container runtime than z8s's `ContainerSupervisor`. Every single builder
method is a feature z8s could borrow:

| # | Feature | Pelagos | z8s runtime | Notes |
|---|---------|---------|-------------|-------|
| 1.1 | PID namespace | Full | Full | Already shared |
| 1.2 | UTS namespace | Full | Partial | `set_hostname` exists, not well tested |
| 1.3 | Mount namespace | Full | Full | |
| 1.4 | IPC namespace | Full | Partial | |
| 1.5 | NET namespace | Full | Partial | |
| 1.6 | USER namespace | Full | Partial | |
| 1.7 | CGROUP namespace | Full | Missing | |
| 1.8 | pivot_root (vs chroot) | Full | Does z8s use? | Pelagos does pivot_root + MNT_DETACH |
| 1.9 | PTY/interactive session | Full (`pty.rs` + `spawn_interactive`) | Missing | Full `TerminalGuard`, SIGWINCH relay |
| 1.10 | cgroups v2: memory limit | Full | Missing | `with_cgroup_memory()` |
| 1.11 | cgroups v2: CPU shares | Full | Missing | `with_cgroup_cpu_shares()` |
| 1.12 | cgroups v2: CPU quota | Full | Missing | `with_cgroup_cpu_quota()` |
| 1.13 | cgroups v2: PIDs limit | Full | Missing | `with_cgroup_pids_limit()` |
| 1.14 | cgroups v2: memory+swap | Full | Missing | `with_cgroup_memory_swap()` |
| 1.15 | cgroups v2: memory reservation (low) | Full | Missing | |
| 1.16 | cgroups v2: memory swappiness | Full | Missing | |
| 1.17 | cgroups v2: cpuset | Full | Missing | `with_cgroup_cpuset_cpus/mems()` |
| 1.18 | cgroups v2: hugetlb limits | Full | Missing | |
| 1.19 | cgroups v2: blkio weight | Full | Missing | |
| 1.20 | cgroups v2: blkio throttle (BPS/IOPS) | Full | Missing | Per-device read/write |
| 1.21 | cgroups v2: device rules | Full | Missing | v1 only |
| 1.22 | cgroups v2: RDT/MB/LLC (via `resctrl`) | Full | Missing | `with_cgroup_rdt*` (not in old z8s either) |
| 1.23 | cgroups: resource stats query | Full | Missing | `ResourceStats` struct with memory/CPU/PID |
| 1.24 | cgroups v2: I/O throttling | Full | Missing | `io.max` per-device limits |
| 1.25 | Rootless overlay (kernel userxattr) | Full | Missing | Auto-probe: kernel 5.11+? fuse-overlayfs fallback |
| 1.26 | Rootless overlay (fuse-overlayfs) | Full | Missing | `is_fuse_overlayfs_available()` |
| 1.27 | UID/GID mapping | Full | Partial | Pelagos supports multi-range via newuidmap/newgidmap |
| 1.28 | Seccomp default (Docker) | Full (`seccomp.rs`, ~400 lines) | Missing | |
| 1.29 | Seccomp minimal | Full | Missing | ~40 syscalls only |
| 1.30 | Seccomp with io_uring | Full | Missing | Variant of Docker profile |
| 1.31 | Seccomp USER_NOTIF intercept | Full (`notif.rs`, ~473 lines) | Missing | Userspace syscall interception |
| 1.32 | Capability management | Full (bitflags, DEFAULT_CAPS) | Missing | |
| 1.33 | No-new-privileges | Full | Missing | |
| 1.34 | Read-only rootfs | Full | Missing | |
| 1.35 | Masked paths (/proc/kcore, etc.) | Full | Missing | |
| 1.36 | Readonly paths (remount RO) | Full | Missing | |
| 1.37 | rlimits (NOFILE, AS, CPU, etc.) | Full | Missing | |
| 1.38 | Landlock LSM rules | Full (`landlock.rs`, ~259 lines) | Missing | Path-based access control, kernel 5.13+ |
| 1.39 | AppArmor profiles | Full | Missing | `/proc/self/attr/apparmor/exec` |
| 1.40 | SELinux labels | Full | Missing | `/proc/self/attr/exec` |
| 1.41 | Ambient capabilities | Full | Missing | `PR_CAP_AMBIENT_RAISE` |
| 1.42 | OOM score adjustment | Full | Missing | |
| 1.43 | Supplementary groups (additionalGids) | Full | Missing | |
| 1.44 | umask | Full | Missing | |
| 1.45 | Working directory (chroot-relative) | Full | Missing | `with_cwd()` |
| 1.46 | Hostname (sethostname) | Full | Partial | Pelagos does it via UTS ns |
| 1.47 | Sysctl (kernel params) | Full | Missing | `/proc/sys/` writes in pre_exec |
| 1.48 | Device nodes (mknod) | Full | Missing | |
| 1.49 | /dev symlinks | Full | Missing | |
| 1.50 | Mount propagation | Full | Missing | `MS_SHARED/SLAVE/PRIVATE/SLAVE` |
| 1.51 | Propagration-only remounts | Full | Missing | |
| 1.52 | Bind mounts (RW) | Full | Missing | `with_bind_mount()` |
| 1.53 | Bind mounts (RO) | Full | Missing | `with_bind_mount_ro()` |
| 1.54 | tmpfs mounts | Full | Missing | Include size/mode options |
| 1.55 | Kernel mounts (proc/sysfs/devpts) | Full | Missing | OCI-ordered mount list |
| 1.56 | OCI-ordered mount list | Full | Missing | Preserve `/proc/mountinfo` order |
| 1.57 | Overlay filesystem | Full | Missing | Single upper+work |
| 1.58 | Multi-layer overlay (image layers) | Full | Missing | `with_image_layers()` |
| 1.59 | Named volumes | Full | Missing | `Volume::create/open/delete` |
| 1.60 | Container links (--link) | Full | Missing | `/etc/hosts` entries |
| 1.61 | Privileged mode | Full | Missing | All caps, no seccomp, RW /sys |
| 1.62 | Wasm/WASI runtime | Full (`wasm.rs`, ~736 lines) | N/A | Unlikely to need in z8s |
| 1.63 | Wait with stdio capture | Full | Missing | `wait_with_output()` |
| 1.64 | Wait preserving overlay | Full | Missing | For build engine |

**Recommendation**: Port the entire `container.rs` builder + pre_exec pipeline, `seccomp.rs`,
`cgroup.rs`, `pty.rs`, `landlock.rs`, `notif.rs` into z8s's `runtime` crate. The builder
pattern is battle-tested with 84 integration tests.

---

## 2. Networking (z8s: `network/`, Pelagos: `network.rs`, `netlink.rs`, `nfnetlink.rs`, `dns.rs`)

Pelagos networking is the most mature feature:

| # | Feature | Pelagos | z8s network | Notes |
|---|---------|---------|-------------|-------|
| 2.1 | Loopback (N1) | Full | Partial | `bring_up_loopback()` via ioctl |
| 2.2 | Bridge (N2) via named netns | Full | Partial | Pelagos uses named netns (`/run/netns/`) no race condition |
| 2.3 | Named networks with persistence | Full | Missing | `NetworkDef` + `config.json` per network |
| 2.4 | Auto-IPAM with file-locked allocation | Full | Missing | `flock` on per-network `next_ip` file |
| 2.5 | IPv4 subnet carve (from pool) | Full | Missing | `ensure_network()` carves /24 from pool |
| 2.6 | IPv6 ULA dual-stack | Full | Missing | Deterministic `fd00::/8` from FNV-1a of name |
| 2.7 | IPv6 NDP pre-seed | Full | Missing | Prevents first-packet loss |
| 2.8 | NAT (MASQUERADE) via nftables (N3) | Full | Missing | Reference-counted per-network |
| 2.9 | Port forwarding via DNAT (N4) | Full | Missing | nftables PREROUTING |
| 2.10 | Userspace TCP port proxy (localhost) | Full | Missing | Tokio-based for lo traffic |
| 2.11 | Userspace UDP port proxy | Full | Missing | Std threads with recv_from |
| 2.12 | DNS /etc/resolv.conf injection (N5) | Full | Missing | Bind-mount from temp dir |
| 2.13 | DNS search domains | Full | Missing | `search` line in resolv.conf |
| 2.14 | DNS resolver options | Full | Missing | `options` line |
| 2.15 | Pasta rootless networking (N6) | Full | Missing | Full internet via user-mode TAP |
| 2.16 | DNS service discovery (N7, builtin) | Full (`bin/pelagos-dns.rs`) | Missing | A-record server |
| 2.17 | DNS service discovery (dnsmasq backend) | Full | Missing | Production-grade fallback |
| 2.18 | DNS INPUT firewall rules | Full | Missing | nftables + iptables-nft compat |
| 2.19 | Multi-network containers (eth1, eth2) | Full | Missing | `with_additional_network()` |
| 2.20 | Container namespace join | Full | Missing | `NetworkMode::Container(name)` |
| 2.21 | Pod sandbox (shared NET/IPC/UTS) | Full (`sandbox.rs`) | Missing | Pause process + named netns |
| 2.22 | Teardown with retry on EBUSY | Full | Missing | 50 retries, 100ms gaps |
| 2.23 | netns process killer (orphans) | Full | Missing | Walks `/proc/*/ns/net` |
| 2.24 | Veth name collision via FNV-1a | Full | Missing | Deterministic, collision-free |
| 2.25 | Bridge management via netlink | Full (`netlink.rs`) | Missing | Direct `rtnetlink` sockets (no `ip` command) |
| 2.26 | nftables management via nfnetlink | Full (`nfnetlink.rs`) | Missing | Direct nfnetlink (no `nft`/`iptables` CLI) |
| 2.27 | IPv6 default route + forwarding | Full | Missing | |
| 2.28 | Shared-network container IP resolution | Full | Missing | `resolve_container_ip_on_shared_network()` |

**Recommendation**: Port `network.rs` in its entirety. The named-netns approach, IPAM,
dual-stack, and nftables integration are all needed. Consider whether to use `nfnetlink.rs`
(direct kernel calls, no CLI deps) or shell out to `ip`/`nft` like old z8s did.

---

## 3. OCI Image & Build (z8s: z8s_core has ImageConfig; Pelagos: `image.rs`, `build.rs`)

| # | Feature | Pelagos | z8s | Notes |
|---|---------|---------|-----|-------|
| 3.1 | OCI image pull from registry | Full | Missing | `oci-client` based, anonymous auth |
| 3.2 | OCI image push to registry | Full | Missing | |
| 3.3 | OCI image login/logout | Full | Missing | `~/.docker/config.json` |
| 3.4 | OCI image tag | Full | Missing | |
| 3.5 | OCI image save/load (tar archive) | Full | Missing | OCI Image Layout format |
| 3.6 | Content-addressable layer store | Full | Missing | SHA-256 keyed |
| 3.7 | Layer extraction (tar+gzip) | Full | Missing | With whiteout support |
| 3.8 | Multi-layer overlay mount | Full | Missing | `with_image_layers()` |
| 3.9 | Build from Remfile (Dockerfile-like) | Full | Missing | Complete parser + executor |
| 3.10 | Multi-stage builds | Full | Missing | `FROM ... AS` / `COPY --from=` |
| 3.11 | ARG with variable substitution | Full | Missing | `$VAR`/`${VAR}` in Remfile |
| 3.12 | ADD (URL download + archive extraction) | Full | Missing | `.tar.gz/.bz2/.xz` |
| 3.13 | .remignore (gitignore-style) | Full | Missing | Via `ignore` crate |
| 3.14 | Build cache (SHA256 keyed) | Full | Missing | `--no-cache` flag |
| 3.15 | HEALTHCHECK instruction | Full | Missing | `HealthConfig` in image config |
| 3.16 | Wasm OCI layers | Full | Missing | `application/vnd.bytecodealliance.wasm.*` |

**Recommendation**: Port `image.rs` entirely. The layer store, manifest format, build cache,
and Remfile parser are all directly useful for z8s's image management.

---

## 4. Compose / Orchestration (z8s: `controller/`, Pelagos: `compose.rs`, `sexpr.rs`)

| # | Feature | Pelagos | z8s controller | Notes |
|---|---------|---------|----------------|-------|
| 4.1 | S-expression parser | Full (`sexpr.rs`) | Missing | Zero-dependency recursive descent |
| 4.2 | Compose model types | Full (`compose.rs`) | Missing | ServiceSpec, NetworkSpec, VolumeMount |
| 4.3 | Topological sort (Kahn's) | Full | Missing | With cycle detection |
| 4.4 | Depends-on with port/health readiness | Full | Missing | TCP + HTTP + Cmd health checks |
| 4.5 | Supervisor: start services in order | Full | Missing | Log relay, wait for ready |
| 4.6 | Scoped naming (project prefix) | Full | Missing | `{project}-{service}` |
| 4.7 | Graceful shutdown (reverse order) | Full | Missing | SIGTERM → SIGKILL |
| 4.8 | Compose up/down/ps/logs CLI | Full | Missing | |

**Recommendation**: The compose S-expression format is unlikely to replace z8s's
store-based model, but the topological sort, readiness checking, and service lifecycle
patterns are directly applicable.

---

## 5. OCI Runtime Spec (z8s: none; Pelagos: `oci.rs`)

| # | Feature | Pelagos | z8s | Notes |
|---|---------|---------|-----|-------|
| 5.1 | OCI config.json parser | Full | Missing | `OciConfig` with all fields |
| 5.2 | OCI create (fork + shim + sync) | Full | Missing | Double-fork, exec.sock sync |
| 5.3 | OCI start (connect + send byte) | Full | Missing | |
| 5.4 | OCI state (JSON output) | Full | Missing | |
| 5.5 | OCI kill (signal by name/number) | Full | Missing | |
| 5.6 | OCI delete (cleanup state) | Full | Missing | |
| 5.7 | OCI hooks (prestart/poststop) | Full | Missing | |
| 5.8 | OCI annotations | Full | Missing | |
| 5.9 | Console socket (PTY via SCM_RIGHTS) | Full | Missing | |

**Recommendation**: Port `oci.rs` if you want CRI-O / containerd compatibility.
Lower priority for a pure orchestrator.

---

## 6. CLI Infrastructure (Pelagos: `cli/`, 24 modules)

| # | Feature | Pelagos | z8s | Notes |
|---|---------|---------|-----|-------|
| 6.1 | Config file (TOML) | Full (`config.rs`) | Missing | Per-user vs system config |
| 6.2 | Path management (data vs runtime) | Full (`paths.rs`) | Missing | Rootless-aware |
| 6.3 | Install validation (pre-flight check) | Full | Missing | `validate_install()` |
| 6.4 | Container stats (live resource usage) | Full (`cli/stats.rs`) | Missing | |
| 6.5 | Container logs (with follow) | Full | Missing | |
| 6.6 | Container inspect (JSON) | Full | Missing | |
| 6.7 | Container prune (remove all stopped) | Full | Missing | |
| 6.8 | Container cleanup (stale netns/overlay) | Full | Missing | |
| 6.9 | Subscribe (NDJSON state events) | Full | Missing | For TUI clients |
| 6.10 | Output format (table/json) | Full | Missing | |
| 6.11 | System commands (df, prune) | Full | Missing | |
| 6.12 | Health endpoint | Full | Missing | |

**Recommendation**: The stats and logs features are particularly relevant for z8s.

---

## 7. Testing (Pelagos: `tests/`, 28K lines)

| # | Feature | Pelagos | z8s | Notes |
|---|---------|---------|-----|-------|
| 7.1 | Integration tests requiring root | 84 tests (~28K lines) | ~44 runtime + 38 controller + 9 int + 5 e2e | |
| 7.2 | e2e test scripts (bash) | 25+ scripts | None | Usage patterns to follow |
| 7.3 | Rootless integration tests | `test-rootless.sh` | None | |
| 7.4 | Network failure tests | `test-networking-failures.sh` | None | |
| 7.5 | Stress test | `test-stress.sh` | None | |
| 7.6 | CRI test | `test-cri.sh` | None | |
| 7.7 | Wasm e2e test | `test-wasm-e2e.sh` | N/A | |

**Recommendation**: The test patterns in `tests/integration_tests.rs` are excellent
reference for how to test namespaces, seccomp, cgroups, networking, etc. with real
kernel isolation.

---

## 8. Features Unique to Each Project

### Pelagos-only (not needed by z8s)

| Feature | Why not needed |
|---------|---------------|
| Wasm/WASI container runtime | Orchestrates containers, not Wasm modules |
| CRI (Container Runtime Interface) gRPC | z8s has its own API |
| Docker API (dockerd) proxy | z8s has its own API |
| embedded-wasm feature | N/A |
| containerd-shim protocol | Only needed for CRI |

### z8s-only (not in Pelagos)

| Feature | Why absent from Pelagos |
|---------|------------------------|
| Distributed store with watch/events | Pelagos is a single-host runtime |
| Resource CRDs (Pod, Deployment, etc.) | Pelagos manages containers only |
| Scheduler with node/pod abstraction | Pelagos runs containers directly |
| Controller reconcilation loop | Not needed for single-host |
| Gossip/anti-entropy | Not needed for single-host |
| RBAC / auth | Not needed for CLI runtime |
| API server | Not needed for CLI runtime |

---

## 9. Priority Borrow List (ordered by impact)

| Priority | Module | Why |
|----------|--------|-----|
| **P0** | `container.rs` builder + pre_exec | z8s runtime is missing 60+ features; this is the foundation |
| **P0** | `cgroup.rs` | `ResourceStats`, memory/CPU/PID limits are essential for scheduling |
| **P0** | `seccomp.rs` | The Docker profile is the de-facto standard |
| **P1** | `network.rs` + `netlink.rs` + `nfnetlink.rs` | z8s network is a stub compared to full bridge/NAT/DNS/IPv6 |
| **P1** | `image.rs` | OCI image pull/push/layer extraction |
| **P1** | `pty.rs` | Interactive exec into containers |
| **P2** | `dns.rs` | DNS service discovery for pod-to-pod communication |
| **P2** | `sandbox.rs` | Pod sandbox (shared network namespace, pause process) |
| **P2** | `paths.rs` | Rootless-friendly path management pattern |
| **P2** | `config.rs` | TOML config pattern with environment overrides |
| **P3** | `landlock.rs` | Additional security layer |
| **P3** | `notif.rs` | Syscall interception (advanced use cases) |
| **P3** | `oci.rs` | OCI spec compliance (needed for CRI integration) |
| **P3** | `build.rs` | Image build engine (useful if z8s builds images) |
| **P3** | `compose.rs` + `sexpr.rs` | The topo-sort and readiness patterns |

---

## 10. Code Quality Observations

| Metric | Pelagos | z8s |
|--------|---------|-----|
| `#![allow(dead_code)]` in modules | Only in `container.rs` | Multiple modules |
| Integration tests | 84, root-required | 9 controller + 5 e2e (growing) |
| Test scripts | 25+ bash scripts | None |
| Documentation | Comprehensive module-level docs | Sparse |
| Error types | thiserror enums everywhere | Mixed |
| Clippy clean | Yes (claimed) | Yes (verified) |
| Dependencies | ~30 direct deps | ~20 across workspace |
| Lines of code (src/) | ~25,000 | ~8,000 (mostly stubs) |
