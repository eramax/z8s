# z8s — Findings & review notes

**Last updated:** 2026-05-26  
**Sources:** `tests/report3.md` (144/16), `/tmp/z8s-test-output.txt` (163/12), ROADMAP.md, code audit, review discussion.  
**Audience:** Honest status before merging or planning real fixes — not a release note.

---

## Executive summary

1. **ClusterIP proxy exists** but backends are wired as **`127.0.0.1:<containerPort>` on the host** (shared network). That is not Kubernetes pod networking. Multiple pods on containerPort **80 collide** on the host — this is a **Phase 6/7** problem, not a Service port problem.

2. **Report v3 B1** was mostly **crash-looping workloads** (wrong entrypoint, Go + `CLONE_NEWPID`, thread limits), not “proxy missing.” After real runtime fixes, **python / whoami / http-echo** ClusterIP tests can pass.

3. **No ROADMAP phase is fully “done as designed”** on a typical rootless dev host. Pivot/chroot often fail → **degraded profile** (user-ns + host filesystem). The roadmap header (“Phase 0–1 done, 38 tests pass”) is **stale**.

4. **A large follow-up pass added test/manifest hacks** (nginx **18082**, nginx-hello **18083**, alpine **`/tmp/data`**, re-apply alpine-pod in tests). That **must not** be treated as product fixes. **Port limits belong on Services (ClusterIP), not on pod `containerPort` in YAML.**

5. **Phase 6** (`CLONE_NEWNET` + pasta) is the roadmap answer for **pods using normal ports (e.g. 80)** while **Phase 3** keeps **Service → backend** forwarding. **Phase 7** is full pod CIDR + CNI.

---

## Test results

| Run | Passed | Failed | Notes |
|-----|--------|--------|-------|
| report3 | 144 | 16 | Baseline |
| v4 (`/tmp/z8s-test-output.txt`) | 163 | 12 | After `CLONE_NEWPID` removal, OCI entrypoint, endpoint probes |

### v4 improvements (real fixes)

| Area | v3 | v4 |
|------|----|----|
| Go services (whoami, http-echo) | Crash / timeout | Often pass ClusterIP wget |
| python-svc | Failed | Pass (non-default port 18080 in manifests) |
| emptyDir “survives recreate” | False positive (EACCES) | Correctly **lost** when bind works |
| ubuntu `securityContext` | uid issues | uid **1001** (subuid), `/etc` blocked |
| PV/PVC API tests | — | **12/12** API CRUD (see Phase 2 — mount not real) |

### v4 remaining failures (12) — root causes

| Bucket | Examples | Real fix (not test hack) |
|--------|----------|---------------------------|
| Shared host **:80** collision | nginx-svc, nginx-hello | Phase 6/7 or per-pod published backend in z8s |
| Volumes in degraded env | emptyDir `/var/data` missing | Phase 2 bind before pivot **or** netns; not `/tmp/data` in YAML |
| Exec WebSocket | whoami/http-echo info tests, 1006 | `exec.rs` protocol + setns/chroot/musl |
| Test ordering | alpine-pod gone by §19, label selector | Fix test order **or** keep pod alive; selector OK if alpine exists |
| nginx-hello workload | Deploy 0/1 Ready | Image/port/runtime — not “service port 80” |

**Label selector note:** `type=test-pod` filter can work; failure showed **postgres-pod** and **python-pod** because they **also** have `type: test-pod` in YAML — alpine-pod was **deleted** in §13, not because the API ignores selectors.

---

## Architecture: Services vs pods (what went wrong in review)

### Intended (Kubernetes + ROADMAP Phase 3 + 6)

```
Client → ClusterIP:servicePort (many services can use :80 on different 127.96.x.x)
       → z8s proxy
       → pod backend at podIP:containerPort (each pod may use :80 in its own netns)
```

### Current implementation

```
Client → ClusterIP:servicePort   ✅ (bind on 127.96.x.x)
       → proxy
       → 127.0.0.1:targetPort     ❌ on shared host network = one listener per host port
```

So **renumbering nginx to 18082 in Deployment YAML** avoids collision but **cheats**: it changes the pod spec instead of giving each pod a distinct reachable backend.

**Port restrictions should apply to Services (virtual ClusterIP ports), not to container ports in manifests.**

---

## Work that should be reverted or redone (review pass)

Do **not** treat these as completed product work:

| Change | Why it’s wrong |
|--------|----------------|
| nginx `listen 18082`, svc port 18082 | Pod spec surgery; masks host :80 collision |
| nginx-hello `18083` + `sed` in command | Fragile; same issue |
| alpine `mountPath: /tmp/data` | Not K8s semantics; hides `/var` EACCES on host |
| `degraded_mount_path(/var/data → /tmp/data)` | Hidden second spec |
| `run-tests.sh` re-apply alpine before §19 | Test ordering patch, not lifecycle fix |
| Loosened / best-effort PVC mount assertions | PV **volume bind not implemented** in `volumes.rs` |

**Legitimate runtime fixes to keep (separate from above):**

