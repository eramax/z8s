# z8s — Findings & review notes

**Last updated:** 2026-05-26 (post exec/degraded audit + test rerun)  
**Sources:** `tests/run-network-fixes.sh`, `tests/run-network-failures.sh`, `/tmp/z8s-test-output.txt` (159/15), `tests/report3.md`, `ROADMAP.md`, host checks (`chroot`, `uidmap`, `subuid`).  
**Audience:** Honest status before merging or planning real fixes — not a release note.

---

## Executive summary

1. **The main dev blocker is filesystem isolation, not missing packages.** On this host, z8s run as user `abb` hits `mount MS_PRIVATE` → **EACCES**, `pivot_root` failure, `chroot` → **EPERM**, then **degraded** mode (user namespace only, host mount view). OCI workloads (nginx, python slim, musl alpine) misbehave without pivot/chroot. **`README.md` / `ROADMAP.md` expect root (or PID 1) for full OCI containers.**

2. **Run z8s as root for realistic container/network tests:** `sudo ./target/release/z8s` or `sudo z8s-daemon start` after `install.sh`. **`uidmap` is already installed** (`newuidmap`, `/etc/subuid` for `abb`); that is not the gap. **`sudo chroot` into a rootfs works** on this machine; **non-root z8s does not.**

3. **A partial Phase 6 slice is in tree** (not pasta/CNI): `CLONE_NEWNET` when `containerPort` and/or **Service `targetPort`** (int) apply; loopback; **host-published** ports (`port_publish.rs`); late publish when Service appears after Pod; ClusterIP proxy via `backend_connect_port()`.

4. **Deployment lifecycle fix is real:** controller/API only count `{deployment}-pod-*`. **`./z8s.sh stop` must kill all `z8s` PIDs** before tests.

5. **Focused tests are not green when run as non-root `abb`:** latest runs **9/3** (`run-network-fixes.sh`) and **9/12/1 skip** (`run-network-failures.sh`). Earlier **12/12** was valid on a single daemon when backends actually listened; current failures align with **degraded rootfs + backends down**, not “proxy missing.”

6. **Do not add per-app runtime hacks** (e.g. `is_nginx_program`, `daemon off` in Rust). Roadmap fix is **pivot/chroot** (or overlay + pivot). Image/command fixes belong in **manifests or OCI entrypoint**, not the runtime.

---

## What you should do on this machine

### Already OK (no install required)

| Check | Status |
|-------|--------|
| `uidmap` (`newuidmap`, `newgidmap`) | Installed |
| `/etc/subuid`, `/etc/subgid` for `abb` | Present |
| `kernel.unprivileged_userns_clone` | `1` |
| `sudo chroot` into cached rootfs | Works |

### Do this for development

```bash
cd /home/abb/dev/z8s
cargo build --release

./z8s.sh stop
sudo ./target/release/z8s >> /tmp/z8s.log 2>&1 &
# or: sudo z8s-daemon start   (after ./install.sh)

pgrep -af z8s          # one process, running as root
./tests/run-network-fixes.sh
./tests/run-network-failures.sh
```

**Verify in log:** no repeated `filesystem isolation unavailable` / `chroot(...) failed: EPERM`. Expect `chroot` or `pivot_root` success paths in `src/container/rootfs.rs`.

### Optional

```bash
sudo mkdir -p /etc/z8s/manifests   # silences manifest watcher errors in dev
```

### Not required yet (roadmap future)

- **pasta** (Phase 6)
- **CNI / eBPF** (Phase 7)
- **fuse-overlayfs** (overlay fallback; kernel 7.x has user-ns overlay)

---

## Test results

| Run | Passed | Failed | Skip | Notes |
|-----|--------|--------|------|-------|
| report3 | 144 | 16 | — | Baseline doc |
| Full latest (`/tmp/z8s-test-output.txt`) | **159** | **15** | — | ~10 min; after deployment + netns |
| **`tests/run-network-fixes.sh`** (2026-05-26, user `abb`) | **9** | **3** | — | ClusterIP: web-a, web-b, python-fix-svc |
| **`tests/run-network-failures.sh`** (2026-05-26, user `abb`) | **9** | **12** | **1** | ubuntu GLIBC skipped |
| **`run-network-fixes.sh`** (historical) | **12** | **0** | — | When rootfs/backends healthy + single `z8s` |

### `run-network-fixes.sh` — latest failures (user `abb`)

