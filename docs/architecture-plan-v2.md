# z8s Architecture Plan v2

> **Date:** 2026-05-26  
> **Scope:** Full codebase audit, internet research, test analysis (16 failures), rootless investigation, networking review  
> **Goal:** Fix all 16 test failures cleanly, establish a correct long-term architecture — no hardcoding, no app-specific hacks

---

## 1. The Core Question: Rootless or Root?

### 1.1 Definitive Answer

**z8s needs both, but root is the correct default.** Rootless is a developer convenience, not the primary target. The ROADMAP phrase "Rootless-first, root-capable" should be **revised to "Root-first, rootless-capable"**.

Here is why, grounded in the actual system state:

```
kernel.apparmor_restrict_unprivileged_userns = 1   ← confirmed on this host
kernel.unprivileged_userns_clone               = 1
kernel version: 7.0.0-15-generic (Ubuntu 25.10)
```

The first sysctl is the **smoking gun** for every test failure. Ubuntu 23.10 introduced AppArmor restrictions on unprivileged user namespaces. When this flag is `1`, any process that does `CLONE_NEWUSER` without an explicit AppArmor profile gets a default restrictive policy that blocks:

- `mount()` — returns `EACCES`
- `chroot()` — returns `EPERM`
- `pivot_root()` — fails as a consequence of the above

This is why running z8s as user `abb` produces degraded mode in every container, even though `newuidmap`/`newgidmap` are installed and `/etc/subuid` is configured.

### 1.2 What Each Mode Can Do

| Operation | Root mode | Rootless + AppArmor profile | Rootless, no profile |
|-----------|-----------|----------------------------|----------------------|
| `CLONE_NEWUSER` | Not needed (already root) | ✓ | ✓ |
| `mount MS_PRIVATE on /` | ✓ | ✓ (with profile) | ✗ EACCES |
| `mount MS_BIND` on rootfs | ✓ | ✓ (with profile) | ✗ EACCES |
| `chroot(rootfs)` | ✓ | ✓ (with profile) | ✗ EPERM |
| `pivot_root` | ✓ | ✓ (with profile + code fix) | ✗ fails |
| emptyDir bind-mount | ✓ | ✓ | ✗ |
| Proper OCI container rootfs | ✓ | ✓ | ✗ degraded |
| cgroup v2 direct | ✓ | Needs delegation | ✗ |

**Without the AppArmor profile, rootless z8s is permanently in degraded mode regardless of any code changes.** The `MNT_LOCKED` / MS_PRIVATE-on-/ errors in the log are AppArmor, not a kernel namespace limitation.

### 1.3 Why `pivot_root` Fails (Technical Detail)

Classic `pivot_root` has two requirements:
1. `new_root` must be a mount point (achieved by bind mounting rootfs → rootfs)
2. The new root mount must not be `MS_SHARED` — it must be `MS_PRIVATE` or `MS_SLAVE`

The current code (`child_enter_ns_fork`) tries `MS_PRIVATE` on `/` to satisfy rule 2. This fails because `/` is `MNT_LOCKED` (inherited from the parent user namespace — a kernel security invariant). Even with `MS_PRIVATE` on `/` working, `pivot_root` on the bind mount would still need the bind mount itself to be private.

**The correct fix:** After `mount(rootfs, rootfs, MS_BIND|MS_REC)`, immediately do `mount(NULL, rootfs, MS_PRIVATE|MS_REC)` on the new bind mount. The bind mount is a fresh entry in the mount table — not inherited, not locked — so `MS_PRIVATE` succeeds. Then `pivot_root(rootfs, rootfs/.z8s_old_root)` works.

But this only matters once AppArmor is out of the way. Without the AppArmor profile, the bind mount itself is blocked.

---

## 2. Root Cause of the 16 Test Failures

All failures trace back to one or two root causes.

### 2.1 Failure Map

