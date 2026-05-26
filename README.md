# z8s

**z8s** (pronounced "zetes") is a minimal, self-contained Kubernetes-compatible orchestrator and init process written in Rust. It combines the role of a PID 1 init system (like s6 or runit) with a kubectl-compatible API server (like k3s), letting you manage workloads using standard `kubectl` commands on a single Linux machine — no cluster, no etcd, no kubelet.

```
┌─────────────────────────────────────────────────────────┐
│                     z8s process                          │
│                                                         │
│  ┌───────────────┐  ┌───────────────────────────────┐   │
│  │  Init (PID 1) │  │   k8s API server (:6443)      │   │
│  │  SIGCHLD reap │  │   REST + WebSocket            │   │
│  │  signal fwd   │  │   kubectl-compatible          │   │
│  └───────────────┘  └───────────────────────────────┘   │
│                                                         │
│  ┌───────────────────────────────────────────────────┐  │
│  │             Process Supervisor                    │  │
│  │  Pods · Deployments · Services · ConfigMaps       │  │
│  │  OCI image pull · chroot execution                │  │
│  │  cgroups v2 resource limits · Health probes       │  │
│  │  Port publishing · Service proxy                  │  │
│  └───────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────┘
```

## Features

| Feature | Details |
|---|---|
| **kubectl-compatible API** | Full REST API on port 6443 — works with unmodified `kubectl` |
| **Pod lifecycle** | Create, list, describe, delete, watch pod phase |
| **Deployments** | Create, scale (`kubectl scale`), describe, delete |
| **Exec** | `kubectl exec` with PTY (`-it`) and pipe (`-i`) support via WebSocket |
| **Logs** | `kubectl logs` from in-memory ring buffer (last 1000 lines) |
| **Namespaces** | Full namespace CRUD with resource isolation |
| **Nodes** | Reports the host as a single node with real CPU/memory capacity |
| **Events** | Pod lifecycle events via `kubectl get events` |
| **Metrics** | `kubectl top pod/node` backed by cgroup v2 stats |
| **Services** | ClusterIP and NodePort — TCP proxy with round-robin load balancing |
| **Endpoints** | Endpoints + EndpointSlices (discovery.k8s.io/v1) for `kubectl describe svc` |
| **ConfigMaps & Secrets** | Full CRUD, envFrom injection, volume mounts |
| **PersistentVolumes / PVCs** | Static PV provisioning, claim binding |
| **DNS** | In-cluster DNS server on port 53, service name resolution, resolv.conf injection |
| **OCI images** | Pull from any OCI-compatible registry; layer cache at `/var/lib/z8s` |
| **Native processes** | `image: ""` runs a host binary directly without any container isolation |
| **chroot containers** | Non-empty image field pulls OCI layers and chroots the process |
| **Network isolation** | Per-pod network namespaces with port publishing to unique host ports |
| **Hostname isolation** | Each container gets its own UTS namespace with the pod name as hostname |
| **Manifest watcher** | Drops YAML files into `/etc/z8s/manifests/` — they are applied automatically |
| **Health probes** | `livenessProbe`, `readinessProbe`, `startupProbe` (exec, httpGet, tcpSocket) |
| **cgroups v2** | Optional memory/CPU limits applied when running as root |
| **PID 1 init** | Zombie reaping, signal forwarding when running as PID 1 in a container |
| **Protobuf bodies** | Accepts `kubectl create` protobuf-encoded requests transparently |
| **Watch support** | `kubectl get --watch` for pods, services, deployments |

## Quick start

### Build

```bash
cargo build --release
```

The release binary is ~5.4 MB, statically optimized (`opt-level = "z"`, `lto = true`, `strip = true`).

### Run (development)

```bash
sudo ./z8s.sh start       # starts target/debug/z8s, logs to /tmp/z8s.log
./z8s.sh status
sudo ./z8s.sh restart
sudo ./z8s.sh stop
```

> `sudo` is required when using OCI containers (chroot, network namespaces). Native processes (`image: ""`) work without root.

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
alias kapply='kubectl --validate=false --server=http://localhost:6443 --insecure-skip-tls-verify'