| Fail | Cause (honest) |
|------|----------------|
| web-a-svc, web-b-svc | Backends not listening on published ports — nginx/whoami in **degraded** netns or exit loop |
| python-fix-svc | Host-network / single-listener path; collision or no listener on 18080 |

### `run-network-failures.sh` — latest failures (user `abb`)

| Bucket | Failures |
|--------|----------|
| Workload | `nginx-hello` readyReplicas=0 / Failed |
| ClusterIP (6) | All svc HTTP via `svc-client` — `wget: error getting response` (proxy up, backends down) |
| Exec (5) | whoami/http-echo ENOENT; hostinfo bad address; dashboard connection refused; nginx-hello Failed |
| Skip | ubuntu GLIBC (host libc vs image) |

**Passes:** Most deployments ready (except nginx-hello), python-pod, svc-client, deployment reconcile checks, non-isolated alpine exec.

---

## Degraded vs supported (ROADMAP alignment)

| Mode | When | Filesystem | Typical symptom |
|------|------|------------|-----------------|
| **Supported** | z8s **root** / PID 1 | `pivot_root` or `chroot` | Process sees `/` as image; volumes bind before pivot |
| **Degraded** | z8s **non-root** `abb` | User ns only | `chroot EPERM`; exec via host paths + ELF interpreter; wrong `/etc` for daemons |

**ROADMAP Phase 1** is marked “Done” but **FINDINGS ~50%**: pivot/chroot often fail in dev → degraded profile. **Phase 2** bind mounts require pivot path. **Phase 6** pasta not implemented; current work is NEWNET + port publish only.

**Design principle (ROADMAP):** *Rootless-first, root-capable* — dev as `abb` is intentionally weak; production init runs with capabilities.

---

## Fixes landed (product code, cumulative)

| Area | Change | Files |
|------|--------|-------|
| Deployment reconcile | Only `{deploy}-pod-*`; ignore label-only standalones | `src/controller.rs` |
| Deployment status | Same name-prefix count | `src/server/api.rs` |
| Per-pod network | `CLONE_NEWNET` if `containerPort` **or** Service `targetPort` (int) | `src/supervisor/process.rs`, `rootfs.rs` |
| Late port publish | `reconcile_network_for_service`, `append_ports` | `process.rs`, `port_publish.rs`, `network/mod.rs` |
| Port publish | `127.0.0.1:hostPort` → netns `127.0.0.1:containerPort` | `src/network/port_publish.rs` |
| Service proxy | `backend_connect_port()` | `service_proxy.rs`, `process.rs` |
| Exec (degraded) | `wrap_dynamic_linker` (ELF PT_INTERP); `container_fs_isolated`; no mount-ns paths when not chrooted | `rootfs.rs`, `exec.rs` |
| Exec (alpine) | Symlink resolve (`sleep` → `busybox`); musl via loader when degraded | `rootfs.rs` |
| OCI entrypoint | Empty/null saved config → `guess_image_config` (entrypoint scripts, single root exe); backfill on pull | `oci_config.rs`, `image.rs` |
| Namespace delete | Cascade namespaced resources | `src/server/api.rs` |
| Operator | `z8s.sh stop` pkill-all `z8s` | `z8s.sh` |
| Regression scripts | `run-network-fixes.sh`, **`run-network-failures.sh`** | `tests/` |

**Removed / rejected:** Per-app nginx argv in Rust (`is_nginx_program`, `apply_degraded_nginx_args`, `apply_foreground_args`). Renumbering `containerPort` in test YAML.

---

## Architecture: current vs intended

### Intended (ROADMAP)

```
unshare(USER|NS|PID|NET|IPC|UTS) → bind volumes → pivot_root(overlay rootfs) → exec
Client → ClusterIP → proxy → pod backend (pod IP or published port)
Phase 6: pasta + NEWNET (not in tree)
```

### Current (non-root dev on this host)

```
unshare(USER|NS|UTS|IPC|[NET]) → pivot/chroot FAIL → degraded
exec: host path + dynamic linker wrapper for musl/glibc ELF
NET: NEWNET + port publish (works at TCP layer if process stays up)
```

**Gap:** Without chroot, daemons read **host** `/etc/nginx`, **host** libc paths; many exit or never listen → ClusterIP tests fail despite proxy binding `127.96.x.x`.

---

## Operational gotchas

| Symptom | Cause |
|---------|--------|
| `readyReplicas=3` but 2 managed pods | Stale `z8s` on `:6443` |
| `cargo build` ignored | Orphan daemon |
| Deployments “Ready” but svc tests fail | API ready ≠ process listening (crash loop in degraded) |
| `chroot(...) failed: EPERM` in log | **Run z8s as root** |

