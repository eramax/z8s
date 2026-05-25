# z8s

**z8s** (pronounced "zetes") is a minimal, self-contained Kubernetes-compatible orchestrator and init process written in Rust. It combines the role of a PID 1 init system (like s6 or runit) with a kubectl-compatible API server (like k3s), letting you manage workloads using standard `kubectl` commands on a single Linux machine — no cluster, no etcd, no kubelet.

```
┌─────────────────────────────────────────────────────┐
│                     z8s process                      │
│                                                     │
│  ┌───────────────┐  ┌───────────────────────────┐   │
│  │  Init (PID 1) │  │   k8s API server (:6443)  │   │
│  │  SIGCHLD reap │  │   REST + WebSocket        │   │
│  │  signal fwd   │  │   kubectl-compatible      │   │
│  └───────────────┘  └───────────────────────────┘   │
│                                                     │
│  ┌───────────────────────────────────────────────┐  │
│  │             Process Supervisor                │  │
│  │  Pods · Deployments · Health probes           │  │
│  │  OCI image pull · chroot execution            │  │
│  │  cgroups v2 resource limits                   │  │
│  └───────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────┘
```

## Features

| Feature | Details |
|---|---|
| **kubectl-compatible API** | Full REST API on port 6443 — works with unmodified `kubectl` |
| **Pod lifecycle** | Create, list, describe, delete, watch pod phase |
| **Deployments** | Create, scale (`kubectl scale`), describe, delete |
| **Exec** | `kubectl exec` with PTY (`-it`) and pipe (`-i`) support via WebSocket |
| **Logs** | `kubectl logs` from in-memory ring buffer (last 1000 lines) |
| **Namespaces** | Full namespace CRUD |
| **Nodes** | Reports the host as a single node with real CPU/memory capacity |
| **Events** | Pod lifecycle events via `kubectl get events` |
| **Metrics** | `kubectl top pod/node` backed by cgroup v2 stats |
| **OCI images** | Pull from any OCI-compatible registry; layer cache at `/var/lib/z8s` |
| **Native processes** | `image: ""` runs a host binary directly without any container isolation |
| **chroot containers** | Non-empty image field pulls OCI layers and chroots the process |
| **Manifest watcher** | Drops YAML files into `/etc/z8s/manifests/` — they are applied automatically |
| **Health probes** | `livenessProbe`, `readinessProbe`, `startupProbe` (exec, httpGet, tcpSocket) |
| **cgroups v2** | Optional memory/CPU limits applied when running as root |
| **PID 1 init** | Zombie reaping, signal forwarding when running as PID 1 in a container |
| **Protobuf bodies** | Accepts `kubectl create` protobuf-encoded requests transparently |

## Quick start

### Build

```bash
cargo build --release
```

The release binary is ~4 MB, statically optimized (`opt-level = "z"`, `lto = true`, `strip = true`).

### Run (development)

```bash
./z8s.sh start       # starts target/debug/z8s, logs to /tmp/z8s.log
./z8s.sh status
./z8s.sh restart
./z8s.sh stop
```

### Run (production / installed)

```bash
./install.sh                              # builds release, copies to /usr/local/bin
sudo z8s-daemon start
```

Runs on port **6443**. No TLS — use `kubectl --insecure-skip-tls-verify` or put a TLS terminator in front.

### Use with kubectl

```bash
export KUBECONFIG=/dev/null
alias k='kubectl --server=http://localhost:6443 --insecure-skip-tls-verify'

k get nodes
k get pods -A
k apply --validate=false -f my-app.yaml
k exec -it my-pod -- bash
k logs my-pod
k top pod
```

## Architecture

### Components

```
src/
├── main.rs               Entry point; wires all components together
├── init.rs               PID 1 init: SIGCHLD zombie reaping, signal forwarding
├── controller.rs         DeploymentController: reconcile loop for replica counts
├── api/
│   ├── mod.rs            AnyResource enum (Pod | Deployment)
│   └── types.rs          ResourceStore, ResourceTracker, ResourceState, YAML parsing
├── server/
│   ├── api.rs            Axum HTTP handlers for all k8s API endpoints
│   ├── exec.rs           kubectl exec — WebSocket PTY + pipe execution
│   └── proto.rs          k8s protobuf body decoder (for kubectl create)
├── supervisor/
│   ├── process.rs        ProcessSupervisor: spawn/stop/reconcile containers
│   ├── cgroup.rs         cgroups v2 resource limits (gracefully degrades without root)
│   └── health.rs         Health probes: exec, httpGet, tcpSocket
├── container/
│   ├── image.rs          OCI image pull, layer extraction, rootfs cache
│   └── rootfs.rs         chroot/namespace helpers (for OCI containers)
├── manifest/
│   └── watcher.rs        inotify-based watcher for /etc/z8s/manifests/
└── builder/              Rust builder API for constructing k8s resources
    ├── pod.rs
    ├── deployment.rs
    ├── container.rs
    └── manifest.rs
```