- OCI Entrypoint/Cmd + `.z8s-oci-config.json`
- `resolve_exec_path` / `build_container_argv` (busybox symlinks)
- Remove global `CLONE_NEWPID` until **conditional** profile exists
- Service proxy deferred bind + TCP endpoint probe
- `start_pod` on PATCH when pod not running

---

## ROADMAP phase completion (honest audit)

Legend: **Done** = matches phase intent on target env · **Partial** · **Not started** · **Divergent** = code exists but contradicts phase spec.

| Phase | Goal (ROADMAP) | Status | ~% | Main gaps / divergences |
|-------|------------------|--------|-----|-------------------------|
| **0** | Exec (TTY/pipes), rootless base | **Partial** | 80 | Exec still brittle (setns, musl, WS 1006) |
| **1** | NEWUSER+NEWNS+NEWUTS, pivot_root, maps, /proc/sys/dev | **Partial** | 50 | Pivot/chroot fail → host FS; not full isolation in dev |
| **1.5** | NEWPID, NEWIPC, Landlock, seccomp | **Divergent** | 25 | NEWIPC yes; **NEWPID removed**; no Landlock/seccomp |
| **2** | Volumes, CM, secret, PV/PVC, **overlayfs** | **Partial** | 55 | No overlay; PVC **API only**; degraded volume hacks |
| **3** | ClusterIP proxy, NodePort, **hickory-dns** | **Partial** | 50 | `127.96` not `10.96`; custom DNS; **host :port backends** |
| **4** | cgroup v2 non-root delegation | **Partial** | 30 | Limits code exists; **no delegation**; off without root |
| **5** | Ingress HTTP/S, WS, TLS | **Not started** | 0 | — |
| **6** | NEWNET + **pasta**, per-pod ports | **Not started** | 0 | **Fixes pod :80 + service routing model** |
| **7** | veth, bridge, pod CIDR, NetworkPolicy | **Not started** | 0 | Full K8s-shaped network |
| **8** | Multi-node agent/primary | **Not started** | 0 | — |

**None of phases 0–4 are “complete as designed” end-to-end on a rootless dev machine.** Closest value: **kubectl API**, **Deployment controller**, **partial isolation**, **partial proxy**.

### Phase 0 — detail

| As designed | In tree |
|-------------|---------|
| WS exec TTY + stdin/stdout/stderr | `src/server/exec.rs` |
| Rootless data under `~/.local/share/z8s` | `image.rs`, `volumes.rs` |

**Divergent:** Multiple exec refactors; duplicate JSON status bug was real; fix must stay single-frame + Close.

### Phase 1 — detail

| As designed | In tree |
|-------------|---------|
| User + mount + UTS ns | `child_enter_ns_fork` |
| pivot_root + bind /proc, /sys, /dev | Often **fails** → fallback message |
| UID/GID maps, subuid | `write_userns_maps`, `/etc/subuid` |

**Divergent:** **Degraded profile** = host filesystem visible; tests claiming “PID isolation ✅” overstated.

### Phase 1.5 — detail

| As designed | In tree |
|-------------|---------|
| NEWPID | **Removed** (Go `newosproc`) |
| NEWIPC | Yes |
| Landlock, seccomp | **Absent** (`Cargo.toml` has neither) |

**Recommendation:** Conditional NEWPID after successful FS isolation; then Landlock + seccomp.

### Phase 2 — detail

| As designed | In tree |
|-------------|---------|
| hostPath, emptyDir, configMap, secret | `volumes.rs` + bind/symlink/copy fallbacks |
| PV/PVC | **Store + API only** — no `persistentVolumeClaim` in `resolve_volume_source` |
| overlayfs shared layers | **Not implemented** — flat `images/` + per-container `rootfs/` copy |

**Divergent:** `stage_volumes_in_rootfs`, `bind_mount_volumes_degraded`, test path `/tmp/data`.

**PV/PVC tests:** CRUD passes; `pvc-pod` mount test is **best-effort** (“may not be implemented” in `run-tests.sh`).

### Phase 3 — detail

| As designed | In tree |
|-------------|---------|
| ClusterIP virtual IP + proxy | `network/mod.rs`, `service_proxy.rs` on **`127.96.0.0/16`** |
| NodePort | `0.0.0.0:30000+` |
| hickory-dns | **Custom UDP** DNS in `network/dns.rs` (not hickory) |
| Implicit host network | Yes — **backend dial = host localhost** |

**This phase does not deliver isolated pod networking** — that is Phase 6/7.

### Phase 4 — detail

| As designed | In tree |
|-------------|---------|
| Delegated cgroup path for non-root | **Not implemented** |
| memory.max, cpu.max from resources | `cgroup.rs` + `process.rs` when **root** |
| pids.max default | Not set |

### Phases 5–8

No meaningful implementation in codebase.

---

## Phase 6 vs 7 — networking (discussion summary)

