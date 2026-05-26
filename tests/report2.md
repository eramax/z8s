# z8s Test Suite Report v2

**Run:** `tests/run-tests.sh` — 20 sections  
**Results:** **142 passed, 18 failed**  
**Date:** 2026-05-26  
**Script:** 1529 lines, 14 YAML manifest files  
**Server:** `http://localhost:6443` (dev mode, `./z8s.sh`)

---

## Summary

| Metric | Count |
|--------|-------|
| Total tests | 160 |
| Passed | 142 |
| Failed | 18 |
| Distinct bugs found | 7 |
| Features working | 10+ |

---

## Bugs Found

### B1. `securityContext` not enforced
**Severity:** High  
**Evidence:** Pods with `runAsUser: 1000`, `runAsNonRoot: true` run as `uid=0(root)`.  
**Affects:** `04-pod-ubuntu.yaml`, `08-deployment-ubuntu.yaml`, `05-pod-python.yaml`  
**Test output:**
```
FAIL ubuntu (z8s-test): expected uid=1000 but got uid=0
```
**Root cause:** User namespace is created but `runAsUser` is never applied to `unshare` or `execvpe`.

---

### B2. Exec WebSocket protocol errors
**Severity:** High  
**Evidence:** Simple stdout commands return `unexpected message type: 1`.  
**Affects:** All `kubectl exec` calls. `curl`, `wget`, `sh` work; `hostname`, `env`, `ip addr`, `cat` break.  
**Root cause:** `src/server/exec.rs` sends stdout on the wrong WebSocket stream channel.

---

### B3. ConfigMap/Secret volumes not mounted
**Severity:** Medium  
**Evidence:** Bind-mount fails with `EACCES: Permission denied`.
```
WARN volumes: Failed to bind-mount .../configmaps/... → /etc/...: EACCES
cat: /mnt/config/GREETING: No such file or directory
```
**Root cause:** `mount MS_PRIVATE` is denied in this environment; chroot fallback skips bind mounts.

---

### B4. Service proxy not routing traffic
**Severity:** High  
**Evidence:** All 6 services have ClusterIPs but `wget` from a client pod times out.  
**Affected:** python-svc, nginx-svc, whoami-svc, http-echo-svc, hostinfo-svc, nginx-hello-svc  
**Test output:**
```
FAIL svc: python HTTP server — no valid response after 3 tries (clusterIP=10.96.0.3, port=8080)
```
**Root cause:** ClusterIP → pod routing is not implemented or the proxy isn't forwarding.

---

### B5. `kubectl apply secret` returns empty body
**Severity:** Low  
**Evidence:** Creating a secret via `kubectl apply` succeeds but the API returns empty response.  
**Test output:**
```
FAIL kubectl apply secret: got ''
```

---

### B6. Label selectors not supported
**Severity:** Low  
**Evidence:** `kubectl get pods -l app=alpine` returns ALL pods unfiltered.  
**Test output:**
```
FAIL label selector: (shows ALL pods including non-matching)
```

---

### B7. Pod listing returns incomplete results mid-scale
**Severity:** Low  
**Evidence:** After scaling to 100 pods, `kubectl get pods` sometimes shows 0 pods briefly.  
**Root cause:** Pod listing and deployment controller reconciliation race.

---

## Fixed Since v1

| Bug | Status | Detail |
|-----|--------|--------|
| `HOME` env var empty | ✅ Fixed | `HOME=/` is now set inside containers |
| Same-name resources collide across namespaces | ✅ Fixed | ConfigMap/Secret/Service isolation works |
| Deployment controller drops `container.env` | ✅ Fixed | Direct env vars now pass through to pods |
| Service API missing printer columns | ✅ Fixed | `kubectl get services` shows TYPE/CLUSTER-IP/PORT(S) correctly |
| `kubectl create --from-literal` | ✅ Fixed | Imperative create with `--from-literal` now works |

---

## Deployment Readiness

| Deployment | Replicas | Ready | Notes |
|------------|----------|-------|-------|
| alpine-deploy | 2 | ✅ | Sleep infinity |
| ubuntu-deploy | 1 | ✅ | z8s-test namespace |
| python-deploy | 2 | ✅ | HTTP server on :8080 |
| postgres-deploy | 1 | ✅ | PostgreSQL 16 |
| nginx-deploy | 2 | ✅ | nginx:alpine |
| whoami | 1 | ✅ | traefik/whoami |
| http-echo | 1 | ✅ | hashicorp/http-echo |
| hostinfo | 1 | ✅ | maximleus/hostinfo |
| cluster-dashboard | 1 | ✅ | eramax/cluster-dashboard |
| logger-deploy | 2 | ✅ | Log generator |
| nginx-hello | 1 | ❌ | nginxdemos/hello — pod crashes immediately |