### Data flow

```
kubectl apply -f pod.yaml
     │
     ▼
POST /api/v1/namespaces/default/pods    ← Axum HTTP handler
     │
     ▼
ResourceStore.apply(Pod)               ← in-memory store (Arc<RwLock<Vec<ResourceTracker>>>)
     │
     ▼
ProcessSupervisor.start_pod()
     ├── ImageManager.unpack_image()   ← pull + extract OCI layers to /var/lib/z8s/rootfs/
     ├── CgroupManager.create_pod_cgroup()
     ├── tokio::process::Command::spawn()
     └── ResourceState → Running
```

### State machine

Each resource has a `ResourceState`:

```
Pending ──► Running ──► Terminated (deleted)
   │                  ╲
   └──────────────────► Failed(reason)
```

The `ProcessSupervisor.reconcile()` loop (every 10 s) restarts any Pending pods that aren't running. The `DeploymentController.run()` loop (every 15 s) ensures the correct number of replica pods exist.

## API endpoints

### Core (v1)

| Method | Path | Description |
|---|---|---|
| GET | `/api` | API versions |
| GET | `/api/v1` | Resource list |
| GET | `/api/v1/pods` | List all pods |
| GET/POST | `/api/v1/namespaces/{ns}/pods` | List / create pods |
| GET/PATCH/PUT/DELETE | `/api/v1/namespaces/{ns}/pods/{name}` | Get / update / delete pod |
| GET | `/api/v1/namespaces/{ns}/pods/{name}/log` | Pod logs |
| GET | `/api/v1/namespaces/{ns}/pods/{name}/exec` | Exec (WebSocket upgrade) |
| GET/POST | `/api/v1/namespaces` | List / create namespaces |
| GET/DELETE | `/api/v1/namespaces/{name}` | Get / delete namespace |
| GET | `/api/v1/nodes` | List nodes |
| GET | `/api/v1/nodes/{name}` | Get node |
| GET | `/api/v1/events` | All events |
| GET | `/api/v1/namespaces/{ns}/events` | Namespace events |
| GET | `/api/v1/configmaps` `/secrets` | Stub (returns empty list) |

### Apps (v1)

| Method | Path | Description |
|---|---|---|
| GET/POST | `/apis/apps/v1/namespaces/{ns}/deployments` | List / create |
| GET/PATCH/PUT/DELETE | `/apis/apps/v1/namespaces/{ns}/deployments/{name}` | Get / update / delete |
| PATCH | `/apis/apps/v1/namespaces/{ns}/deployments/{name}/scale` | Scale replicas |

### Metrics

| Method | Path | Description |
|---|---|---|
| GET | `/apis/metrics.k8s.io/v1beta1/nodes` | Node CPU/memory (`kubectl top node`) |
| GET | `/apis/metrics.k8s.io/v1beta1/pods` | Pod CPU/memory (`kubectl top pod`) |
| GET | `/apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods` | Namespace pod metrics |

### Discovery / health

| Path | Description |
|---|---|
| `/apis` | API group list |
| `/openapi/v2`, `/openapi/v3` | OpenAPI schema stubs |
| `/version` | Server version |
| `/healthz`, `/readyz`, `/livez` | Health checks |

## Pod spec

z8s supports a subset of the standard Kubernetes Pod spec.

### Native process (no image)

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: my-service
  namespace: default
spec:
  containers:
  - name: main
    image: ""               # empty → run directly on host
    command: ["/usr/bin/my-service"]
    args: ["--port=8080"]
    env:
    - name: LOG_LEVEL
      value: info
```

### OCI container (chroot)

```yaml
apiVersion: v1
kind: Pod
metadata:
  name: nginx
spec:
  containers:
  - name: web
    image: docker.io/library/nginx:latest
    command: ["nginx", "-g", "daemon off;"]
    resources:
      requests:
        memory: "64Mi"
      limits:
        memory: "128Mi"
        cpu: "500m"
```

> **Note:** `chroot` requires root. Running z8s as a non-root user automatically disables chroot and cgroup enforcement; native processes (`image: ""`) still work.

### Health probes

```yaml
spec:
  containers:
  - name: app
    image: ""
    command: ["/usr/bin/app"]
    livenessProbe:
      exec:
        command: ["/bin/test", "-f", "/tmp/healthy"]
      initialDelaySeconds: 5
      periodSeconds: 10
    readinessProbe:
      httpGet:
        path: /healthz
        port: 8080
      initialDelaySeconds: 2
      periodSeconds: 5
    startupProbe:
      tcpSocket:
        port: 8080
      failureThreshold: 30
      periodSeconds: 1