| Test | Failure | Root cause |
|------|---------|-----------|
| `nginx-hello deployment ready` | readyReplicas=0 after 90s | nginx reads host `/etc/nginx` in degraded mode → exits |
| `alpine emptyDir write/read` | `nonexistent directory /var/data` | bind mount fails in degraded → symlink fallback broken |
| `ubuntu envFrom configmap` | GLIBC_2.43 not found | degraded mode → ubuntu binary runs against host libc |
| `ubuntu envFrom secret` | GLIBC_2.43 not found | same |
| `ubuntu-deploy pod envFrom` | GLIBC_2.43 not found | same |
| `svc: python HTTP server` | no valid response (ClusterIP) | python backend crashes in degraded; proxy up but backend dead |
| `svc: nginx default page` | no valid response (ClusterIP) | nginx backend dead |
| `svc: whoami info page` | no valid response (ClusterIP) | whoami backend dead |
| `svc: http-echo text` | no valid response (ClusterIP) | http-echo backend dead |
| `svc: hostinfo page` | no valid response (ClusterIP) | hostinfo backend dead |
| `svc: nginx-hello page` | no valid response (ClusterIP) | nginx-hello dead |
| `exec: deployment pod exec` | spawn error: No such file | exec enters wrong namespace path in degraded |
| `info: nginx-hello` | pod in Failed phase | same as nginx-hello deployment above |
| `info: whoami` | spawn error: No such file | exec path resolution wrong in degraded |
| `info: http-echo` | spawn error: No such file | exec path resolution wrong in degraded |
| `info: cluster-dashboard` | connection refused | dashboard backend dead |

**Key insight:** The 6 service proxy tests (ClusterIP) are NOT a proxy bug. The `127.96.x.x` proxy binds and listens correctly. The backends simply have no TCP listener because they crashed in degraded mode. Fix the root (run as root or fix AppArmor), and all 6 service tests should flip to green.

### 2.2 Failures That Persist Even with Root

| Failure | Status in root mode |
|---------|---------------------|
| Ubuntu GLIBC_2.43 | **Should pass** — with chroot, container uses ubuntu image's own libc |
| Exec spawn errors | **Needs investigation** — exec handler's `setns` path for isolated containers |

The ubuntu failures are a degraded-mode artifact. With `chroot`, the container process sees the ubuntu image's `/lib/x86_64-linux-gnu/libm.so.6` (which has GLIBC_2.43), not the host's.

---

## 3. Networking Architecture

### 3.1 ClusterIP Proxy (127.96.x.x) — Keep It

The `127.96.x.x` ClusterIP scheme is **correct and elegant**. The entire `127.0.0.0/8` loopback range is routable on any Linux system without iptables, routing tables, or kernel config. Binding a service proxy directly on `127.96.x.x:port` means:
- No iptables rules needed
- No kernel module dependencies
- Works with or without root
- ClusterIPs are stable (not just NodePorts)

**Do not change this.** The 6 service test failures are backends-down, not proxy-wrong.

### 3.2 Current Network Architecture (Implemented)

```
Container (CLONE_NEWNET)
  loopback (127.0.0.1)
  port-published: 127.0.0.1:<host_port> → container:containerPort
     (via port_publish.rs / nsenter + socat-like forwarding)

ClusterIP 127.96.x.x:svcPort → service_proxy.rs → backend connect_port
NodePort  0.0.0.0:3xxxx       → service_proxy.rs → backend connect_port
```

Containers with `containerPort` or a matching Service `targetPort` get `CLONE_NEWNET`. The port publish layer creates a loopback forward from the host into the container's network namespace. The ClusterIP proxy looks up published ports to find backends.

### 3.3 What's Missing: Outbound Connectivity

Containers in `CLONE_NEWNET` can receive connections (via port publish) but cannot make outbound connections. For a container running `curl`, it would fail because there's no route out of the container network namespace.

**Fix: pasta** (Phase 6 per ROADMAP)

Pasta is now the Podman default (Podman 5.8, released March 2026). It:
- Copies the host network config into the container (no NAT)
- Gives containers real outbound connectivity
- Handles port forwarding natively (`-t host_port:container_port`)
- Works rootless (no capabilities needed)
- Outperforms slirp4netns for up to 8 parallel connections (confirmed by benchmarks)

**Integration sequence:**

```
Parent process                          Child (CLONE_NEWNET)
  fork() ──────────────────────────────►  loopback only (no route)
  write UID maps
  exec: pasta --pid <child_pid>           pasta configures tap/eth0:
         -t <host_port>:<container_port>    IP address from host
         -u <udp_ports>                     default routes
                                            DNS config
  pasta exits on namespace deletion ◄──►  eth0 with real connectivity
  write ack byte
                                           chroot(rootfs)
                                           exec(entrypoint)
```

**Install:** `sudo apt install passt` (pasta and passt are the same binary, different argv[0])

### 3.4 DNS