k get nodes
k get pods -A
k apply --validate=false -f my-app.yaml
k exec -it my-pod -- bash
k logs my-pod
k top pod
k get svc
k describe svc my-service
```

## Quick example: nginx + NodePort service

```bash
kubectl apply --validate=false -f - <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: nginx
  labels:
    app: nginx
spec:
  replicas: 2
  selector:
    matchLabels:
      app: nginx
  template:
    metadata:
      labels:
        app: nginx
    spec:
      containers:
      - name: nginx
        image: nginx:latest
        ports:
        - containerPort: 80
EOF

kubectl apply --validate=false -f - <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: nginx-svc
spec:
  selector:
    app: nginx
  ports:
  - name: http
    port: 80
    targetPort: 80
    nodePort: 30080
  type: NodePort
EOF

# Access via NodePort
curl http://localhost:30080
```

## Architecture

### Components

```
src/
├── main.rs               Entry point; wires all components together
├── init.rs               PID 1 init: SIGCHLD zombie reaping, signal forwarding
├── controller.rs         DeploymentController: reconcile loop for replica counts
├── api/
│   ├── mod.rs            AnyResource enum (Pod, Service, ConfigMap, etc.)
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
│   ├── rootfs.rs         chroot/namespace helpers, UTS hostname isolation
│   ├── volumes.rs        Volume mount resolution (hostPath, emptyDir, configMap, secret)
│   ├── oci_config.rs     OCI config parsing (entrypoint, cmd, env)
│   └── port_publish.rs   127.0.0.1 port forwarding from host → container netns
├── network/
│   ├── mod.rs            NetworkManager: service proxy lifecycle, endpoints
│   ├── dns.rs            In-cluster DNS server (service resolution)
│   ├── service_proxy.rs  TCP proxy with round-robin backend selection
│   └── port_publish.rs   Unique host port allocation per container port
├── manifest/
│   └── watcher.rs        inotify-based watcher for /etc/z8s/manifests/
├── config.rs             CLI config (port, log level, etc.)
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
ResourceStore.apply(Pod)               ← in-memory store
     │
     ▼
ProcessSupervisor.start_pod()
     ├── ImageManager.unpack_image()   ← pull + extract OCI layers
     ├── CgroupManager.create_pod_cgroup()
     ├── child_enter_ns_fork/root      ← unshare(CLONE_NEWNS|NEWUTS|NEWIPC|NEWNET)
     │   ├── sethostname(pod_name)     ← per-pod hostname in UTS namespace
     │   ├── publish_ports()           ← unique 127.0.0.1:host_port → container:port
     │   └── chroot/pivot_root        ← filesystem isolation
     └── execvpe(entrypoint, args)     ← run the container process
```

### Service proxy flow

```
curl http://localhost:30080
     │
     ▼
run_proxy_addr (NodePort listener on 0.0.0.0:30080)
     │ (accept)
     ▼
handle_connection()
     ├── find_endpoints()            ← query store for pods matching selector
     │   ├── is_pod_alive() check
     │   ├── resolve_container_port()
     │   └── TCP health check
     ├── round-robin pick            ← atomic counter % endpoints.len()
     └── proxy TCP to 127.0.0.1:host_port  ← forwarded into pod's netns
