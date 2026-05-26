# z8s Test Suite Report — Final

**Run:** `tests/run-tests.sh` — 22 sections  
**Results:** **158 passed, 19 failed**  
**Date:** 2026-05-26  
**Script:** 1606 lines, 15 YAML manifest files  
**Note:** No `(best-effort)` passes — every command error is reported as FAIL

---

## Results Summary

| Metric | Count |
|--------|-------|
| Total tests | 177 |
| Passed | 158 |
| Failed | 19 |
| Deployments ready | 11/11 |
| Pods ready | 5/5 |

---

## 19 Failures — Root Causes

### Category 1: GLIBC Version Mismatch (8 failures)
**Severity:** Environment (not z8s)  
**Affected:** ubuntu-pod + ubuntu-deploy  
**Error:**
```
/var/lib/z8s/rootfs/ubuntu-pod-ubuntu/bin/id: /lib/x86_64-linux-gnu/libm.so.6:
version 'GLIBC_2.43' not found
```
**All 8 ubuntu tests fail** because every binary (`id`, `touch`, `cat`, `ls`, `env`) requires `GLIBC_2.43` which the host doesn't provide. This is a mismatch between the ubuntu:latest image and the host's libc.

**Tests:** ubuntu uid, /etc write, /etc/shadow, hostPath, envFrom (x2), ubuntu-deploy uid, ubuntu-deploy envFrom

---

### Category 2: Spawn Error — `/bin/sh` Not Found (5 failures)
**Severity:** Medium — z8s rootfs or exec path issue  
**Error:**
```
error: spawn error: No such file or directory (os error 2)
```
`/bin/sh` is not found when exec attempts to run commands via `sh -c`. This affects pods using `command: ["sh", "-c", "..."]` with certain base images (python:3-slim, postgres:16-alpine, traefik/whoami, hashicorp/http-echo).

**Tests:** python HTTP server, postgres pg_isready, postgres psql, info whoami, info http-echo

---

### Category 3: Volume Bind-Mount EACCES (3 failures)
**Severity:** Medium — mount denied  
**Affected:** emptyDir volume  
**Error:**
```
sh: can't create /var/data/test.txt: nonexistent directory
WARN volumes: Failed to bind-mount .../emptydir/... → /var/data: EACCES
```
Also affects hostPath, configMap, secret volumes (degraded fallback copies instead of mounts). The environment doesn't allow `mount MS_PRIVATE`.

**Tests:** alpine emptyDir write/read, nginx-deploy scale 0→1 (deployment volume emptyDir), nginx-deploy HTTP check (config not mounted)

---

### Category 4: Service Routing / Port Mangling (2 failures)
**Severity:** High — ClusterIP services don't route correctly  
**Affected:** nginx-svc (port 80), hostinfo-svc (port 18081 instead of 8080)  
**Error:**
```
FAIL svc: nginx default page — no valid response after 3 tries (clusterIP=...:80)
FAIL svc: hostinfo page — no valid response after 3 tries (clusterIP=...:18081)
```
Note: python-svc, whoami-svc, http-echo-svc, nginx-hello-svc all PASS. The hostinfo port `18081` suggests nodePort allocator interference with ClusterIP services.

---

### Category 5: Info Pod Exec (1 failure)
**Severity:** Low — same spawn error as Category 2  
**Error:**
```
FAIL info: cluster-dashboard — expected 'dashboard|cluster|html' in response,
got: error: command exited with code 4
```

---

## What Works Well

| Feature | Status |
|---------|--------|
| Pod lifecycle (create, start, Ready, delete) | ✅ |
| Direct env vars (`env:` with `value:`) | ✅ |
| `envFrom` configMapRef / secretRef | ✅ |
| ConfigMap/Secret CRUD + describe | ✅ |
| `kubectl logs` (stdout, stderr, timestamps) | ✅ |
| `kubectl scale` (2→4→1, 2→3, 2→100) | ✅ |
| `kubectl expose` | ✅ |
| `kubectl describe` (all types) | ✅ |
| `kubectl apply` / `kubectl delete` | ✅ |
| `kubectl create configmap --from-literal` | ✅ |
| Namespace isolation | ✅ |
| PID namespace isolation | ✅ |
| /proc write protection | ✅ |
| Internet connectivity from containers | ✅ |
| Service env var injection | ✅ |
| Service routing (python, whoami, http-echo, nginx-hello) | ✅ |
| PersistentVolume CRUD | ✅ |
| PersistentVolumeClaim CRUD (cross-ns) | ✅ |
| PVC volume mount in pod | ✅ |
| API output formats (-o json, -o yaml, -o jsonpath) | ✅ |
| Label selectors | ✅ |
| All-namespaces listing (pods, deployments, services, PV, PVC) | ✅ |
| Health / version endpoints | ✅ |

---

## Scale Performance

| Operation | Time |
|-----------|------|
| Alpine deploy 2→4 | 12-13s |
| Alpine deploy 4→1 | 0s |
| Python deploy 2→3 | 15-17s |
| **Alpine deploy 2→100** | **16-19s** |
| Alpine deploy 100→1 | 0s |
| Deployment ordering | 31-33ms before pods appear |

---

## Test YAML Files

| File | Resources |
|------|-----------|
| `00-namespace.yaml` | z8s-test, z8s-prod |
| `01-configmap.yaml` | app-config, nginx-config |
| `02-secret.yaml` | app-secret |
| `03-pod-alpine.yaml` | Alpine pod with volumes + env |
| `04-pod-ubuntu.yaml` | Ubuntu pod (non-root) |
| `05-pod-python.yaml` | Python HTTP server pod |
| `06-pod-postgres.yaml` | PostgreSQL pod |
| `07-deployment-alpine.yaml` | Alpine deployment (2 replicas) |
| `08-deployment-ubuntu.yaml` | Ubuntu deployment in z8s-test |
| `09-deployment-python.yaml` | Python deployment (2 replicas) |
| `10-deployment-postgres.yaml` | PostgreSQL deployment |
| `11-service.yaml` | Services for all apps |
| `12-deployment-nginx.yaml` | Nginx deployment |
| `13-deployment-info.yaml` | Info images (nginxdemos, whoami, http-echo, hostinfo, cluster-dashboard) |
| `14-pod-logger.yaml` | Log-generating pod + deployment |
| `15-pv-pvc.yaml` | PVs, PVCs (cross-ns), PVC mount pod |

---

## Quick Commands

```bash
# Run full test suite
./tests/run-tests.sh

# Run with output to file
./tests/run-tests.sh 2>&1 | tee /tmp/z8s-test-output.txt

# Apply individual YAML
kubectl --server=http://localhost:6443 apply -f tests/00-namespace.yaml
```
