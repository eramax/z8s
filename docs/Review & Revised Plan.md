# z8s Architecture Review & Revised Plan

I've read the ROADMAP, FINDINGS, test results (159 pass / 15 fail), and all source files. I've also researched current best practices for rootless containers, networking (pasta), overlayfs, and user namespace limitations. Here's the complete picture.

---

## The Core Question: Rootless vs Root?

**Answer:** z8s needs to support **both** modes properly. The architecture should be "root-first, rootless-capable" — not "rootless-first."

Here's why, based on the research and your actual codebase:

### What rootless (running as user abb) can do:

- User namespace isolation (`CLONE_NEWUSER`) — works
- Mount namespace (`CLONE_NEWNS`) — works
- UTS, IPC namespaces — works
- UID/GID mapping via `/etc/subuid` — works (you have `abb:165536:65536`)
- Overlayfs inside user namespace — works on kernel 5.11+ (your kernel is 7.0, so yes)
- `chroot()` inside user namespace — works when the mount ns is owned by your user namespace

### What rootless CANNOT do:

- **`pivot_root()`** — fails with `EINVAL` due to `MNT_LOCKED` on inherited mounts. The kernel marks mounts from the parent namespace as `MNT_LOCKED`, preventing `pivot_root` from working on them. This is a fundamental kernel restriction.
- There **is** a new `MOVE_MOUNT_BENEATH` approach (kernel 5.12+) that works around this, but it requires the `open_tree` + `move_mount` syscall dance instead of classic `pivot_root`.
- `mount MS_PRIVATE` on the root mount — fails with `EACCES` in user ns (this is exactly what FINDINGS documents)

### What running as root enables:

- True `pivot_root` (or `chroot`) — full filesystem isolation
- All namespace flags without restriction
- Cgroups v2 resource limits without delegation complexity
- Network namespace setup with real bridge/veth (CNI)
- Bind mounts work everywhere

**Your system:** Ubuntu 25.10, kernel 7.0.0, uidmap installed, subuid/subgid configured. No pasta, no slirp4netns, no fuse-overlayfs installed.

---

## Revised Verdict on Runtime Mode

| Mode | When | FS Isolation | Network | cgroups | Verdict |
|------|------|-------------|---------|---------|---------|
| **Root** (`sudo z8s`) | Production (PID 1), dev testing | `pivot_root` or `chroot` | Full (CNI, pasta, veth) | Direct | **Primary target** |
| **Rootless** (`z8s` as user) | Development, CI | `chroot` in user ns (works), overlayfs | pasta or host-network | systemd delegation | Supported but limited |

The FINDINGS doc got it right: the primary dev blocker is filesystem isolation, and running as root fixes it. But the code should still handle rootless gracefully with clear degraded-mode warnings.

---

## Root Cause Analysis of the 15 Test Failures

The 15 failures cluster into 4 categories:

### Category 1: Degraded FS isolation (backends not listening) — 8 failures

- `nginx-hello` deployment never ready (degraded mode, wrong `/etc/nginx`, daemon exits)
- All ClusterIP service tests fail: `python-svc`, `nginx-svc`, `whoami-svc`, `http-echo-svc`, `hostinfo-svc`, `nginx-hello-svc`, `cluster-dashboard` — proxy binds fine but backends crash in degraded mode

**Root cause:** Running as non-root `abb` → `mount MS_PRIVATE EACCES` → `pivot_root` fails → `chroot` fails → degraded → daemons read host `/etc` → crash/exit

### Category 2: Exec failures (no filesystem isolation in exec path) — 3 failures

- `whoami`, `http-echo`, `deployment pod exec`: spawn error: No such file or directory

**Root cause:** `build_command()` only enters mount namespace if `isolated_net` **AND** `fs_isolated`. For degraded containers, exec falls back to dynamic linker wrapper, which fails for Go static binaries (no `PT_INTERP`)

### Category 3: Ubuntu GLIBC mismatch — 3 failures

- `ubuntu envFrom` configmap/secret/deploy: `GLIBC_2.43 not found`

**Root cause:** The Ubuntu image needs glibc 2.43 but the host has an older version. In degraded mode, binaries are executed via host paths and hit the host's glibc.

### Category 4: Volume failure — 1 failure

- `alpine emptyDir` write: `can't create /var/data/test.txt: nonexistent directory`

**Root cause:** In degraded mode, bind mounts fail. The symlink fallback points to a path that doesn't exist inside the container's view. Also, `stage_volumes_in_rootfs()` creates symlinks that may not resolve correctly after `pivot_root`/`chroot`.

---

## What Needs to Change in the Roadmap

The current ROADMAP is mostly correct but has some issues based on what the previous AI agent actually implemented:

- **Phase 1 is NOT done** — It says "pivot_root" works but it fails in dev. Need a two-track approach.
- **Phase 1.5 (NEWPID)** — Currently intentionally skipped because Go runtimes fail in degraded mode. This is actually correct for degraded mode. Once root-mode chroot/pivot works, NEWPID should be enabled for root mode.
- **Phase 6 networking** — A partial implementation exists (`CLONE_NEWNET` + `port_publish` + `service_proxy` + DNS) but it's loopback-only, no outbound connectivity. Need pasta for real networking.
- **Phase 3 (hickory-dns)** — A custom DNS server was built instead. It works but lacks SRV, CNAME, AAAA, namespace-aware resolution. Should be kept and improved rather than replaced with hickory-dns (less dependency bloat).

