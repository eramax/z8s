# Container Isolation Hardening Plan

## Problem

Containers (e.g., `dep-hub` in the hub-spoke test) can see all host processes in `/proc`, kill arbitrary processes on the host, bypass file permission checks, and — when no `containerPort` is declared — share the host network namespace.

Three root causes:

| Gap | Location | Why |
|---|---|---|
| **No PID namespace** | `src/cri/rootfs.rs:533` | `CLONE_NEWPID` commented out — nginx got `ENOMEM`, Go runtimes got `EINVAL` |
| **Capabilities too broad** | `src/cri/rootfs.rs:842-860` | `CAP_KILL`, `CAP_DAC_OVERRIDE`, `CAP_NET_RAW`, `CAP_MKNOD` retained by default |
| **Network isolation opt-in** | `src/components/compute/spec_builder.rs:63` | `isolated_net = !declared_ports.is_empty()` — no ports → host netns |

## Changes Needed

### 1. PID Namespace via Double-Fork

**Files**: `src/cri/runtime.rs`, `src/cri/rootfs.rs`

#### Current flow (`spawn_root_ns_container`):
```
fork() → child
         child: unshare(NS | UTS | IPC [+ NET]), sethostname,
                pivot_root/chroot, mount_filesystems,
                sync/ack(veth), child_setup_privileges, execvpe()
parent:   cgroup, veth, track child PID
```

#### New flow (double-fork for `CLONE_NEWPID`):
```
fork() → intermediate child
         intermediate: unshare(NEWPID | NS | UTS | IPC [+ NET])
         ↓
         fork() → grandchild (PID 1 in new PID namespace)
                  grandchild: setsid(), sethostname(), loopback,
                              sync/ack(veth), pivot_root/chroot,
                              mount fresh /proc, child_setup_privileges,
                              execvpe()
         intermediate: write grandchild PID to pipe, exit(0) → reaped by subreaper

parent:   read grandchild PID from pipe
          cgroup_add_pid(grandchild_pid)
          handle_veth_netns(grandchild_pid) ← enter grandchild's netns
          track grandchild PID in ContainerInstance
```

#### Why double-fork?
`unshare(CLONE_NEWPID)` does NOT move the caller — only *future* children. So the child must `unshare(NEWPID)` then `fork()` again. The grandchild is the first process in the new PID namespace (PID 1).

#### Supporting change — `PR_SET_CHILD_SUBREAPER` (`src/init.rs`):
Without this, the grandchild (orphaned when the intermediate child exits) gets reparented to systemd, and z8s can't `waitpid` it. Adding `prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0)` to `InitHandler::new()` makes z8s the reaper for orphaned descendants. Now `ProcessTracker::reap_zombies` (which calls `waitpid(-1, WNOHANG)`) sees the grandchild exit directly.

#### Refactoring needed in `child_enter_ns_root`:
Split into two phases so the intermediate child (phase 1) and grandchild (phase 2) can each call the right part:

```
Phase 1 (intermediate child):
  unshare_namespaces(flags, hostname)    // unshare + sethostname

Phase 2 (grandchild):
  setup_container_rootfs(rootfs, volumes, is_root)   // pivot_root/chroot + mount_filesystems
```

A new function `unshare_namespaces()` extracts the unshare+sethostname logic. `child_enter_ns_root` is kept for backward compat but calls both phases. The grandchild path calls phase 2 only (namespaces already inherited).

### 2. Default Network Namespace Isolation

**File**: `src/components/compute/spec_builder.rs:63`

Change:
```rust
// BEFORE:
let isolated_net = !declared_ports.is_empty();

// AFTER:
let isolated_net = !host_network;
```

Where `host_network` is read from `pod.spec.host_network.unwrap_or(false)`.

The `ContainerConfig` already has `isolated_net: bool`, so the rest of the pipeline handles netns creation automatically.

### 3. Capability Hardening

**File**: `src/cri/rootfs.rs:842-860`

Remove from `keep` set:
| Capability | Risk |
|---|---|
| `CAP_KILL` | Kill any host process (no PID ns barrier) |
| `CAP_DAC_OVERRIDE` | Bypass file permission checks inside rootfs |
| `CAP_NET_RAW` | Raw sockets, packet sniffing from inside netns |
| `CAP_MKNOD` | Create device nodes inside container |