| Question | Answer |
|----------|--------|
| Does Phase 6 fix networking? | **Fixes “each pod can use containerPort 80”** via per-pod netns + pasta. Service proxy (Phase 3) still forwards **ClusterIP → backend**, but backend becomes **pod-reachable address**, not shared `127.0.0.1:80`. |
| Is Phase 6 enough for “real K8s”? | **Step.** Pod IPs, bridge, policies = **Phase 7**. |
| Why did review change YAML ports? | **Wrong layer** — avoided implementing Phase 6/7 or a **supervisor published-port map**. |

ROADMAP quote (Phase 6): pasta `-t host:container` for simple publish; **“service load balancing”** still via Phase 3 tokio proxy — both must be wired together with **per-pod backend addresses**.

---

## Runtime findings (still valid)

### B1 — Service proxy

Proxy **is implemented**. Failures were mostly **backends not listening**.

Go crash (before NEWPID fix):

```text
runtime: failed to create new OS thread (have 2 already; errno=22)
fatal error: newosproc
```

Also: empty `command` → `/bin/sh`; Python `ThreadingHTTPServer` + thread limits.

### B2 — Exec

`kubectl exec` WebSocket **1006** / spawn ENOENT: setns vs chroot vs **musl** (`/bin/sh` → busybox must be `busybox sh …`); host-path vs in-container paths.

### B3 — Volumes

Bind before pivot fails in degraded env → emptyDir data in rootfs cache (false “survives recreate”) or missing mount path.

### B4 / B5 — alpine-pod & selectors

Lifecycle (delete in §13) and **multiple pods** with same label — not necessarily API bugs.

### `CLONE_NEWPID`

| Profile | NEWPID | When |
|---------|--------|------|
| A Full | On | pivot/chroot OK |
| B Degraded | Off | userns-only; Go works |

Permanent global removal ≠ Phase 1.5 complete.

---

## Environment (typical dev)

| Observation | Implication |
|-------------|-------------|
| Non-root z8s | `spawn_userns_container`; cgroup off |
| `MS_PRIVATE` / pivot / chroot EACCES | Degraded isolation |
| `uidmap` | Maps like uid **1001** on host |
| `kubectl apply --validate=true` | May fail; tests use `--validate=false` |

Logs: `/tmp/z8s.log` · API: `http://localhost:6443` · Binary: `target/debug/z8s` via `z8s.sh`.

---

## Code touchpoints

| Area | Path |
|------|------|
| Namespaces / pivot / exec path | `src/container/rootfs.rs` |
| Spawn, argv, probes | `src/supervisor/process.rs` |
| OCI entrypoint | `src/container/oci_config.rs`, `image.rs` |
| Service proxy | `src/network/service_proxy.rs`, `network/mod.rs` |
| DNS | `src/network/dns.rs` |
| Volumes | `src/container/volumes.rs` |
| Exec WS | `src/server/exec.rs` |
| API / PV / selectors | `src/server/api.rs` |
| Deployments | `src/controller.rs` |
| Tests | `tests/run-tests.sh`, `tests/*.yaml` |

---

## Recommended direction (priority)

### Do first (product, not tests)

1. **Revert** manifest/test port and `/tmp/data` hacks (restore nginx :80, `/var/data`, etc.).
2. **Phase 6 slice:** `hostNetwork: false` + pasta (or interim: supervisor **published backend** map per pod/containerPort for proxy only).
3. **Wire Phase 3 proxy** to **per-pod backend** from (2) — never assume all pods share `127.0.0.1:containerPort`.
4. **Conditional `CLONE_NEWPID`** + document degraded vs full profile in ROADMAP.
5. **Phase 2:** PVC volume resolution; real emptyDir bind strategy in degraded mode (no YAML path change).
6. **Exec:** one supported model (setns + in-rootfs paths or documented `chroot` helper).

### Do not do again

- Renumber `containerPort` in test YAML to pass service tests.
- Mark PV mount “pass” without `persistentVolumeClaim` in `volumes.rs`.
- Claim phases “complete” when pivot fails on the main dev path.

### Documentation

- Update **ROADMAP.md** “Current State” (test count, `127.96`, degraded profile, NEWPID).
- Keep this file as the honest cross-check against phases.

---

## Verification checklist

- [ ] `cargo build` · `./z8s.sh restart`
- [ ] whoami/http-echo Ready; no `newosproc` in logs
- [ ] `curl` ClusterIP for whoami-svc (both pods on **containerPort 80** without YAML renumber)
- [ ] `grep 'listening on 127.96' /tmp/z8s.log`
- [ ] `kubectl exec` stable (no 1006)
- [ ] emptyDir at **`/var/data`** (or documented profile), not only `/tmp/data`
- [ ] Full suite: `./tests/run-tests.sh 2>&1 | tee tests/report4.md`

---

## Open questions

1. Phase 6 first, or minimal **published-port registry** before pasta?
2. Does NEWPID work on this host when pivot succeeds (root / PID 1)?
3. Target ClusterIP range: keep `127.96.x.x` or move toward `10.96.0.0/12`?
4. OpenAPI: fix for unmodified `kubectl apply`?

---

*Honest review document. Supersedes earlier optimism about “proxy not implemented” and conflates test hacks with phase completion.*