---

## Revised Roadmap: The Priority Order

### Priority 0: Fix the Foundation (Do This First)

**Goal:** Make root-mode work properly, fix the degraded mode path, eliminate test cheating.

| Task | What | Why |
|------|------|-----|
| **0a** | Root-mode `pivot_root` path | Verify `spawn_root_ns_container()` actually uses `pivot_root` (not just `chroot`). Currently even root mode uses `chroot`. Implement proper `pivot_root` for root mode. |
| **0b** | Rootless `chroot` path | Inside a user namespace, `chroot()` should work if the mount namespace is owned by the user namespace. Fix the fallback chain: try `pivot_root` → `chroot` → degraded. The `chroot` path may actually work if the rootfs is a bind mount created inside the user namespace. |
| **0c** | Fix exec namespace entry | Remove the `isolated_net` **AND** `fs_isolated` requirement. Exec should enter mount namespace whenever the container has FS isolation, regardless of network state. |
| **0d** | Install passt package | `sudo apt install passt` — needed for pasta networking |
| **0e** | EmptyDir fix | Create the directory inside the rootfs before bind mounting, not just at the host path. Fix the symlink fallback to use relative paths. |
| **0f** | Startup detection | Detect root vs non-root at startup. When non-root, print clear warning about degraded mode limitations. Add `--allow-degraded` flag to suppress the warning. |

### Priority 1: Real Networking (pasta + outbound connectivity)

**Goal:** Containers in network namespaces get real outbound connectivity, not just loopback.

| Task | What |
|------|------|
| **1a** | Integrate pasta |
| **1b** | Port forwarding via pasta |
| **1c** | Host-network mode |
| **1d** | DNS inside netns |

The flow for isolated networking:

```
parent                          child (CLONE_NEWNET)
  fork() ──────────────────────►  loopback only
  write UID maps
  launch: pasta --pid <child> -t 8080:80
  pasta sets up tap/eth0 ──────►  eth0 with IP + routes + DNS
  write ack byte
                                   chroot(rootfs)
                                   exec(entrypoint)
```

For host-network mode (default):

```
parent                          child (no CLONE_NEWNET)
  fork() ──────────────────────►  shares host network
  write UID maps
  write ack byte
                                   chroot(rootfs)
                                   exec(entrypoint)
```

### Priority 2: Complete Namespace Isolation

| Task | What |
|------|------|
| **2a** | Add `CLONE_NEWPID` for root mode |
| **2b** | Add `CLONE_NEWIPC` for root mode |
| **2c** | Consolidate spawn paths |
| **2d** | `ContainerSpawnCtx` |

### Priority 3: Overlayfs Image Layers

| Task | What |
|------|------|
| **3a** | Native overlayfs |
| **3b** | Layer sharing |
| **3c** | Fallback: full copy |

### Priority 4: Security Hardening

| Task | What |
|------|------|
| **4a** | Landlock |
| **4b** | seccomp-bpf |
| **4c** | Capability drop |

### Priority 5: Service Proxy Improvements

| Task | What |
|------|------|
| **5a** | DNS namespace awareness |
| **5b** | Health check probes |
| **5c** | Rolling updates |

---

## The Ubuntu/GLIBC Issue

This is a real environment mismatch, not a z8s bug. The Ubuntu 25.10 image requires glibc 2.43 but the host (also Ubuntu 25.10, kernel 7.0) might have a different glibc. When running as root with proper `chroot`, the container uses its own glibc and this problem goes away.

**Fix:** run as root. No code changes needed for this specific issue.

---

## Immediate Action Items (What to do right now)

1. Install pasta: `sudo apt install passt`
2. Build and run as root:
   ```
   cargo build --release
   ./z8s.sh stop
   sudo ./target/release/z8s >> /tmp/z8s.log 2>&1 &
   ```
3. Re-run tests as root to establish a new baseline
4. Fix Priority 0 items (chroot path, exec namespace entry, emptyDir, startup detection)
5. Integrate pasta for real networking (Priority 1)

This should bring the test pass rate from **159/174** to near **174/174**. The remaining failures would be legitimate missing features (overlayfs, Landlock, etc.) rather than "degraded mode" workarounds.

---

## Summary of Key Architecture Decisions

| Decision | Recommendation | Rationale |
|----------|---------------|-----------|
| Root vs rootless? | Root for production, rootless as fallback | `pivot_root` needs root; `chroot` works in user ns but is weaker. Both paths needed. |
| Networking approach? | pasta for isolated netns, host-network as default | Pasta is the Podman standard, no NAT, works rootless, lower latency than slirp4netns. |
| `pivot_root` in user ns? | Not via classic `pivot_root(2)` | `MNT_LOCKED` blocks it. Use `chroot()` in user ns, or `MOVE_MOUNT_BENEATH` (kernel 5.12+) for a modern alternative. |
| Custom DNS or hickory-dns? | Keep custom DNS, improve it | Already works, fewer dependencies, smaller binary. Add namespace awareness and CNAME support. |
| Overlayfs? | Native overlayfs (kernel 7.0) | Works in user namespaces since 5.11. No fuse-overlayfs needed on this system. |
| NEWPID? | Yes for root mode, skip for degraded | Go runtimes need working chroot to handle NEWPID. Only skip in degraded mode. |