These are only removed from the *default* set. `securityContext.capabilities.add` in the Pod spec still works — users can opt back in with `["KILL", "DAC_OVERRIDE", "NET_RAW", "MKNOD"]`.

### 4. Signal Delivery for PID-Namespaced Containers

**File**: `src/cri/runtime.rs:948-963`

When a container process is PID 1 in its own PID namespace, the kernel blocks signals (except `SIGKILL`/`SIGSTOP`) sent from ancestor namespaces unless the process registered a handler. Most container entrypoints don't.

Current `stop_container` sends `SIGTERM` → `SIGKILL` to the process group. The `SIGTERM` to PID 1 will be silently dropped. The `SIGKILL` fallback works.

**Change**: Skip `SIGTERM` and send `SIGKILL` directly for PID-namespaced containers. We can detect this by the presence of the PID namespace flag (stored on `ContainerInstance` or `RunningContainer`).

## Additional Investigations Needed

| Item | Status | Action |
|---|---|---|
| `stop_container` process group kill | ✅ Works | `setsid()` in grandchild establishes process group. `kill(-gc_pid, SIGKILL)` kills all container processes |
| `ProcessTracker::reap_zombies` vs `InitHandler::reap_zombies` race | ⚠️ Pre-existing | Both call `waitpid(-1, WNOHANG)`. If init task reaps first, process tracker misses the event and container won't restart. Fix: remove `waitpid` from init task, keep only signal forwarding there |
| `/proc` mount after pivot_root | ✅ Works | `mount_filesystems(true)` already mounts fresh `proc` — will show only processes in the new PID namespace |
| veth/sync protocol timing | ✅ Works | Grandchild inherits sync_w/ack_r fds. Parent reads gc_pid from pipe first, then calls `handle_veth_netns` which waits for grandchild's "S" byte |
| UID/GID mapping (userns) | ❌ Not needed | z8s runs as root → `spawn_root_ns_container` path only. `spawn_userns_container` is dead code when `is_root()` is true |
| Nginx multi-process in PID ns | ⚠️ Was the reason `CLONE_NEWPID` was disabled | The old comment said nginx gets `ENOMEM` when spawning workers. Needs testing with a fresh `/proc` mount in the new PID ns. The `mount_filesystems(true)` path mounts fresh proc/sysfs — this should resolve the ENOMEM issue because proc_nodev is now correct |
| Go runtime (whoami, http-echo) | ⚠️ Was the reason for userns case | The userns path (now dead code under root) had `EINVAL` issues. Not applicable |
| `init.rs` SIGCHLD handler double-vsps reaping | ⚠️ Need to decouple | Remove `waitpid` from `InitHandler::reap_zombies`. Init handler should only handle shutdown signals (SIGTERM/SIGINT) and forward them |

## Edge Cases & Risks

| Edge case | Mitigation |
|---|---|
| Container that forks worker processes (nginx, uwsgi) | Fresh `/proc` mount in new PID ns. Grandchild (PID 1) must reap zombies. If the container entrypoint doesn't reap, zombie accumulation. Users should add `tini` or a proper init to their image |
| Container process exits → kernel SIGKILLs all namespace processes | This is correct shutdown behavior. All container processes are killed |
| Capability removal breaks existing workloads | Add `["KILL", "DAC_OVERRIDE", "NET_RAW", "MKNOD"]` to `securityContext.capabilities.add` in the Pod spec |
| HostNetwork pods | `spec.host_network: true` → skip netns, just like before |
| Shared process namespace (`shareProcessNamespace: true`) | Will work across containers in the same pod — they're in the same PID namespace |
| `kubectl exec` / debugging | Works through the existing veth/netns setup, no change needed |

## Summary of Files to Change

| File | Change |
|---|---|
| `src/cri/runtime.rs` | Double-fork, gc_pid pipe, grandchild PID tracking |
| `src/cri/rootfs.rs` | Split `child_enter_ns_root` into unshare + rootfs phases; drop capabilities; overlay mount rootfs |
| `src/init.rs` | Add `PR_SET_CHILD_SUBREAPER`; remove `waitpid` from zombie reaping |
| `src/components/compute/spec_builder.rs` | Default network isolation based on `hostNetwork` |
| `src/cri/runtime.rs` | `stop_container` skip SIGTERM for PID-ns containers |
| `src/cri/image.rs` | `unpack_image` returns image cache path + skips `copy_cache_to_container` |
| `src/cri/spec.rs` | `ContainerConfig` adds optional `image_cache_path` for overlay lowerdir |