```

### Endpoints / EndpointSlices

When you run `kubectl describe svc`, the scheduler:

1. Queries the EndpointSlice API (`discovery.k8s.io/v1`) — **implemented**
2. Returns one slice per matching pod with its unique published host port
3. Falls back to the v1 Endpoints API

This makes `Endpoints:` show distinguishable `127.0.0.1:20000,127.0.0.1:20001,127.0.0.1:20002` entries.

## API endpoints

### Core (v1)

| Method | Path | Description |
|---|---|---|
| GET | `/api` | API versions |
| GET | `/api/v1` | Resource list |
| GET/POST | `/api/v1/namespaces` | List / create namespaces |
| GET/DELETE | `/api/v1/namespaces/{name}` | Get / delete namespace |
| GET | `/api/v1/nodes` | List nodes |
| GET | `/api/v1/nodes/{name}` | Get node |
| GET | `/api/v1/pods` | List all pods |
| GET/POST | `/api/v1/namespaces/{ns}/pods` | List / create pods |
| GET/PATCH/PUT/DELETE | `/api/v1/namespaces/{ns}/pods/{name}` | Get / update / delete pod |
| GET | `/api/v1/namespaces/{ns}/pods/{name}/log` | Pod logs |
| GET/POST | `/api/v1/namespaces/{ns}/pods/{name}/exec` | Exec (WebSocket upgrade) |
| GET/POST | `/api/v1/namespaces/{ns}/services` | List / create services |
| GET/PUT/PATCH/DELETE | `/api/v1/namespaces/{ns}/services/{name}` | Get / update / delete |
| GET | `/api/v1/endpoints` | List all endpoints |
| GET | `/api/v1/namespaces/{ns}/endpoints` | List namespace endpoints |
| GET | `/api/v1/namespaces/{ns}/endpoints/{name}` | Get service endpoints |
| GET | `/api/v1/configmaps` | List all configmaps |
| GET/POST | `/api/v1/namespaces/{ns}/configmaps` | List / create |
| GET/PUT/PATCH/DELETE | `/api/v1/namespaces/{ns}/configmaps/{name}` | Get / update / delete |
| GET | `/api/v1/secrets` | List all secrets |
| GET/POST | `/api/v1/namespaces/{ns}/secrets` | List / create |
| GET/PUT/PATCH/DELETE | `/api/v1/namespaces/{ns}/secrets/{name}` | Get / update / delete |
| GET | `/api/v1/events` | All events |
| GET | `/api/v1/namespaces/{ns}/events` | Namespace events |
| GET/POST | `/api/v1/persistentvolumes` | List / create PVs |
| GET/DELETE | `/api/v1/persistentvolumes/{name}` | Get / delete PV |
| GET | `/api/v1/persistentvolumeclaims` | List all PVCs |
| GET/POST | `/api/v1/namespaces/{ns}/persistentvolumeclaims` | List / create PVCs |
| GET/DELETE | `/api/v1/namespaces/{ns}/persistentvolumeclaims/{name}` | Get / delete PVC |

### Apps (v1)

| Method | Path | Description |
|---|---|---|
| GET/POST | `/apis/apps/v1/deployments` | List all / create |
| GET/POST | `/apis/apps/v1/namespaces/{ns}/deployments` | List / create |
| GET/PATCH/PUT/DELETE | `/apis/apps/v1/namespaces/{ns}/deployments/{name}` | Get / update / delete |
| PATCH | `/apis/apps/v1/namespaces/{ns}/deployments/{name}/scale` | Scale replicas |

### Discovery (v1)

| Method | Path | Description |
|---|---|---|
| GET | `/apis/discovery.k8s.io/v1` | Resource list |
| GET | `/apis/discovery.k8s.io/v1/endpointslices` | All EndpointSlices |
| GET | `/apis/discovery.k8s.io/v1/namespaces/{ns}/endpointslices` | Namespace slices |
| GET | `/apis/discovery.k8s.io/v1/namespaces/{ns}/endpointslices/{name}` | Get slice |

### Metrics

| Method | Path | Description |
|---|---|---|
| GET | `/apis/metrics.k8s.io/v1beta1/nodes` | Node CPU/memory (`kubectl top node`) |
| GET | `/apis/metrics.k8s.io/v1beta1/pods` | Pod CPU/memory (`kubectl top pod`) |
| GET | `/apis/metrics.k8s.io/v1beta1/namespaces/{ns}/pods` | Namespace pod metrics |

### Authorization

| Method | Path | Description |
|---|---|---|
| POST | `/apis/authorization.k8s.io/v1/selfsubjectaccessreviews` | Access review |
| POST | `/apis/authorization.k8s.io/v1/subjectaccessreviews` | Access review |

### Discovery / health

| Path | Description |
|---|---|
| `/apis` | API groups |
| `/openapi/v2`, `/openapi/v3` | OpenAPI schema stubs |
| `/version` | Server version string |
| `/healthz`, `/readyz`, `/livez` | Health checks (all return `ok`) |

## Service types

### ClusterIP

Default type. Binds the service's ClusterIP on the host loopback interface and proxies to matching pods:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-svc
spec:
  selector:
    app: my-app
  ports:
  - port: 80
    targetPort: 8080
  type: ClusterIP
```

