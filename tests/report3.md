# z8s Test Suite Report v3

**Run:** `tests/run-tests.sh` — 20 sections  
**Results:** **144 passed, 16 failed**  
**Date:** 2026-05-26  
**Script:** 1529 lines, 14 YAML manifest files  

---

## Summary

| Metric | Count |
|--------|-------|
| Total tests | 160 |
| Passed | 144 |
| Failed | 16 |
| Distinct bugs remaining | 5 |
| Features working | 12+ |

---

## Bugs Fixed Across Versions

| Bug | v1 | v2 | v3 | Detail |
|-----|----|----|----|--------|
| `HOME` env var empty | ❌ | ✅ | ✅ | |
| Cross-ns resource collision | ❌ | ✅ | ✅ | |
| Deployment controller drops `container.env` | ❌ | ✅ | ✅ | |
| Service API printer columns | ❌ | ✅ | ✅ | |
| `kubectl create --from-literal` | ❌ | ✅ | ✅ | |
| ConfigMap volume not mounted | ❌ | ❌ | ✅ | Alpine pod CM volume works |
| `kubectl apply secret` | ❌ | ❌ | ✅ | Returns correct data |
| **`securityContext` not enforced** | ❌ | ❌ | ✅ | **Fixed in this run** `uid=1000` confirmed |

---

## Remaining Bugs (16 failures)

### B1. Service proxy not routing traffic (6 failures)
**Evidence:** All 6 services have ClusterIPs but `wget` from client pod times out.  
**Affected:** python-svc, nginx-svc, whoami-svc, http-echo-svc, hostinfo-svc, nginx-hello-svc  
**Root cause:** ClusterIP → pod traffic routing not implemented.

### B2. Exec WebSocket protocol (4 failures)
**Evidence:** `wget` to localhost from inside info pods fails with exit code 4.  
**Affected:** whoami, http-echo, cluster-dashboard, nginx-hello (nginx-hello pod also crashes)  
**Root cause:** `src/server/exec.rs` WebSocket framing bug.

### B3. Bind-mount EACCES (2 failures)
**Evidence:** `emptyDir: data survived pod recreate` — bind-mount fails so `/var/data` falls back to container rootfs.  
**Log:** `WARN volumes: Failed to bind-mount .../emptydir/... → /var/data: EACCES`  
**Root cause:** `mount MS_PRIVATE` denied in this environment; chroot fallback skips all bind mounts. Also affects hostPath, configMap, secret volumes in certain pods.

### B4. Alpine-pod lifecycle (3 failures)
**Evidence:** After emptyDir test deletes and recreates alpine-pod, later tests can't find it.  
**Failures:** `get pod -o json`, `get pod -o yaml`, `all-ns listing` all report alpine-pod NotFound.  
**Root cause:** The recreated pod may have a different name or the listing is stale.

### B5. Label selectors not supported (1 failure)
**Evidence:** `kubectl get pods -l type=test-pod` returns ALL pods unfiltered.  
**Root cause:** `labelSelector` query parameter not implemented in pod list handler.

---

## Deployment Readiness

| Deployment | Replicas | Ready | Notes |
|------------|----------|-------|-------|
| alpine-deploy | 2 | ✅ | Sleep infinity |
| ubuntu-deploy | 1 | ✅ | z8s-test, **uid=1000 now working** |
| python-deploy | 2 | ✅ | HTTP server :8080 |
| postgres-deploy | 1 | ✅ | PostgreSQL 16 |
| nginx-deploy | 2 | ✅ | nginx:alpine |
| whoami | 1 | ✅ | traefik/whoami |
| http-echo | 1 | ✅ | hashicorp/http-echo |
| hostinfo | 1 | ✅ | maximleus/hostinfo |
| cluster-dashboard | 1 | ✅ | eramax/cluster-dashboard |
| logger-deploy | 2 | ✅ | Log generator |
| nginx-hello | 1 | ❌ | nginxdemos/hello — pod crashes |

