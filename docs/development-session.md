# Development Session: Container Isolation & Runtime Architecture

## Overview
This session hardened container isolation (PID namespaces, capabilities, exec), fixed overlay rootfs, added cluster management commands, and split the binary into runner + node architecture.

## Changes Made

### 1. Exec PID Isolation (`src/cri/exec.rs`)

**Problem**: `kubectl exec` showed all host processes inside the container. `/proc/self` was broken.

**Root cause**: `setns(CLONE_NEWPID)` gives a **ghost PID** — the process moves into the target PID namespace but the old procfs (mounted by PID 1 at container startup) doesn't show this ghost. Tools like `ps` that scan `/proc` would either fail (`/proc/self` broken) or show host processes.

**Fixes applied**:

| Bug | Location | Fix |
|-----|----------|-----|
| `container_fs_isolated()` always false after pivot_root | `exec.rs:394` | Removed `fs_isolated` from `use_mnt_ns` equation; use `binary_in_rootfs && can_enter_mnt` |
| `spawn_with_pty` gated mnt-ns entry on `isolated_net` | `exec.rs:238` | Changed `enter_container_namespaces(ns, isolated_net, isolated_net)` to always enter mnt ns |
| `setns(CLONE_NEWPID)` needs `fork()` after for real PID | `exec.rs` | After entering user+mnt+net+pid namespaces, `fork()` — child mounts fresh procfs and execs target; parent waits and exits with child's exit code |
| `std::process::exit()` closes child's inherited fds | `exec_container_pre_exec` | Changed to `libc::_exit()` — raw syscall, no destructors |
| Stale PID after container restart | `resolve_container` | Added `is_pid_alive` filter checking `/proc/{pid}` exists |
| TTY exec hang (setsid/TIOCSCTTY orphaned) | `spawn_with_pty` | Consolidated pre_exec into single function; TTY setup runs in same process that execs |

**Current behavior**:
- `kubectl exec pod -- ps` → shows only container processes
- `kubectl exec -it pod -- sh` → interactive shell works
- Container's PID 1 remains the entrypoint

### 2. Overlay Rootfs Removal (`src/cri/image.rs`)

**Problem**: `mount_overlay_rootfs` mounted overlay on the host filesystem (tmpfs + overlay). The overlay mount succeeded on the host but was invisible inside the container's mount namespace after `pivot_root`/`chroot`. Pods crash-looped with `execvpe(python3) failed: ENOENT`.

**Fix**: Replaced overlay approach with `copy_cache_to_container` — always copies the image cache to the container rootfs.

```rust
// Before: tmpfs + overlay mount
fn mount_overlay_rootfs(...) {
    mount("tmpfs", container_rootfs);
    mount("overlay", merged, lowerdir=cache, upperdir=upper, workdir=work);
    Ok(merged) // invisible inside container namespace
}

// After: direct copy
fn mount_overlay_rootfs(...) {
    copy_cache_to_container(cache_path, container_rootfs, ...)
}
```

### 3. Process Manager & CLI (`src/main.rs`, `src/node.rs`)

**Problem**: `fork()` inside `#[tokio::main]` causes deadlock — tokio's worker threads hold internal mutexes that are duplicated in an undefined state after fork. The child process hangs on a futex immediately after fork.

**Fix**: Split binary into two modes:

| Mode | Binary entry | Runtime | What it does |
|------|-------------|---------|-------------|
| **Runner** | `main.rs` (~202 lines) | Synchronous only | Spawns node processes via `Command::spawn`, returns to shell |
| **Node** | `node.rs` (~280 lines) | Full tokio async | API server, pod runtime, gossip, scheduler |

**Flow**:
1. `sudo z8s` → runner calls `Command::spawn("z8s run --port 6443")` → exits
2. Node process starts fresh with its own tokio runtime → no fork involved
3. `sudo z8s restart` → runner SIGTERMs old node → spawns new one
4. `sudo z8s node start --port X` → runner spawns additional node with correct `--peers`

### 4. Lock & Daemonization

**Problems found**:

| Approach | Issue |
|----------|-------|
| PID file (`/tmp/z8s-{port}.pid`) | Stale PIDs, race conditions, `sudo` process named "sudo" not "z8s" |
| `flock(LOCK_EX)` | Not inherited by child after `fork()` |
| `fork()` inside tokio runtime | Deadlock on internal mutexes (worker threads hold locks) |
| `fork()` before tokio runtime | Rust allocator (`malloc`) mutex deadlock after fork |

**Final solution**: No daemonization. Runner spawns node as a child process. Node runs in foreground with its own tokio runtime. Lock file uses `flock(LOCK_EX | LOCK_NB)` acquired by the node process itself.

### 5. Cluster Management

**Added commands**:

| Command | Description |
|---------|-------------|
| `sudo z8s` | Spawn default node on :6443, return to shell |
| `sudo z8s restart` | SIGTERM + respawn |
| `sudo z8s node start --port X [--peer-addr IP] [--service-cidr] [--pod-cidr]` | Spawn additional node |
| `sudo z8s run --port X` | Run node in foreground (for systemd) |
| `sudo z8s join ws://...` | Join cluster as worker (unchanged) |

**Peer address fix**: `node start` was using the new node's `--port` value as the main node's port for the peer address. Fixed by hardcoding main port to 6443.

### 6. Gossip Sync

**Observation**: Secondary nodes connect to the main node via WebSocket. The initial `SyncFull` exchange sends all resources. However:
- Main → secondary sync works for the initial exchange
- Anti-entropy loop is a **no-op** (computes hash but never compares)
- If initial `SyncFull` is missed, nodes never catch up
- This is pre-existing, not introduced in this session

## Files Changed

| File | Lines | Change |
|------|-------|--------|
| `src/cri/exec.rs` | ~780 | PID namespace forking, TTY exec fixed, stale PID filter |
| `src/cri/image.rs` | ~340 | Overlay → copy, removed overlay imports |
| `src/cri/rootfs.rs` | ~980 | `container_fs_isolated` kept but no longer used by exec |
| `src/main.rs` | ~202 | Rewritten as runner CLI (was ~610 lines) |
| `src/node.rs` | ~280 | **New** — extracted server init from old main.rs |
| `src/config.rs` | ~310 | Added `run`/`restart`/`node` as recognized args |
| `src/api/server.rs` | ~690 | SO_REUSEADDR for fast restart |
| `src/store/ws.rs` | ~176 | Debug logging (minor) |

## Key Decisions

1. **Single binary, two modes** — avoids IPC complexity while keeping runner lightweight
2. **No `fork()` in async context** — `Command::spawn` is safe, fork is not
3. **`_exit()` over `process::exit()`** — prevents destructors from closing shared fds
4. **Copy rootfs over overlay** — overlay invisible inside container mount namespace
5. **No PID ns entry in exec** — kernel limitation on 7.0.0-15; setns into child PID namespace + fresh proc mount still shows host PIDs. Fork is required for real isolation.

## Known Issues

1. **Anti-entropy no-op** — gossip nodes may miss syncs if initial `SyncFull` fails
2. **Node pod count** — requires scheduler to set `nodeName` on pods
3. **`i/o timeout` on fast TTY commands** — WebSocket close handshake race (cosmetic)
