# z8s Architecture Plan v2

> **Date:** 2026-05-26 (updated after youki/libcontainer exploration)
> **Scope:** Full codebase audit, internet research, test analysis (16 failures), rootless investigation, networking review, youki deep-dive
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
| `mount MS_SLAVE on /` | ✓ | ✓ (with profile) | ✗ EACCES |
| `mount MS_PRIVATE on parent` | ✓ | ✓ (with profile) | ✗ EACCES |
| `mount MS_BIND` on rootfs | ✓ | ✓ (with profile) | ✗ EACCES |
| `chroot(rootfs)` | ✓ | ✓ (with profile) | ✗ EPERM |
| `pivot_root` | ✓ | ✓ (with profile + code fix) | ✗ fails |
| emptyDir bind-mount | ✓ | ✓ | ✗ |
| Proper OCI container rootfs | ✓ | ✓ | ✗ degraded |
| cgroup v2 direct | ✓ | Needs delegation | ✗ |

**Without the AppArmor profile, rootless z8s is permanently in degraded mode regardless of any code changes.**

---

## 2. Root Cause of the 16 Test Failures

All failures trace back to one root cause: degraded mode from AppArmor blocking mount/chroot in user namespaces.

### 2.1 Failure Map

| Test | Failure | Root cause |
|------|---------|-----------|
| `nginx-hello deployment ready` | readyReplicas=0 after 90s | nginx reads host `/etc/nginx` in degraded → exits |
| `alpine emptyDir write/read` | `nonexistent directory /var/data` | bind mount fails in degraded; symlink fallback broken |
| `ubuntu envFrom configmap` | GLIBC_2.43 not found | degraded → ubuntu binary runs against host libc |
| `ubuntu envFrom secret` | GLIBC_2.43 not found | same |
| `ubuntu-deploy pod envFrom` | GLIBC_2.43 not found | same |
| `svc: python HTTP server` | no valid response (ClusterIP) | backend dead in degraded; proxy up but backend gone |
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

**Key insight:** The 6 service proxy tests are NOT a proxy bug. The `127.96.x.x` proxy binds and listens correctly. Backends crashed in degraded mode. Fix the root → all 6 flip to green.

---

## 3. Networking Architecture

### 3.1 ClusterIP Proxy (127.96.x.x) — Keep It

The `127.96.x.x` ClusterIP scheme is correct. The entire `127.0.0.0/8` loopback range is routable without iptables. Do not change this.

### 3.2 Current Network Architecture

```
Container (CLONE_NEWNET)
  loopback (127.0.0.1)
  port-published: 127.0.0.1:<host_port> → container:containerPort

ClusterIP 127.96.x.x:svcPort → service_proxy.rs → backend connect_port
NodePort  0.0.0.0:3xxxx       → service_proxy.rs → backend connect_port
```

### 3.3 Missing: Outbound Connectivity → pasta (Phase 4)

Containers in `CLONE_NEWNET` cannot make outbound connections. Fix: **pasta** (same binary as passt, Podman default since v5.8).

Integration sequence:
```
Parent                              Child (CLONE_NEWNET)
  fork() ──────────────────────────►  loopback only
  write UID maps
  exec: pasta --pid <child>            tap/eth0 with real IP + routes + DNS
         -t <host_port>:<container>    port forwarding configured
  write ack byte
                                       chroot(rootfs) → exec(entrypoint)
```

Install: `sudo apt install passt`

### 3.4 DNS

The embedded DNS server is retained and improved (namespace awareness, CNAME for ExternalName). With pasta, containers in CLONE_NEWNET get pasta's DNS forwarding, and z8s embedded DNS handles in-cluster service resolution.

---

## 4. Filesystem Isolation — Revised with Youki Insights

### 4.1 The Correct pivot_root Sequence (Learned from Youki)

The biggest concrete insight from reading youki's `rootfs/rootfs.rs` and `rootfs/mount.rs` is that z8s is doing the pivot_root setup in the wrong order with the wrong targets. Youki's sequence is:

```
Step 1: mount(None, "/", None, MS_REC | MS_SLAVE, None)
        ↑ MS_SLAVE (not MS_PRIVATE) on root — prevents propagation
          back to parent but doesn't fail on MNT_LOCKED as hard

Step 2: make_parent_mount_private(rootfs)
        ↑ Parse /proc/self/mountinfo → find the specific parent mount
          that contains the rootfs directory → set ONLY THAT mount
          to MS_PRIVATE. Much more targeted than blindly hitting "/".

Step 3: mount(rootfs, rootfs, None, MS_BIND | MS_REC, None)
        ↑ Bind rootfs onto itself (makes it a mountpoint for pivot_root)

Step 4: <bind volumes into rootfs here>

Step 5: pivot_root(rootfs, rootfs/.z8s_old_root)

Step 6: chdir("/") → umount2("/.z8s_old_root", MNT_DETACH)
```

**Why this is better than z8s's current approach:**

z8s currently tries `MS_PRIVATE` on `/` which fails with `EACCES` because `/` is `MNT_LOCKED` (inherited from parent namespace). The youki approach targets the parent mount of the rootfs directory specifically — which may or may not be locked — and gracefully skips it if it's not shared.

### 4.2 `make_parent_mount_private()` — New Function for z8s

This function parses `/proc/self/mountinfo` to find which mount contains the rootfs directory, checks if it has the `shared:N` peer group field, and only then applies `MS_PRIVATE` to that specific mount point. Code sketch:

```rust
fn make_parent_mount_private(rootfs: &Path) -> Result<()> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")?;
    // Find the most specific mount that is a prefix of rootfs
    let parent_mount = find_parent_mount(rootfs, &mountinfo)?;
    // Only apply MS_PRIVATE if it's currently shared (has "shared:N" field)
    if parent_mount.is_shared {
        mount(None::<&str>, &parent_mount.point, None::<&str>,
              MsFlags::MS_PRIVATE, None::<&str>)?;
    }
    Ok(())
}
```

### 4.3 Updated Spawn Flow

```
spawn_container()
  ├── if is_root() → spawn_root_ns_container()
  │     child: unshare(NEWNS | NEWPID | NEWUTS | NEWIPC | [NEWNET])
  │            mount(None, "/", MS_REC|MS_SLAVE)      ← Step 1
  │            make_parent_mount_private(rootfs)       ← Step 2 (NEW)
  │            bind_mount_volumes(rootfs, volumes)     ← Step 3
  │            mount(rootfs, rootfs, MS_BIND|MS_REC)  ← Step 4
  │            pivot_root OR chroot(rootfs)
  │            mount /proc /sys /tmp /dev /run /dev/pts
  │            drop_capabilities()                    ← Phase 1.5 (NEW)
  │            exec(entrypoint)
  │
  └── else → spawn_userns_container() [needs AppArmor profile]
        child: unshare(NEWUSER | NEWNS | NEWPID | NEWUTS | NEWIPC | [NEWNET])
               write sync byte; wait for UID map ack
               mount(None, "/", MS_REC|MS_SLAVE)      ← Step 1
               make_parent_mount_private(rootfs)       ← Step 2 (NEW)
               mount(rootfs, rootfs, MS_BIND|MS_REC)  ← Step 4
               bind-mount /proc /sys /dev nodes into rootfs
               bind_mount_volumes inside rootfs
               pivot_root(rootfs, .z8s_old_root)
               umount /.z8s_old_root
               mount /proc (procfs) /tmp /run /dev/pts
               drop_capabilities()                    ← Phase 1.5 (NEW)
               exec(entrypoint)
```

### 4.4 Adding CLONE_NEWPID

Safe once chroot/pivot_root works. Add to `child_enter_ns_root()`:

```rust
let mut flags = CloneFlags::CLONE_NEWNS
    | CloneFlags::CLONE_NEWPID   // ← add
    | CloneFlags::CLONE_NEWUTS
    | CloneFlags::CLONE_NEWIPC;
```

Keep omitting it in degraded fallback — Go runtimes still fail there.