The embedded DNS server (custom, not hickory-dns) is retained. It already handles `<svc>.<ns>.svc.cluster.local → ClusterIP`. Future improvements:
- CNAME for ExternalName services
- AAAA records (IPv6 ClusterIPs)
- Namespace-aware resolution (pods in ns X get correct search domain)

With pasta, containers in CLONE_NEWNET get pasta's DNS forwarding. The z8s embedded DNS listens on `127.0.0.1:53` (or the ClusterIP DNS service address) and is already configured into each container's `resolv.conf`.

---

## 4. Filesystem Isolation Architecture

### 4.1 Current State

```
spawn_container()
  ├── if is_root() → spawn_root_ns_container()
  │     └── fork()
  │           child: child_enter_ns_root(rootfs, volumes, isolate_net)
  │                   unshare(NEWNS | NEWUTS | [NEWNET])
  │                   mount MS_PRIVATE on / ← works as root
  │                   bind_mount_volumes()  ← works as root
  │                   chroot(rootfs)        ← works as root
  │                   mount /proc /sys /tmp /dev
  │                   exec(entrypoint)
  │
  └── else → spawn_userns_container()
        └── fork()
              child: unshare(NEWUSER | NEWNS | NEWUTS | NEWIPC | [NEWNET])
                     write sync byte; wait for UID map ack
                     mount MS_PRIVATE on / ← EACCES (AppArmor)
                     enter_rootfs():
                       mount(rootfs, rootfs, BIND) ← EACCES (AppArmor)
                       pivot_root()                ← fails
                     chroot(rootfs)               ← EPERM (AppArmor)
                     → DEGRADED MODE
```

### 4.2 Target State

```
spawn_container()
  ├── if is_root() → spawn_root_ns_container()
  │     child: unshare(NEWNS | NEWPID | NEWUTS | NEWIPC | [NEWNET])  ← add NEWPID
  │            mount MS_PRIVATE on /
  │            bind_mount_volumes(rootfs, volumes)
  │            pivot_root(rootfs) OR chroot(rootfs)
  │            mount /proc (new procfs) /sys /tmp /dev /run /dev/pts
  │            drop capabilities (keep only what spec declares)
  │            exec(entrypoint)
  │
  └── else → spawn_userns_container() [requires AppArmor profile]
        child: unshare(NEWUSER | NEWNS | NEWPID | NEWUTS | NEWIPC | [NEWNET])
               write sync byte; wait for UID map ack
               mount(rootfs, rootfs, MS_BIND|MS_REC)     ← create new mount
               mount(NULL, rootfs, MS_PRIVATE|MS_REC)    ← set private (new mount = not locked)
               bind_mount_volumes inside rootfs
               bind-mount /proc /sys /dev nodes into rootfs
               pivot_root(rootfs, rootfs/.z8s_old_root)  ← now works!
               umount /.z8s_old_root
               mount /proc (procfs) /tmp /run /dev/pts
               exec(entrypoint)
```

### 4.3 Adding CLONE_NEWPID

