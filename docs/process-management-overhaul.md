# z8s Process Management Overhaul

## Problem Statement

z8s had critical issues with process management that caused:

1. **Multiple z8s processes running simultaneously** — no global instance lock prevented
   starting a second z8s on the same machine (different ports).
2. **kubectl hangs forever** — when multiple z8s processes fought over the same
   resources (ports, nftables rules, cgroups), the API server became unresponsive.
3. **Unreliable PID tracking** — the binary spawned a child and exited, making PID
   tracking impossible via shell scripts.
4. **No node lifecycle management** — `node start` spawned child processes that were
   completely untracked; no `node stop`, no `node list`, no cleanup.
5. **Panics on degraded systems** — `CgroupManager::new()` and `ImageManager::new()`
   would retry and panic on failure instead of degrading gracefully.
6. **Blocking in async context** — gossip setup used `block_in_place` + `block_on`
   inside an async function.
7. **No resource cleanup on shutdown** — nftables rules, orphan veths, lock files
   were left behind on shutdown.

## Architecture

### Process Model

```
z8s (CLI invocation)
  └── z8s run --port 6443  [daemon child, keeps running]
       ├── API server (axum, port 6443)
       ├── Reconciler (2s tick)
       ├── DNS server (port 53)
       ├── Manifest watcher
       ├── Process tracker + zombie reaper
       ├── Network manager (service proxy)
       ├── Gossip clients (WebSocket to peers)
       ├── Scheduler (if redb enabled)
       └── Container processes (forked children)

  z8s node start --port 7443
  └── z8s run --port 7443 --peers main=127.0.0.1:6443
       └── [independent z8s instance, managed by main]
```

### Lock & PID File Layout

```
/tmp/z8s.lock                # Global instance lock (flock)
                             # Only ONE main z8s per machine
/tmp/z8s-<port>.lock         # Per-port lock (flock)
                             # Prevents two z8s on same port
/tmp/z8s-<port>-main.pid     # Main z8s PID file
/tmp/z8s-<port>-node.pid     # Node z8s PID file
```

- **Global lock**: `flock(LOCK_EX | LOCK_NB)` on `/tmp/z8s.lock`. Only the main z8s
  instance acquires this. Prevents running two `z8s` commands simultaneously.
- **Per-port lock**: `flock(LOCK_EX | LOCK_NB)` on `/tmp/z8s-<port>.lock`. Every z8s
  instance (main or node) acquires this. Prevents port conflicts.
- **PID files**: Written on startup, removed on shutdown. Contains the process PID.
  Stale PID files (dead process) are auto-cleaned.

### CLI Commands

```
z8s                          # Start main z8s (daemon mode)
z8s run --port <PORT>        # Run as server (foreground, for systemd/scripts)
z8s stop                     # Stop the main z8s instance
z8s restart                  # Restart the main z8s instance
z8s status                   # Show running z8s processes
z8s node start --port <PORT> # Start a new node
z8s node stop --port <PORT>  # Stop a node
z8s node list                # List running nodes
z8s join <ws-url>            # Join a cluster as worker
```

### Shutdown Sequence

1. Signal received (SIGTERM/SIGINT)
2. `shutdown_tx.send(true)` triggers watch channel
3. All pods stopped via `RuntimeProvider::stop_pod()`
4. nftables rules cleaned up
5. Orphan veths cleaned up
6. Lock files removed
7. PID files removed
8. Process exits

## Changes Made

### `src/main.rs` — Process lifecycle management

- **Global instance lock**: `acquire_global_lock()` uses `flock()` on `/tmp/z8s.lock`.
  Only the main z8s (no `--peers`) acquires this. Prevents multiple instances.
- **Per-port lock**: `acquire_port_lock()` uses `flock()` on `/tmp/z8s-<port>.lock`.
  Every z8s instance acquires this.
- **PID file management**: `write_pid_file(port, role)` and `remove_pid_file(port, role)`.
  Role is "main" or "node". PID files contain the process PID, cleaned up on shutdown.
- **Stale PID detection**: `read_pid_file()` checks if the PID is alive; removes stale files.
- **Node lifecycle**: `node_start()`, `node_stop()`, `node_list()` — full node management.
- **Stop command**: `stop_z8s()` sends SIGTERM, waits up to 10s, then SIGKILL.
- **Status command**: `show_status()` lists all running z8s processes from PID files.
- **Removed**: The old `find_z8s_pid()` that searched /proc for a single PID.
  Replaced with `find_z8s_pids()` that returns all z8s PIDs.

### `src/node.rs` — Daemon runtime

- **CgroupManager fallback**: Uses `CgroupManager::new_stub()` instead of panicking.
  Logs a warning, continues without resource limits.
- **ImageManager fallback**: Uses `ImageManager::new_stub()` instead of panicking.
  Logs a warning, continues without image pulling.
- **Gossip fix**: Removed `block_in_place` + `block_on` hack. Now registers peer
  channels directly with `st.lock().await.add_peer(tx)`.
- **Shutdown cleanup**: Added nftables cleanup, orphan veth cleanup, lock file
  removal, and PID file removal on shutdown.
- **Lock file cleanup**: Removes the per-port lock file during shutdown.

### `src/cri/cgroup.rs` — Stub CgroupManager

- Added `CgroupManager::new_stub()` that returns a no-op cgroup manager.
  All methods return `Ok(())` or sensible defaults. No panics.

### `src/cri/image.rs` — Stub ImageManager

- Added `ImageManager::new_stub()` that returns a no-op image manager.
  Image pulls return an error, but the system continues running.

### `src/netmux/nftables.rs` — Cleanup method

- Added `NftEngine::cleanup()` that removes all z8s nftables rules.
  Called during shutdown to leave the system clean.

## File Responsibility Matrix

| File | Type | Purpose |
|------|------|---------|
| `/tmp/z8s.lock` | z8s-level | Global instance lock (one per machine) |
| `/tmp/z8s-<port>.lock` | z8s-level | Per-port lock (one per port) |
| `/tmp/z8s-<port>-main.pid` | z8s-level | Main instance PID file |
| `/tmp/z8s-<port>-node.pid` | node-level | Node instance PID file |
| `/tmp/z8s-<port>.log` | z8s-level | Main instance log file |
| `/var/lib/z8s/` | node-level | Container data (rootfs, images) |
| `/etc/z8s/manifests/` | z8s-level | Manifest directory |

## Verification

```bash
# 1. Build
cargo build

# 2. Start z8s
sudo ./target/debug/z8s

# 3. Verify single instance
./target/debug/z8s status
# Should show: z8s main running on port 6443 (PID xxx)

# 4. Try starting second instance (should fail)
sudo ./target/debug/z8s
# Should fail: "Another z8s instance is already running"

# 5. Test node management
./target/debug/z8s node start --port 7443
./target/debug/z8s node list
# Should show: main:6443 PID=xxx  node:7443 PID=yyy

# 6. Test stop
./target/debug/z8s node stop --port 7443
./target/debug/z8s stop

# 7. Verify cleanup
ls /tmp/z8s*
# Should be empty
```