---

## Commands Verified

| Command | Status | Notes |
|---------|--------|-------|
| `kubectl apply -f` | ✅ Works | YAML apply |
| `kubectl create configmap --from-literal` | ✅ Fixed | Now works |
| `kubectl create secret generic --from-literal` | ⚠️ | Creates but returns empty body |
| `kubectl get` | ✅ Works | `-o json`, `-o yaml`, `-o jsonpath`, `--all-namespaces` |
| `kubectl describe` | ✅ Works | Pods, deployments, services, configmaps, secrets, nodes |
| `kubectl delete` | ✅ Works | Verify with `get` after delete |
| `kubectl logs` | ✅ Works | stdout, stderr, timestamps |
| `kubectl exec` | ⚠️ | See B2 — simple commands fail |
| `kubectl scale` | ✅ Works | 2→4→1, 2→3, 2→100 |
| `kubectl expose` | ✅ Works | Creates service with assigned ClusterIP |

---

## Scale Performance

| Operation | Time |
|-----------|------|
| Alpine deploy 2→4 | 5s |
| Alpine deploy 4→1 | 0s |
| Python deploy 2→3 | 16s |
| **Alpine deploy 2→100** | **15s** |
| Alpine deploy 100→1 | 0s |
| Pods confirmed in listing | 100/100 |

---

## Volume Behavior

| Volume Type | Works? | Notes |
|-------------|--------|-------|
| `emptyDir` | ✅ | Write, read, ephemeral (data lost on pod delete) |
| `hostPath` | ❌ | Bind-mount denied (EACCES) |
| `configMap` | ❌ | Not mounted (same EACCES issue) |
| `secret` | ❌ | Not mounted (same EACCES issue) |

---

## Security Isolation

| Test | Result |
|------|--------|
| PID namespace isolation | ✅ Container has own /proc/1 |
| /proc write protection | ✅ Cannot write to /proc/sys |
| Non-root file access | ✅ Blocked on /etc |
| Internet connectivity | ✅ 1.1.1.1 reachable |
| Stdin pipe exec | ✅ `echo cmd \| k exec -i` |
| /etc/shadow | ✅ Container has own passwd |

---

## Service Env Injection

Service env vars are injected into all pods correctly:

| Var | Value | Verified |
|-----|-------|----------|
| `PYTHON_SVC_SERVICE_HOST` | 10.96.0.3 | ✅ |
| `NGINX_SVC_SERVICE_HOST` | 10.96.0.6 | ✅ |
| `POSTGRES_SVC_SERVICE_HOST` | 10.96.0.5 | ✅ |
| `NGINX_HELLO_SVC_SERVICE_HOST` | 10.96.0.9 | ✅ |
| `WHOAMI_SVC_SERVICE_HOST` | 10.96.0.10 | ✅ |
| `HOSTINFO_SVC_SERVICE_HOST` | 10.96.0.12 | ✅ |
| `CLUSTER_DASHBOARD_SVC_SERVICE_HOST` | 10.96.0.13 | ✅ |
| `HTTP_ECHO_SVC_SERVICE_HOST` | 10.96.0.11 | ✅ |

---

## Quick Commands

```bash
# Run full test suite
./tests/run-tests.sh

# Run individual tests
kubectl --server=http://localhost:6443 apply --validate=false -f tests/
kubectl --server=http://localhost:6443 get pods
kubectl --server=http://localhost:6443 logs logger-pod
```

---

## Test YAML Files

| File | Resources |
|------|-----------|
| `00-namespace.yaml` | z8s-test, z8s-prod namespaces |
| `01-configmap.yaml` | app-config (default+z8s-test), nginx-config |
| `02-secret.yaml` | app-secret (default+z8s-test) |
| `03-pod-alpine.yaml` | Alpine pod with volumes, env, envFrom |
| `04-pod-ubuntu.yaml` | Ubuntu pod (non-root), hostPath |
| `05-pod-python.yaml` | Python HTTP server pod |
| `06-pod-postgres.yaml` | PostgreSQL pod |
| `07-deployment-alpine.yaml` | Alpine deployment (2 replicas) |
| `08-deployment-ubuntu.yaml` | Ubuntu deployment in z8s-test |
| `09-deployment-python.yaml` | Python deployment (2 replicas) |
| `10-deployment-postgres.yaml` | PostgreSQL deployment |
| `11-service.yaml` | Services for all apps |
| `12-deployment-nginx.yaml` | Nginx deployment |
| `13-deployment-info.yaml` | Info images + services |
| `14-pod-logger.yaml` | Log-generating pod + deployment |