The current code explicitly skips `CLONE_NEWPID` because Go runtime threads fail with `EINVAL` in degraded mode (no chroot means PID namespace + Go's thread spawning is broken). This is a degraded-mode-only problem.

In root mode (or rootless with AppArmor + proper chroot), `CLONE_NEWPID` is safe and should be added:

```rust
// In child_enter_ns_root():
let mut flags = CloneFlags::CLONE_NEWNS
    | CloneFlags::CLONE_NEWPID   // ← add
    | CloneFlags::CLONE_NEWUTS
    | CloneFlags::CLONE_NEWIPC;
```

`CLONE_NEWPID` gives containers PID 1 in their own PID namespace. Host processes are invisible from inside. This is a meaningful security improvement.

**Note:** CLONE_NEWPID is already conditionally safe in user namespace mode once degraded mode is eliminated.

---

## 5. Volume Implementation

### 5.1 Current Volume Flow

```
prepare_volumes()           [parent, before fork]
  → resolve_volume_source()
      emptyDir: create ~/.local/share/z8s/emptydir/<pod_uid>-<name>/
      configMap: materialize keys to ~/.local/share/z8s/configmaps/<ns>/<name>/
      secret:    materialize base64-decoded keys to ~/.local/share/z8s/secrets/<ns>/<name>/

child_enter_ns_fork() / child_enter_ns_root()
  → bind_mount_volumes(rootfs, volumes)   [before chroot/pivot_root]
      for each volume:
        src = host path
        dst = rootfs + container_path
        mount(src, dst, MS_BIND|MS_REC)
        if read_only: remount MS_RDONLY
        fallback: symlink (emptyDir) or copy_tree (configmap/secret)
```

### 5.2 Known Bug: emptyDir Missing Target Directory

The failing test: `sh: can't create /var/data/test.txt: nonexistent directory`

**Root cause:** In `bind_mount_volumes()`, the target `rootfs/var/data` is only created if `src.is_dir()`. The emptyDir host path IS a directory, so `create_dir_all(dst_path)` is called. But if `dst_path` already exists as a file or stale symlink from a previous pod incarnation, the bind mount fails silently and falls through to the symlink fallback.

The symlink fallback: `symlink(src_host_path, rootfs/var/data)` — after chroot/pivot_root, `rootfs/var/data` is now a symlink pointing to a host absolute path that doesn't exist inside the container.

**Fix:** Before bind-mounting, always remove any stale entry at the target and recreate as a directory:

```rust
// In bind_mount_volumes(), for emptyDir:
if dst_path.exists() || dst_path.is_symlink() {
    if dst_path.is_dir() {
        std::fs::remove_dir_all(dst_path).ok();
    } else {
        std::fs::remove_file(dst_path).ok();
    }
}
std::fs::create_dir_all(dst_path)?;
// Then bind mount
```

### 5.3 emptyDir Data Persistence Bug

emptyDir data should be ephemeral (cleared on pod delete/recreate). The `cleanup_emptydir()` call correctly removes the host staging dir on pod stop. But if a pod crashes and restarts without going through the full delete path, the old emptyDir data survives.

**Fix:** In `reconcile()`, before spawning a pod, call `scrub_rootfs_volume_mounts()` and `cleanup_emptydir()` to ensure a clean slate.

---

## 6. Exec Implementation

### 6.1 Current Exec Path

The exec handler (`src/server/exec.rs`) does:
1. Looks up the running container by name
2. Gets the container PID and rootfs path
3. Calls `build_command()` which decides the exec method based on isolation state

The problem for isolated containers (in root mode with chroot):
- Container is in a mount namespace (`CLONE_NEWNS`)
- Exec must enter that namespace via `setns(CLONE_NEWNS)` before executing the binary
- The binary path inside the namespace is `/bin/whoami` (in-container path)
- The binary path on the host is `rootfs_path + /bin/whoami`

### 6.2 Current Bug

Looking at `argv_for_isolation()`:
```rust
if isolation == RootfsIsolation::Degraded {
    build_container_argv(entrypoint, args, rootfs_host_path)
} else {
    build_container_argv_in_mount_ns(entrypoint, args, rootfs_host_path)
}
```

For `Chroot` isolation, `build_container_argv_in_mount_ns()` is used — it translates the host path to an in-container path. This is correct. But the exec must also actually **enter the mount namespace** of the container process via `setns(pid_ns_fd, CLONE_NEWNS)`.

If exec enters the mount namespace but uses the wrong binary path, it gets ENOENT. If it doesn't enter the mount namespace but uses the in-container path, it also gets ENOENT (path doesn't exist on host as `/bin/whoami`).

**Fix:** The exec handler must:
1. Open `/proc/<container_pid>/ns/mnt`
2. `setns(mnt_fd, CLONE_NEWNS)`
3. Then `exec(in_container_path)` — e.g. `/bin/whoami`

For isolated-network containers without chroot (current degraded fallback for some cases), keep the host-path exec with dynamic linker wrapping.

The condition should be:
```rust
let enter_mnt_ns = isolation != RootfsIsolation::Degraded;
// Not: isolated_net && fs_isolated (current wrong condition)
```

---

## 7. Security Hardening (Phase 1.5)

### 7.1 Capability Drop

In root mode, after `fork()` in the child and after setting up namespaces, the child still has all root capabilities. These should be dropped before `exec`:

Keep only:
- `CAP_NET_BIND_SERVICE` — if pod declares ports < 1024
- `CAP_CHOWN`, `CAP_SETUID`, `CAP_SETGID` — if container needs `su` internally

Everything else: `prctl(PR_SET_SECUREBITS, ...)` + `cap_set_proc(min_caps)`.

The `securityContext.privileged: true` in pod spec bypasses this (explicit opt-in only).

### 7.2 Landlock

Add after namespace setup, before `exec`, in both root and user-namespace paths:

```rust
// Restrict container to its rootfs only
use landlock::{ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, PathBeneath, PathFd};
let ruleset = Ruleset::default()
    .handle_access(AccessFs::from_all(ABI::V4))?
    .create()?
    .add_rule(PathBeneath::new(PathFd::new(rootfs_path)?, AccessFs::from_all(ABI::V4)))?
    .restrict_self()?;
```

This provides defense-in-depth: even if a container escapes its mount namespace (kernel bug), Landlock prevents host filesystem access. Kernel 5.13+ for filesystem restrictions.

### 7.3 seccomp

Docker's default allowlist as the starting point. Key denials:
- `ptrace`, `kexec_load`, `create_module`, `mount` (if not privileged), `reboot`, `syslog`, `acct`, `settimeofday`

Allow per pod spec when `securityContext.capabilities.add` is set.

---

## 8. AppArmor Profile for z8s

This is the key enabler for rootless mode. The profile must be installed by `install.sh`.

### 8.1 Profile Location

`/etc/apparmor.d/usr.local.bin.z8s` (adjust path to match install location)

### 8.2 Minimum Required Profile

```apparmor
#include <tunables/global>

profile z8s /usr/local/bin/z8s {
  #include <abstractions/base>
  #include <abstractions/nameservice>

  # Capabilities for namespace + container management
  capability sys_admin,       # mount, unshare, setns
  capability sys_chroot,      # chroot(2)
  capability sys_ptrace,      # /proc/<pid>/ access for setns
  capability net_admin,       # network namespace setup
  capability net_bind_service,# bind ports < 1024
  capability setuid,          # uid mapping
  capability setgid,          # gid mapping
  capability dac_override,    # file access in container rootfs
  capability dac_read_search, # same

  # Mount operations — required for container rootfs setup
  mount,
  umount,
  pivot_root,

  # Allow r/w of the data directory
  owner @{HOME}/.local/share/z8s/** rwkl,
  /var/lib/z8s/** rwkl,       # root mode data dir

  # Allow exec of container processes
  /** ix,

  # Allow reading namespace files
  /proc/*/ns/** r,
  /proc/*/uid_map rw,
  /proc/*/gid_map rw,
  /proc/*/setgroups rw,

  # Network
  network,
}
```

### 8.3 Installation

Add to `install.sh`:

```bash
if [ -d /etc/apparmor.d ]; then
    cp etc/apparmor/z8s /etc/apparmor.d/usr.local.bin.z8s
    apparmor_parser -r /etc/apparmor.d/usr.local.bin.z8s
    echo "AppArmor profile installed"
fi
```

---

## 9. Revised Roadmap

### Phase Summary (Updated)

| Phase | Goal | Key additions | Status |
|-------|------|--------------|--------|
| **0** | Foundation fixes | Run as root, fix pivot_root code, emptyDir, exec setns | **Do now** |
| **0.5** | AppArmor profile | Enables rootless mode properly | **Do now** |
| **1** | Namespace isolation | NEWPID in root mode, NEWIPC (done), startup detection | **Next** |
| **1.5** | Security hardening | Landlock, seccomp, capability drop | After Phase 1 |
| **2** | Storage | Volume bind-mount fixes, emptyDir cleanup, PV/PVC | Partially done |
| **3** | Networking | ClusterIP proxy done; DNS improvements | Partially done |
| **4** | pasta | Per-container outbound via pasta | Phase 6 renamed |
| **5** | cgroup v2 | Resource limits, delegation | |
| **6** | Ingress | HTTP/S, WebSocket, TCP/UDP | |
| **7** | Overlayfs | Layer sharing (kernel 7.0 supports in user ns) | |
| **8** | eBPF CNI | veth+bridge, NetworkPolicy | |
| **9** | Multi-node | Primary + agent, gRPC, tonic | |

### Phase 0 — Immediate Code Changes

| Task | File | Change |
|------|------|--------|
| Fix rootless pivot_root | `src/container/rootfs.rs` | Add `MS_PRIVATE` on bind mount inside `enter_rootfs()` |
| Add NEWPID in root mode | `src/container/rootfs.rs` | Add `CLONE_NEWPID` to `child_enter_ns_root()` |
| Fix emptyDir target dir | `src/container/volumes.rs` | Remove stale entry + recreate dir before bind |
| Fix exec namespace entry | `src/server/exec.rs` | Enter mount ns when `isolation != Degraded` (not tied to net isolation) |
| Startup mode warning | `src/main.rs` | Warn when non-root + `apparmor_restrict_unprivileged_userns=1` |
| AppArmor profile | `etc/apparmor/z8s` | New file; `install.sh` installs it |