```

## Deployment spec

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: my-app
  namespace: default
spec:
  replicas: 3
  selector:
    matchLabels:
      app: my-app
  template:
    metadata:
      labels:
        app: my-app
    spec:
      containers:
      - name: worker
        image: ""
        command: ["/usr/bin/worker"]
```

Scale at any time:

```bash
kubectl --server=http://localhost:6443 --insecure-skip-tls-verify \
  scale deployment my-app --replicas=5
```

## Manifest watcher

Place YAML files in `/etc/z8s/manifests/` and z8s applies them at startup and whenever a file changes:

```bash
sudo mkdir -p /etc/z8s/manifests
sudo cp my-service.yaml /etc/z8s/manifests/
# z8s detects the file and starts the pod automatically
```

Supports multi-document YAML (separated by `---`). Supports both Pod and Deployment kinds.

## exec

```bash
# Interactive shell (PTY)
kubectl exec -it my-pod -- bash

# Non-interactive command
kubectl exec my-pod -- /bin/echo hello

# Pipe stdin
echo "ls /" | kubectl exec -i my-pod -- /bin/sh
```

For OCI containers, exec chroots into the container's rootfs. For native processes (`image: ""`), exec runs directly on the host. Both modes use the k8s WebSocket exec protocol (v4/v5 channel multiplexing).

## Resource limits (cgroups v2)

When running as root with cgroups v2 available:

```yaml
resources:
  requests:
    memory: "64Mi"     # sets memory.low
  limits:
    memory: "128Mi"    # sets memory.max
    cpu: "500m"        # sets cpu.max (500000 50000)
```

cgroup hierarchy: `/sys/fs/cgroup/z8s/<pod-uid>/`

When not running as root, resource limits are silently skipped; everything else functions normally.

## OCI image cache

Images are pulled from the registry once and cached:

| Path | Contents |
|---|---|
| `/var/lib/z8s/images/` | Raw layer tarballs (keyed by image ref hash) |
| `/var/lib/z8s/rootfs/` | Extracted rootfs directories (`<name>-<container>/`) |

To force a re-pull, delete the relevant directory and restart the pod.

## Daemon management

### Development

```bash
./z8s.sh {start|stop|restart|status}
```

- Binary: `target/debug/z8s`
- Log: `/tmp/z8s.log`
- PID file: `/tmp/z8s.pid`

### Production

```bash
sudo z8s-daemon {start|stop|restart|status}
```

- Binary: `/usr/local/bin/z8s`
- Log: `/var/log/z8s/z8s.log`
- PID file: `/var/run/z8s.pid`

## Running as PID 1

When z8s is started as PID 1 (inside a container or VM), it activates full init mode:

- Blocks SIGCHLD, SIGTERM, SIGINT, SIGHUP, SIGUSR1, SIGUSR2 via `signalfd`
- Reaps zombie children automatically via `waitpid(WNOHANG)`
- Forwards SIGTERM → graceful shutdown of all managed pods
- Broadcasts SIGTERM to all process groups on shutdown

```dockerfile
FROM scratch
COPY z8s /z8s
COPY manifests/ /etc/z8s/manifests/
ENTRYPOINT ["/z8s"]
```

## Testing

```bash
# Build debug binary
cargo build

# Run the integration test suite (starts z8s via z8s.sh, leaves it running)
./test.sh

# The test suite covers:
#   Server startup · Discovery · Namespaces · Nodes
#   Pod lifecycle (create/ready/logs/delete)
#   Exec (echo, ls, stdin pipe)
#   Deployments (create/list/scale/describe/delete)
#   Events · ConfigMaps/Secrets · Cross-namespace listing
```

The test script uses `kubectl` against the live server and validates API responses. It requires `kubectl` to be in `PATH`.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |
| `Z8S_SERVER` | `http://localhost:6443` | Override for test.sh |

## Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime |
| `axum 0.8` | HTTP + WebSocket server |
| `k8s-openapi 0.27 (v1_35)` | Typed k8s API structs |
| `nix 0.31` | PTY, signals, cgroups, mounts |
| `oci-distribution` | OCI registry client |
| `flate2` + `tar` | Layer extraction |
| `serde_json` + `serde_yaml` | Serialization |
| `notify` | inotify-based manifest watcher |
| `tracing` | Structured logging |
| `uuid` | Resource UID generation |
| `chrono` | Timestamps |
| `futures-util` | WebSocket sink/stream |
| `anyhow` | Error handling |

## Limitations

- **Single node only** — no clustering or networking between nodes
- **In-memory state** — all resource state is lost on restart
- **No RBAC / auth** — all requests are accepted
- **chroot requires root** — OCI containers need root; native processes work as any user
- **No service mesh / DNS** — no in-cluster DNS or service proxy
- **No persistent volumes** — no volume mounts or storage classes
- **No init containers** — `initContainers` field is not processed

## License

MIT