---

## Bonus: Overlay Mount Rootfs

### Problem

Every container gets a full recursive copy of the entire image rootfs via `copy_dir()` in `image.rs:213-214`.

```
images/{hash}  ──copy_dir()──►  rootfs/{container_id}  (full copy, 100s of MB)
```

At scale (many replicas of the same image) this wastes disk, startup time, and I/O bandwidth.

### Solution — Overlay Mount

Replace the copy with an overlay mount. The shared image cache becomes the read-only `lowerdir`, and each container gets a thin writable `upperdir`:

```
Pod layout:
  /var/lib/z8s/
    images/{image_hash}/          ← shared, read-only (lowerdir)
    rootfs/{container_id}/
      upper/                      ← per-container writable layer (tmpfs or dir)
      work/                       ← overlayfs internal
      merged/                     ← overlay mount → container sees as /
```

### Current flow (simplified):

```
image.rs:
  unpack_layer() → extracts into images/{hash}
  copy_cache_to_container(images/{hash}, rootfs/{id})

rootfs.rs:
  mount_rootfs_components():
    MS_BIND rootfs/{id} onto itself
    MS_BIND /proc, /sys from host
    pivot_root(rootfs/{id})
```

### New flow:

```
image.rs:
  unpack_layer() → extracts into images/{hash}  (same)
  returns images/{hash} path, skips copy

rootfs.rs:
  mount_rootfs_components():
    mount -t overlay:
      lowerdir=images/{hash}
      upperdir=rootfs/{id}/upper
      workdir=rootfs/{id}/work
      merged=rootfs/{id}/merged         ← target for chroot/pivot_root
    MS_BIND /proc, /sys from host into merged/
    pivot_root(rootfs/{id}/merged)
```

### Changes needed

| File | Change |
|---|---|
| `src/cri/image.rs` | Remove `copy_dir()` in `unpack_image`. Return `(rootfs_path, image_cache_path)` so the overlay mount knows the lowerdir |
| `src/cri/spec.rs` | Add `image_cache_path: Option<String>` to `ContainerConfig` |
| `src/cri/runtime.rs` | Pass `image_cache_path` through to `spawn_root_ns_container` |
| `src/cri/rootfs.rs` | New function `maybe_mount_overlay(rootfs_path, image_cache_path)` called in `mount_rootfs_components` before pivot_root. Creates `upper/`, `work/`, `merged/` dirs, runs `mount -t overlay` |
| `src/cri/volumes.rs` | Volume bind mounts target `merged/` instead of rootfs root |
| `src/cri/rootfs.rs` | `prepare_rootfs()` (mknod, resolv.conf, etc.) targets `merged/` instead of rootfs root |

### Web Research Findings

**Docker overlay2 driver** — Docker stores each image layer in a separate directory with a `diff/` subdirectory. A symlink directory `l/` holds short-name links to avoid hitting the kernel's 4096-byte mount argument limit. The overlay mount uses colon-separated `lowerdir=l/A:l/B:l/C:...` with the topmost layer leftmost (highest priority). Each container gets `upper/`, `work/`, `merged/` directories. Mount flags are `rw,relatime,lowerdir=...,upperdir=...,workdir=...`.

**z8s simplification** — Since z8s already flattens all layers into a single cache directory during `unpack_image()`, our lowerdir is a single path (no colon-separated list needed). This avoids the symlink length workaround entirely. The flattened cache is one directory, shared read-only by all containers of the same image.

**Rust nix mount API** — `nix::mount::mount()` accepts `data: Option<&P>` which receives the overlay options string:

```rust
use nix::mount::{mount, MsFlags};
use std::ffi::CString;

let data = CString::new(format!(
    "lowerdir={},upperdir={},workdir={}",
    image_cache_path, upper_path, work_path
)).unwrap();

mount(
    Some("overlay"),                         // source (filesystem name)
    &merged_path,                            // target mountpoint
    Some("overlay"),                         // fstype
    MsFlags::MS_NODEV | MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID,
    Some(data.as_c_str()),                   // overlay options
)?;
```