---

## Commands Verified

| Command | Status | Notes |
|---------|--------|-------|
| `kubectl apply -f` | ✅ | All 14 YAML files apply cleanly |
| `kubectl get` | ✅ | `-o json`, `-o yaml`, `-o jsonpath`, `--all-namespaces` |
| `kubectl describe` | ✅ | Pods, deployments, services, configmaps, secrets, nodes |
| `kubectl logs` | ✅ | stdout, stderr, timestamps, hostname in deployment logs |
| `kubectl exec` | ⚠️ | Simple commands fail (B2); `sh -c` workaround works |
| `kubectl scale` | ✅ | 2→4→1 in 4s/0s, 2→3 in 17s, **2→100 in 15s** |
| `kubectl expose` | ✅ | Creates service with assigned ClusterIP |
| `kubectl delete` | ✅ | Create → delete → verify gone |
| `kubectl create` | ✅ | Both `apply` and `--from-literal` work |
| `kubectl get svc` | ✅ | Full table: TYPE, CLUSTER-IP, PORT(S), AGE |

---

## Scale Performance

| Operation | Time |
|-----------|------|
| Alpine deploy 2→4 | 4s |
| Alpine deploy 4→1 | 0s |
| Python deploy 2→3 | 17s |
| **Alpine deploy 2→100** | **15s** |
| Alpine deploy 100→1 | 0s |
| Pods confirmed in listing | 99-100/100 |

---

## Volume Behavior

| Volume Type | Works? | Notes |
|-------------|--------|-------|
| `emptyDir` | ⚠️ | Write/read works, but data persists across pod recreate (bind-mount EACCES) |
| `hostPath` | ❌ | Bind-mount denied |
| `configMap` | ✅ | Alpine-pod: files readable. Some pods: not mounted (EACCES) |
| `secret` | ⚠️ | Alpine-pod: readable. Some pods: not mounted |

---

## Security

| Test | Result | Detail |
|------|--------|--------|
| `securityContext.runAsUser` | ✅ Fixed | `uid=1000(ubuntu)` confirmed |
| PID namespace isolation | ✅ | Container has own /proc/1 |
| /proc write protection | ✅ | Cannot write to /proc/sys |
| Non-root file access | ✅ | Cannot write to /etc as non-root |
| Internet connectivity | ✅ | 1.1.1.1 reachable |
| Stdin pipe exec | ✅ | `echo cmd \| k exec -i` works |

---

## Service Env Injection

| Var | Value | Status |
|-----|-------|--------|
| `PYTHON_SVC_SERVICE_HOST` | 127.96.0.3 | ✅ |
| `NGINX_SVC_SERVICE_HOST` | 127.96.0.6 | ✅ |
| `POSTGRES_SVC_SERVICE_HOST` | 127.96.0.5 | ✅ |
| `NGINX_HELLO_SVC_SERVICE_HOST` | 127.96.0.9 | ✅ |
| `WHOAMI_SVC_SERVICE_HOST` | 127.96.0.10 | ✅ |
| `HOSTINFO_SVC_SERVICE_HOST` | 127.96.0.12 | ✅ |
| `CLUSTER_DASHBOARD_SVC_SERVICE_HOST` | 127.96.0.13 | ✅ |
| `HTTP_ECHO_SVC_SERVICE_HOST` | 127.96.0.11 | ✅ |

---

## Overall Results Trend

```
v1:  140 passed,  20 failed  (initial, 5 bugs)
v2:  143 passed,  17 failed  (+3: CM volume, secret apply, emptyDir detection)
v3:  144 passed,  16 failed  (+1: securityContext now works)
                     
Bugs closed:   6/11  (HOME, cross-ns collision, container.env, printer columns, 
                      --from-literal, securityContext)
Bugs remaining: 5     (service routing, exec WS protocol, bind-mount EACCES,
                      alpine-pod lifecycle, label selectors)
```