### Phase 0.5 — Rootless Validation

After AppArmor profile is installed and pivot_root code is fixed:
1. Verify: `sudo apparmor_parser -r /etc/apparmor.d/usr.local.bin.z8s`
2. Run z8s as user `abb` (no sudo)
3. Check log: should show `pivot_root: success` not `filesystem isolation unavailable`
4. Run `./tests/run-network-fixes.sh` — expect same pass rate as root mode

---

## 10. Key Design Decisions (Locked)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Root vs rootless | Root-first, rootless via AppArmor profile | pivot_root needs AppArmor profile in user ns on Ubuntu 23.10+ |
| ClusterIP range | `127.96.x.x` (loopback) | Always routable, no iptables, no kernel config |
| Networking backend | pasta for CLONE_NEWNET containers | Podman default, no NAT, works rootless, correct performance |
| DNS | Custom embedded (keep, improve) | Fewer deps, already works, matches ResourceStore live |
| Overlayfs | Native (kernel 7.0 supports in user ns since 5.11) | No fuse-overlayfs needed |
| NEWPID | Yes for root/AppArmor mode; skip only in degraded | Go runtimes fail with NEWPID only in degraded (no chroot) |
| Ingress | Custom tokio-based (no nginx, no Pingora) | Consistent with z8s lightweight philosophy |
| seccomp | syscallz, Docker default allowlist | Proven, maintained allowlist |
| Landlock | Yes, after namespace setup | Defense in depth against kernel bugs |

---

## 11. What NOT to Do

- **No per-app Rust branches.** No `is_nginx_program()`, no `apply_nginx_args()`, no detecting the image name and applying workarounds. If nginx needs `daemon off`, that goes in the manifest's `args:` field or the OCI image's CMD, not in z8s.
- **No greenwashing tests.** No hardcoded responses, no test skips without a documented reason.
- **No disabling AppArmor globally.** `sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` is a system-wide security regression. The solution is a proper AppArmor profile for z8s.
- **No iptables dependency** for ClusterIP routing. The `127.96.x.x` approach is correct; do not backtrack to `10.96.x.x` with iptables DNAT.
- **No chicken-egg with pasta and NEWNET.** Keep current port-publish NEWNET approach for now. Pasta replaces it in Phase 4.

---

## 12. Verification Checklist

### With `sudo ./target/release/z8s` (root mode, today)

Expected outcomes after running `./tests/run-tests.sh`:

| Category | Expected |
|----------|----------|
| All service proxy tests (6) | PASS — backends listen under chroot |
| nginx-hello deployment | PASS — nginx reads own /etc/nginx |
| ubuntu envFrom (3) | PASS — chroot gives ubuntu its own libc |
| alpine emptyDir | PASS after emptyDir fix |
| exec failures (3) | PASS after exec setns fix |
| cluster-dashboard | PASS if backend starts |
| Total | ~174/174 |

### With rootless `abb` + AppArmor profile

Same expected outcomes as root mode, except:
- cgroup resource limits — degraded (needs systemd delegation config)
- `CLONE_NEWPID` — enabled once pivot_root works

---

## 13. Implementation Order

```
1. cargo build --release
   ./z8s.sh stop
   sudo ./target/release/z8s >> /tmp/z8s.log 2>&1 &
   # Establish root-mode baseline
   ./tests/run-tests.sh 2>&1 | tee /tmp/z8s-root-baseline.txt

2. Fix emptyDir (volumes.rs) + exec setns (exec.rs)
   cargo build --release && sudo kill -HUP $(pgrep z8s)  [or restart]
   ./tests/run-tests.sh  # should be near 174/174

3. Fix enter_rootfs() pivot_root (rootfs.rs): add MS_PRIVATE on bind mount

4. Create AppArmor profile (etc/apparmor/z8s)
   sudo apparmor_parser -r /etc/apparmor.d/z8s
   # Run as non-root and verify no degraded

5. Add CLONE_NEWPID to child_enter_ns_root()

6. Startup detection + warning in main.rs

7. Update install.sh to install AppArmor profile

8. pasta integration (Phase 4)
```

---

*Supersedes `Review & Revised Plan.md`. Primary source: codebase audit 2026-05-26, kernel config `kernel.apparmor_restrict_unprivileged_userns=1`, test output `/tmp/z8s-test-output.txt` (159 pass / 16 fail).*