(The `nix::mount::mount()` data parameter maps to the `data` argument of the `mount(2)` syscall, which for overlayfs receives the comma-separated `lowerdir=...,upperdir=...,workdir=...` string.)

**Docker `overlay2` vs containerd `overlayfs`** — Docker Engine 29.0+ migrated to containerd's `overlayfs` snapshotter, which uses the same kernel overlay mechanism but manages snapshots (layer checkpoints) for container lifecycle. z8s doesn't need a full snapshotter — a single overlay mount per container is sufficient.

**Kernel support** — Overlayfs merged in Linux 3.18 (2014). The `overlay2` driver requires kernel 4.0+. Also requires `d_type=true` on the backing filesystem (xfs: `ftype=1`, ext4: default on). z8s should probe `/proc/filesystems` for `nodev\toverlay` and fall back to `copy_dir()` if unavailable.

### Implementation detail — mount ordering

```
Before pivot_root, inside the child/grandchild's mount namespace:
  1. mount -t overlay ... rootfs/{id}/merged       ← overlay mount
  2. MS_BIND /proc           → rootfs/{id}/merged/proc
  3. MS_BIND /sys            → rootfs/{id}/merged/sys
  4. MS_BIND /dev/{null,...} → rootfs/{id}/merged/dev/...  (or mknod in upper)
  5. pivot_root(rootfs/{id}/merged)                 ← container sees merged as /
```

After `pivot_root`, the fresh proc mount inside the new PID namespace hides host processes.

### Considerations

| Item | Notes |
|---|---|
| **Image cache must stay immutable** | After overlay is mounted, writes to `images/{hash}` would corrupt running containers. Ensure nothing mutates the cache after extraction |
| **Container deletion** | `umount rootfs/{id}/merged`, then delete `rootfs/{id}`. Upper/work dirs hold only container modifications |
| **Volumes** | Bind-mounts overlay `merged/` as the pivot target. Volumes are bind-mounted into `merged/{container_path}` as before |
| **`/etc/resolv.conf` / `/etc/hosts`** | Written into `upper/` (since the container will see them in merged view). `prepare_rootfs()` targets `merged/` |
| **Device nodes** | `mknod` in `upper/` (needs `CAP_MKNOD` or bind-mount from host). If we drop `CAP_MKNOD` (see capability hardening), we fall back to bind-mounting device nodes from host into `merged/dev/` |
| **`copy_up` on first write** | Overlayfs copies data from lower to upper on first modification. This is lazy — no upfront copy cost |
| **Disk usage** | Only modified files occupy space in `upper/`. For read-mostly workloads, this is near-zero per-container overhead |
| **Kernel support** | Overlayfs merged into Linux 3.18. z8s should probe `/proc/filesystems` for `overlay` and fall back to `copy_dir()`. Backing fs must support `d_type` (ext4: default on, xfs: `ftype=1`) |
| **`rename()` limitation** | Overlayfs returns `EXDEV` when renaming across layers. Applications must handle this (Docker docs note that `yum` needs `yum-plugin-ovl`). Unlikely to affect most z8s workloads |

## Test Plan

1. **Unit**: `cargo test` — existing tests should pass (capability drop may affect tests that check for specific cap sets)
2. **Hub-spoke**: `tests/netmux/test_hub_spoke.sh` — dep-hub container should NOT see host processes, should NOT kill host processes, should only reach spoke services (not host)
3. **Multi-node gossip**: `tests/cluster/start-3-nodes.sh` — verify pods start correctly in PID namespaces
4. **Nginx test**: Deploy an nginx pod with `containerPort: 80` — verify nginx starts and serves without `ENOMEM`
5. **Capability verification**: Deploy a pod that tries `kill -9 1` (host PID 1) — should fail with EPERM
6. **HostNetwork pod**: Deploy pod with `spec.hostNetwork: true` — should work without netns isolation
7. **Overlay smoke test**: Deploy multiple replicas of same image, verify disk usage is near-zero per container (no full rootfs copies)
8. **Overlay write test**: Container writes files, restart, verify writes persist in upperdir
9. **Overlay fallback test**: Run on a kernel without overlayfs support, verify fallback to `copy_dir()` works