```bash
pgrep -af z8s    # one process
grep -E 'degraded|chroot|pivot' /tmp/z8s.log | tail -20
```

---

## ROADMAP phase completion

| Phase | Status | ~% | Notes |
|-------|--------|-----|-------|
| **0** Exec | Partial | 75 | Alpine + loader OK in degraded; netns pods need chroot |
| **1** User/mount/UTS | Partial | 50 | **Blocked in dev by non-root** → degraded |
| **1.5** NEWPID/Landlock | Divergent | 25 | NEWPID removed (Go threads); NEWIPC yes |
| **2** Volumes | Partial | 55 | Bind EACCES / skip in degraded |
| **3** ClusterIP | Partial | 55 | Proxy OK; backends fail without rootfs |
| **4** cgroup | Partial | 30 | Needs root |
| **5** Ingress | Not started | 0 | — |
| **6** NEWNET | Partial | 40 | Publish + reconcile; **no pasta** |
| **7** CNI / eBPF | Not started | 0 | — |
| **8** Multi-node | Not started | 0 | — |

---

## Runtime findings

### Service proxy

Implemented. Fails when **no healthy TCP backend** on `127.0.0.1:<published_port>` — common in degraded mode, not a missing proxy.

### Exec

| Case | Status |
|------|--------|
| Non-isolated alpine (`svc-client`) | OK with `wrap_dynamic_linker` |
| Isolated netns + **no** fs isolation | ENOENT / wrong paths for Go static images; use root z8s |
| Ubuntu | GLIBC_2.43 vs host — environment |

### nginx / nginx-hello

Fails in degraded + netns: wrong root, daemon exit, missing OCI entrypoint in cache. **Fix:** run z8s as root; ensure image **Entrypoint/Cmd** in manifest if image defaults wrong — **not** Rust nginx detection.

### Python `python-deploy`

Manifest binds `127.0.0.1:18080` without `containerPort`; Service `targetPort` should trigger netns + publish after `service_target_ports_for_service` — still verify under **root** z8s.

---

## Recommended direction (priority)

### 1. Environment (you, today)

- Run **z8s as root** for OCI and re-run `run-network-fixes.sh` + `run-network-failures.sh`.
- Confirm log shows chroot/pivot, not degraded.

### 2. Product (code)

1. Document degraded mode limits in `README.md` / warn when `!is_root()` at startup.
2. Host-network port allocation OR require `hostPort` for pods without netns.
3. **Owner references** on deployment pods (optional; prefix works).
4. Phase 6: **pasta** per ROADMAP (replace ad-hoc publish-only story long-term).
5. Exec: only needed in degraded; should simplify once chroot is universal in dev/prod.

### 3. Verification

```bash
cargo build && ./z8s.sh stop
sudo ./target/release/z8s >> /tmp/z8s.log 2>&1 &
pgrep -af z8s
./tests/run-network-fixes.sh
./tests/run-network-failures.sh
# Full suite only when needed:
# ./tests/run-tests.sh 2>&1 | tee /tmp/z8s-test-output.txt
```

### Do not do

- Per-app branches in Rust (`nginx`, `python`, etc.).
- Renumber `containerPort` in YAML to greenwash.
- Assume 12/12 focused without root + single `z8s`.
- Trust tests while log shows `filesystem isolation unavailable`.

---

## Open questions

1. Startup warning + exit if OCI requested but not root? Or explicit `--allow-degraded`?
2. Host-port registry for pods without `containerPort`?
3. Persist `ownerReferences`?
4. Ubuntu on old hosts: skip vs bundle vs newer image?
5. Re-run full suite as **root** and refresh 159/15 baseline?

---

## Code touchpoints

| Area | Path |
|------|------|
| Isolation / chroot / degraded | `src/container/rootfs.rs` |
| OCI entrypoint / guess | `src/container/oci_config.rs`, `image.rs` |
| Port publish / loopback | `src/network/port_publish.rs` |
| Spawn, service ports, publish | `src/supervisor/process.rs` |
| Service proxy | `src/network/service_proxy.rs`, `network/mod.rs` |
| Exec | `src/server/exec.rs` |
| Fast regression | `tests/run-network-fixes.sh`, `tests/run-network-failures.sh` |
| Full suite | `tests/run-tests.sh` |

---

*Supersedes “12/12 always” and “install uidmap to fix nginx.” Primary blocker for this dev host: **run z8s as root** for pivot/chroot per ROADMAP Phase 1.*