---

## 5. Security Hardening — Updated with Youki Insights

### 5.1 Capability Dropping (Phase 1.5)

Youki's `capabilities.rs` uses the `caps` crate and the 5-set Linux capability model (Bounding, Effective, Permitted, Inheritable, Ambient). The pattern is clean and directly adoptable.

**Crate to add:** `caps = "0.5"` (same as youki)

**Default capability set for unprivileged containers** (from youki's test fixtures):
```
CAP_AUDIT_WRITE    — write audit log records
CAP_KILL           — send signals to other processes
CAP_NET_BIND_SERVICE — bind ports < 1024
```

Drop everything else from all 5 sets. Pattern:

```rust
// In child, after setuid/setgid, before exec:
fn drop_capabilities(privileged: bool, extra_caps: &[caps::Capability]) {
    if privileged { return; }

    let mut keep = caps::CapsHashSet::new();
    keep.insert(caps::Capability::CAP_AUDIT_WRITE);
    keep.insert(caps::Capability::CAP_KILL);
    keep.insert(caps::Capability::CAP_NET_BIND_SERVICE);
    for cap in extra_caps { keep.insert(*cap); }

    for set in [CapSet::Bounding, CapSet::Effective,
                CapSet::Permitted, CapSet::Inheritable, CapSet::Ambient] {
        caps::set(None, set, &keep).ok();
    }
}
```

`securityContext.privileged: true` in pod spec skips this entirely.
`securityContext.capabilities.add` in pod spec extends the `keep` set.

**Ref:** `refs/youki/crates/libcontainer/src/capabilities.rs` lines 134-164

### 5.2 Seccomp (Phase 1.5)

Youki's default seccomp profile has been copied to `etc/seccomp/default.json` (832 lines). It uses:
- **Default action:** `SCMP_ACT_ERRNO` — deny everything not explicitly allowed
- **Architecture support:** x86_64, aarch64, mips64, riscv64
- **Allowlist:** ~310 syscalls (same baseline as Docker/runc)

**Crate to add:** `syscallz = "0.17"` (lighter than libseccomp-sys, no C dependency)

**Implementation:**

```rust
fn apply_seccomp(privileged: bool) -> Result<()> {
    if privileged { return Ok(()); }

    let profile: SeccompProfile = serde_json::from_str(
        include_str!("../../etc/seccomp/default.json")
    )?;

    let mut ctx = syscallz::Context::init_with_action(
        syscallz::Action::Errno(libc::EPERM as u32)
    )?;

    for rule in &profile.syscalls {
        for name in &rule.names {
            if let Ok(nr) = syscallz::Syscall::from_name(name) {
                ctx.set_action_for_syscall(syscallz::Action::Allow, nr)?;
            }
        }
    }
    ctx.load()?;
    Ok(())
}
```

Apply after Landlock, before `exec`. `securityContext.privileged: true` skips it.

**File:** `etc/seccomp/default.json` — copied from youki, covers all standard container workloads.

### 5.3 Landlock (Phase 1.5)

After namespace setup, before exec, restrict the container to its rootfs:

```rust
use landlock::{ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr};

fn apply_landlock(rootfs: &str) -> Result<()> {
    Ruleset::default()
        .handle_access(AccessFs::from_all(ABI::V4))?
        .create()?
        .add_rule(PathBeneath::new(PathFd::new(rootfs)?, AccessFs::from_all(ABI::V4)))?
        .restrict_self()?;
    Ok(())
}
```

Kernel 5.13+ for filesystem restrictions. Detect at runtime with `Ruleset::default()` — if it fails, log a warning and continue (graceful degradation on older kernels).

**Crate:** `landlock = "0.4"`

### 5.4 Execution Order in Child

```
unshare namespaces
  └─ (if userns) wait for UID map ack
mount MS_SLAVE on /
make_parent_mount_private(rootfs)
bind volumes + devices into rootfs
pivot_root or chroot
mount /proc /sys /tmp /dev /run /dev/pts
setgid(run_as_group)
setuid(run_as_user)
apply_landlock(rootfs)    ← Phase 1.5
apply_seccomp(privileged) ← Phase 1.5
drop_capabilities(...)    ← Phase 1.5
exec(entrypoint)
```

---

## 6. Volume Implementation

### 6.1 emptyDir Bug Fix

Failing test: `sh: can't create /var/data/test.txt: nonexistent directory`

Before bind mounting in `bind_mount_volumes()`, remove any stale entry at the target and always recreate:

```rust
// For each volume before bind:
if dst_path.exists() || dst_path.is_symlink() {
    if dst_path.is_dir() { std::fs::remove_dir_all(dst_path).ok(); }
    else { std::fs::remove_file(dst_path).ok(); }
}
std::fs::create_dir_all(dst_path)?;
```

### 6.2 emptyDir Persistence Fix

Before spawning any pod in `reconcile()`, call `cleanup_emptydir()` to ensure a clean slate on restart.

---

## 7. Exec Implementation Fix

### 7.1 Mount Namespace Entry

The exec handler must enter the container's mount namespace via `setns(CLONE_NEWNS)` whenever `isolation != Degraded`. The current condition is wrong (`isolated_net && fs_isolated`).

Fix:
```rust
let enter_mnt_ns = isolation != RootfsIsolation::Degraded;
```

Then exec uses in-container paths (e.g., `/bin/whoami`), not host paths.

---

## 8. AppArmor Profile for z8s

This is the key enabler for rootless mode. Must be installed by `install.sh`.

### 8.1 Profile

`/etc/apparmor.d/usr.local.bin.z8s`:

```apparmor
#include <tunables/global>

profile z8s /usr/local/bin/z8s {
  #include <abstractions/base>
  #include <abstractions/nameservice>

  capability sys_admin,
  capability sys_chroot,
  capability sys_ptrace,
  capability net_admin,
  capability net_bind_service,
  capability setuid,
  capability setgid,
  capability dac_override,
  capability dac_read_search,

  mount,
  umount,
  pivot_root,

  owner @{HOME}/.local/share/z8s/** rwkl,
  /var/lib/z8s/** rwkl,

  /** ix,

  /proc/*/ns/** r,
  /proc/*/uid_map rw,
  /proc/*/gid_map rw,
  /proc/*/setgroups rw,
  /proc/self/mountinfo r,    ← needed for make_parent_mount_private()

  network,
}
```

### 8.2 Installation

Add to `install.sh`:
```bash
if [ -d /etc/apparmor.d ]; then
    cp etc/apparmor/z8s /etc/apparmor.d/usr.local.bin.z8s
    apparmor_parser -r /etc/apparmor.d/usr.local.bin.z8s
fi
```

---

## 9. What We Learned from Youki — Decision Table

| Area | Youki approach | z8s current | Decision | Action |
|------|---------------|-------------|----------|--------|
| **pivot_root setup** | MS_SLAVE on `/` + `make_parent_mount_private(rootfs)` | MS_PRIVATE on `/` (fails, locked) | **ADOPT** | Rewrite `enter_rootfs()` |
| **Parent mount detection** | Parse `/proc/self/mountinfo`, find specific parent, set private | Not done | **ADOPT** | New `make_parent_mount_private()` fn |
| **Namespace ordering** | Sequential unshare per namespace, NEWUSER always first | Single combined `unshare()` | **KEEP** | z8s's single call is simpler and fine |
| **UID/GID mapping** | newuidmap → single write | newuidmap → subuid direct → single | **KEEP** | z8s's fallback chain is superior |
| **Capabilities** | `caps` crate, 5-set model, from OCI spec | None (inherits all root caps) | **ADOPT Phase 1.5** | Add `caps = "0.5"`, implement `drop_capabilities()` |
| **Seccomp** | `libseccomp`, loads from OCI spec, no default built-in | None | **ADOPT Phase 1.5** | Use `syscallz`, use copied `etc/seccomp/default.json` |
| **Landlock** | Not in libcontainer yet | None | **IMPLEMENT** | Use `landlock` crate directly |
| **Fork/channel model** | 3-tier (main → intermediate → init) for runc compat | 2-tier (parent → child) with byte-pipe sync | **KEEP** | 2-tier is correct for z8s's architecture |
| **Device setup in userns** | `bind_dev()`: create placeholder → bind mount | Pre-create placeholders → bind mount | **KEEP** | z8s's approach is equivalent and cleaner |
| **Mount into container** | Complex fd-based mount via `open_tree`/`move_mount` | Simple `nix::mount()` | **KEEP** | overkill for z8s's scope |
| **3-tier process** | Needed for youki's runc-compat lifecycle | Not needed | **SKIP** | z8s uses in-process state |

### Files Copied from Youki

| Source | Destination | Use |
|--------|-------------|-----|
| `experiment/seccomp/tests/fixtures/default.json` | `etc/seccomp/default.json` | Default syscall allowlist for Phase 1.5 seccomp |

### Files Referenced but Not Copied

| File | Lines | What to borrow |
|------|-------|---------------|
| `crates/libcontainer/src/rootfs/mount.rs` | 544-565 | `make_parent_mount_private()` algorithm |
| `crates/libcontainer/src/rootfs/rootfs.rs` | 41-80 | `mount_to_rootfs()` sequence (MS_SLAVE → parent private → bind → pivot) |
| `crates/libcontainer/src/capabilities.rs` | 134-164 | `drop_privileges()` pattern and `caps` crate usage |
| `crates/libcontainer/src/seccomp/mod.rs` | 143-250 | Seccomp context setup pattern |

---

## 10. Revised Implementation Priority

### Phase 0 — Immediate (Fix the 16 failures)

| Task | File | What changes |
|------|------|-------------|
| **Run as root** | — | `sudo ./target/release/z8s`; establishes clean baseline |
| **Fix `enter_rootfs()`** | `src/container/rootfs.rs` | Replace MS_PRIVATE-on-/ with MS_SLAVE-on-/ + `make_parent_mount_private(rootfs)` |
| **Add `make_parent_mount_private()`** | `src/container/rootfs.rs` | New fn: parse `/proc/self/mountinfo`, set MS_PRIVATE on parent mount if shared |
| **Fix emptyDir target dir** | `src/container/volumes.rs` | Remove stale entry + recreate dir before bind |
| **Fix exec setns condition** | `src/server/exec.rs` | Enter mount ns when `isolation != Degraded` |
| **Add CLONE_NEWPID root mode** | `src/container/rootfs.rs` | Add to `child_enter_ns_root()` flags |
| **Startup warning** | `src/main.rs` | Warn when non-root + AppArmor restriction detected |

### Phase 0.5 — Rootless Enablement

| Task | File | What changes |
|------|------|-------------|
| **AppArmor profile** | `etc/apparmor/z8s` (new) | Create profile file |
| **install.sh** | `install.sh` | Install + reload AppArmor profile |
| **Validate rootless** | — | Run tests as `abb` after profile install |

### Phase 1.5 — Security Hardening

| Task | Crate | What changes |
|------|-------|-------------|
| **Capability drop** | `caps = "0.5"` | `drop_capabilities()` in child after setuid; default set = [AUDIT_WRITE, KILL, NET_BIND_SERVICE]; extend via `securityContext.capabilities.add` |
| **Seccomp** | `syscallz = "0.17"` | Load `etc/seccomp/default.json` at compile time via `include_str!`; apply after Landlock before exec |
| **Landlock** | `landlock = "0.4"` | Restrict container to rootfs; graceful degradation if kernel < 5.13 |

### Phase 4 — Networking (pasta)

| Task | What |
|------|------|
| pasta integration | Replace port-publish-only NEWNET with pasta for outbound connectivity |
| DNS improvements | CNAME, AAAA, namespace-aware resolution |

### Later Phases

| Phase | Goal |
|-------|------|
| 5 | cgroup v2 delegation + resource limits |
| 6 | Ingress controller (HTTP/S, WebSocket, TCP/UDP) |
| 7 | Overlayfs layer sharing |
| 8 | eBPF CNI + NetworkPolicy |
| 9 | Multi-node (primary + agent, gRPC) |

---

## 11. Key Design Decisions (Locked)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Root vs rootless | Root-first; rootless via AppArmor profile | pivot_root blocked by AppArmor in user ns on Ubuntu 23.10+ without profile |
| pivot_root technique | MS_SLAVE on `/` + `make_parent_mount_private(rootfs)` | From youki; more targeted than MS_PRIVATE on locked `/` |
| ClusterIP range | `127.96.x.x` (loopback) | Always routable, no iptables, no kernel config needed |
| Networking backend | pasta for CLONE_NEWNET (Phase 4) | Podman default since v5.8, no NAT, works rootless |
| DNS | Custom embedded (keep, improve) | Fewer deps, already works, live ResourceStore view |
| Capability drop | `caps` crate, 5-set model | Same approach as youki; well-tested, OCI-aligned |
| Seccomp | `syscallz` + copied `default.json` | Lighter than libseccomp-sys; defaults from youki's proven allowlist |
| Landlock | `landlock` crate, rootfs-only rule | Defense in depth; graceful on older kernels |
| Fork model | 2-tier (parent → child) with byte-pipe sync | Correct for z8s's in-process control plane; don't adopt youki's 3-tier |
| Overlayfs | Native (kernel 7.0; user ns supported since 5.11) | No fuse-overlayfs needed |
| NEWPID | Root/profile mode only; skip in degraded | Go runtimes fail in degraded without chroot |

---

## 12. What NOT to Do

- **No per-app Rust branches.** No `is_nginx_program()`, no argv injection for specific images.
- **No greenwashing tests.** No hardcoded responses, no skip without documented reason.
- **No MS_PRIVATE on `/` attempts.** That path is `MNT_LOCKED` in user ns; youki confirmed the correct alternative.
- **No disabling AppArmor globally.** Use the profile.
- **No iptables dependency** for ClusterIP routing. `127.96.x.x` is correct.
- **No adopting youki's 3-tier fork model.** z8s's 2-tier is architecturally correct.

---

## 13. File Map

| File | Role |
|------|------|
| `src/container/rootfs.rs` | Namespace setup, pivot_root, `make_parent_mount_private()` (new), exec path resolution |
| `src/container/volumes.rs` | Volume resolution and bind mounts; emptyDir fix |
| `src/supervisor/process.rs` | Fork/spawn/reconcile; NEWPID in root mode |
| `src/server/exec.rs` | kubectl exec handler; setns fix |
| `src/main.rs` | Startup AppArmor detection warning |
| `etc/apparmor/z8s` | AppArmor profile for rootless mode (new) |
| `etc/seccomp/default.json` | Seccomp syscall allowlist copied from youki (in place) |
| `refs/youki/` | Reference codebase; do not import as dependency |

---

## 14. Verification Checklist

### With `sudo ./target/release/z8s` (after Phase 0 fixes)

| Category | Expected |
|----------|----------|
| All service proxy tests (6) | PASS — backends listen under chroot |
| nginx-hello deployment | PASS — nginx reads own /etc/nginx |
| ubuntu envFrom (3) | PASS — chroot gives ubuntu its own libc |
| alpine emptyDir | PASS after emptyDir fix |
| exec failures (3) | PASS after exec setns fix |
| cluster-dashboard | PASS if backend starts |
| **Total** | **~174/174** |

### With rootless `abb` + AppArmor profile (after Phase 0.5)

Same expected pass rate as root mode.

---

*Supersedes previous `Review & Revised Plan.md`. Updated after deep-read of `refs/youki/crates/libcontainer/src/` — pivot_root sequence, capabilities, seccomp defaults, device setup. Primary sources: youki `rootfs/rootfs.rs` lines 41-80, `rootfs/mount.rs` lines 544-565, `capabilities.rs` lines 134-164, `experiment/seccomp/tests/fixtures/default.json`.*