### NodePort

Adds a host-level listener on `0.0.0.0:<nodePort>` that load-balances to matching pods:

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-svc
spec:
  selector:
    app: my-app
  ports:
  - name: http
    port: 80
    targetPort: 80
    nodePort: 30080
  type: NodePort
```

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

### Volumes

Supported volume types:

| Type | Description |
|---|---|
| `emptyDir` | Ephemeral directory, lost when pod is recreated |
| `hostPath` | Host path mounted into container |
| `configMap` | ConfigMap data as files (read-only) |
| `secret` | Secret data as files (read-only) |

### ConfigMap / Secret injection

```yaml
# envFrom: inject all keys as environment variables
envFrom:
- configMapRef:
    name: app-config
- secretRef:
    name: app-secret

# volume mounts
volumeMounts:
- name: config
  mountPath: /etc/config
volumes:
- name: config
  configMap:
    name: app-config
```

### Restart policy

| Policy | Behavior |
|---|---|
| `Always` (default) | Always restart |
| `Never` | Never restart (pod reaches Succeeded or Failed) |
| `OnFailure` | Restart only on non-zero exit |

### securityContext

```yaml
securityContext:
  runAsUser: 65534
  runAsGroup: 65534
```

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

## In-cluster DNS

z8s runs a built-in DNS server on port 53 (or 5353 if 53 is unavailable). It resolves:

- Bare service names → ClusterIP (`my-svc`)
- FQDN service names (`my-svc.default.svc.cluster.local`)
- External names via upstream resolver

Containers get a patched `/etc/resolv.conf` with `nameserver 127.0.0.1` and `search default.svc.cluster.local svc.cluster.local cluster.local`.

## Manifest watcher

Place YAML files in `/etc/z8s/manifests/` and z8s applies them at startup and whenever a file changes:

```bash
sudo mkdir -p /etc/z8s/manifests
sudo cp my-service.yaml /etc/z8s/manifests/
# z8s detects the file and starts the pod automatically
```

Supports multi-document YAML (separated by `---`). Supports Pod, Deployment, and Service kinds.

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
sudo ./z8s.sh {start|stop|restart|status}
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
sudo ./test.sh

# Run specific test scripts
bash tests/test-nginx-deploy.sh
bash tests/test-whoami-deploy.sh

# Run comprehensive pre-written test suite
bash tests/run-tests.sh
```

The test scripts use `kubectl` against the live server and validate API responses. They require `kubectl` to be in `PATH`.

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `RUST_LOG` | `info` | Log level (`debug`, `info`, `warn`, `error`) |
| `Z8S_SERVER` | `http://localhost:6443` | Override for test scripts |

## Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime |
| `axum 0.8` | HTTP + WebSocket server |
| `k8s-openapi 0.27 (v1_35)` | Typed k8s API structs |
| `nix 0.31` | PTY, signals, cgroups, mounts, namespaces |
| `oci-distribution` | OCI registry client |
| `flate2` + `tar` | Layer extraction |
| `serde_json` + `serde_yaml` | Serialization |
| `notify` | inotify-based manifest watcher |
| `tracing` | Structured logging |
| `uuid` | Resource UID generation |
| `chrono` | Timestamps |
| `futures-util` | WebSocket sink/stream |
| `anyhow` | Error handling |
| `caps` | Capability dropping |
| `capsicum` / `landlock` | Sandboxing |

## Limitations

- **Single node only** — no clustering or networking between nodes
- **In-memory state** — all resource state is lost on restart
- **No RBAC / auth** — all requests are accepted
- **chroot requires root** — OCI containers need root; native processes work as any user
- **No init containers** — `initContainers` field is not processed
- **No LoadBalancer services** — NodePort is the only external-access type
- **No StatefulSet / DaemonSet / Job / CronJob** — only Pod and Deployment controllers
- **No pod-to-pod networking** — services use host-level TCP proxying, not pod IP routing

## License

MIT
