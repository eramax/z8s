# Veth + Bridge Network Architecture Plan

> **Date:** 2026-05-27
> **Goal:** Replace setns-based port publishing with full L2 bridge networking — real pod IPs, inter-pod connectivity, external reachability, built-in ingress (HTTP + TCP), multi-node support, no `iproute2` dependencies.
> **Constraint:** All netlink operations use the same `libc` FFI pattern as `ensure_loopback_alias` — no new crates, no shelling out.

---

## 1. Current Architecture (Baseline)

```
HOST NETNS                      POD NETNS (#42)
┌──────────────────────┐        ┌──────────────────┐
│                      │        │                  │
│  Service Proxy       │        │  nginx           │
│  10.96.0.3:80        │        │  127.0.0.1:80    │
│                      │        │                  │
│  Port Forwarder      │        │  lo (UP)         │
│  127.0.0.1:20000 ────┼─setns──┤  127.0.0.1       │
│                      │        │                  │
└──────────────────────┘        └──────────────────┘
```

**Problems:**
- Pod IP is always `127.0.0.1` — no real identity
- Inter-pod communication requires Service proxy (no direct pod IP routing)
- External clients cannot reach pods directly
- Port publishing uses a short-lived thread + `setns(CLONE_NEWNET)` per connection
- No foundation for built-in ingress, domain routing, or multi-node
- No SNAT for outbound traffic — pods that call external APIs will have unroutable source IPs

---

## 2. Proposed Architecture — Full Stack

```
Cloudflare / External DNS              ┌─────────────────┐
  *.z8s.emo.net ──A──► <host-ip>       │ Config           │
                                        │ --pod-cidr       │
┌───────────────────────────────────────┤ 10.42.0.0/16     │
│  Z8S HOST (single node view)         │ --svc-cidr       │
│                                      │ 10.96.0.0/16     │
│  ┌─────────────────────────────────┐ │ --ingress-ports  │
│  │  Built-in Ingress Controller    │ │ 80,443,22,5432   │
│  │  (axum HTTP + tokio TCP)        │ └──────────────────┘
│  │                                 │
│  │  :80  ── HTTP Host header ──► svc1.z8s.emo.net ──► 10.42.0.2:8080
│  │  :443 ── TLS SNI        ──► svc1.z8s.emo.net ──► 10.42.0.2:443
│  │  :22  ── port match     ──► sshd-svc           ──► 10.42.0.5:22
│  │  :5432─ port match      ──► db-svc             ──► 10.42.0.6:5432
│  │  ┌──────────────────────────────────┐           │
│  │  │  Route decision:                 │           │
│  │  │  HTTP   → match Host header      │           │
│  │  │  TLS    → match SNI from         │           │
│  │  │           ClientHello            │           │
│  │  │  Raw TCP→ match destination port │           │
│  │  │  Then:   copy_bidirectional()    │           │
│  │  │           (kernel splice,        │           │
│  │  │            zero-copy)            │           │
│  │  └──────────────────────────────────┘           │
│  └─────────────────────────────────┘               │
│                                                     │
│  ┌──────────────────────────────────────────────┐   │
│  │  z8s0 (bridge, 10.42.0.1/16)                 │   │
│  │                                               │   │
│  │  veth-podA ── peer ──► pod-A (10.42.0.2/16)  │   │
│  │  veth-podB ── peer ──► pod-B (10.42.0.3/16)  │   │
│  │  veth-podC ── peer ──► pod-C (10.42.0.4/16)  │   │
│  │                                               │   │
│  │  ClusterIP loopback alias (unchanged):        │   │
│  │  10.96.0.3 ──► lo                             │   │
│  └──────────────────────────────────────────────┘   │
│                                                     │
│  Host route (exposes pod CIDR externally):          │
│  10.42.0.0/16 dev z8s0                              │
└─────────────────────────────────────────────────────┘
```

---

## 3. Ingress Architecture — Two Layers

### 3.1 HTTP Ingress (Layer 7)

Built into z8s using existing `axum` + `tokio`. No separate pod, no deployment, no lifecycle management.

```
Ingress resource (CRD or YAML config):
  apiVersion: z8s.io/v1
  kind: Ingress
  metadata:
    name: svc1
  spec:
    rules:
    - host: svc1.z8s.emo.net
      http:
        paths:
        - path: /
          backend:
            serviceName: nginx-svc
            servicePort: 80
```

The ingress controller watches these resources from the store (same as the deployment controller), binds the port once, and routes by `Host` header. Backend selection uses the same `find_endpoints()` round-robin as the ClusterIP proxy.

**Performance:** `axum` handler does `tokio::io::copy_bidirectional(client, backend)`. The kernel uses `splice(2)` for zero-copy — data never touches userspace after the initial accept + header read. This is identical to nginx, Traefik, and Caddy's internal path.

```
Per-connection cost for HTTP:
  1. accept (syscall)
  2. read HTTP headers (~1-2 recv syscalls)
  3. route decision (userspace, nanoseconds)
  4. connect to backend (syscall)  ──▶  10.42.0.2:80
  5. splice loop (~2 syscalls per RTT)
  6. close (2 syscalls)

No userspace data copying. No extra network hop.
```

### 3.2 TCP Ingress (Layer 4)

Same built-in process, different routing strategy:

| Protocol | Routing method | Example |
|---|---|---|
| HTTP | `Host` header | `curl http://svc1.z8s.emo.net` |
| HTTPS / TLS | SNI from ClientHello | `curl https://svc1.z8s.emo.net` |
| PostgreSQL (TLS) | SNI from ClientHello | `psql "host=db.svc1.z8s.emo.net sslmode=require"` |
| SSH (no TLS) | Destination port | `ssh -p 22 user@z8s.emo.net` |
| PostgreSQL (no TLS) | Destination port | `psql -h z8s.emo.net -p 5432` |
| Custom TCP | Destination port | Any port → any backend |

**Ingress resource for TCP:**

```yaml
apiVersion: z8s.io/v1
kind: Ingress
metadata:
  name: db-ingress
spec:
  rules:
  - host: db.svc1.z8s.emo.net
    port: 5432
    protocol: TCP
    tls: true        # ← enables SNI-based routing instead of port-based
    backend:
      serviceName: postgres-svc
      servicePort: 5432
```

When `tls: true`, the proxy reads the full TLS `ClientHello` handshake message. The TLS record header is 5 bytes, but the SNI extension is deep inside the handshake — the full parse requires reading up to ~2KB (typical real-world ClientHello with all extensions). The proxy reads `recv()` in a loop up to a 4KB buffer, finds the SNI extension in the handshake bytes, extracts the hostname, then splices the already-read data plus any remainder to the backend. The read is non-blocking — the TLS handshake bytes are forwarded verbatim so the backend sees the complete ClientHello seamlessly.

When `tls: false` or plain TCP, the proxy matches by destination port. If multiple services claim the same port, the first-match wins (same as physical port conflict — unambiguous).

### 3.3 Deeper Subdomain Routing

All handled by `Host` header or SNI matching:

```
*.z8s.emo.net
  ├── svc1.z8s.emo.net          → nginx-svc
  ├── db.svc1.z8s.emo.net       → postgres-svc  (SNI or port)
  ├── products.svc1.z8s.emo.net → products-svc   (HTTP Host header)
  └── api.products.svc1.z8s.emo.net → api-svc    (any depth)
```

No limit on subdomain depth. Matching is simple string suffix or regex — trivially handled by axum's router or a manual `std::str::ends_with` check.

### 3.4 What about services that need custom ports on the same host?

The Ingress resource specifies the **listener port** on the host. Each unique port needs one listen socket. For services that share a port (e.g., multiple HTTP sites on :80), the `Host` header disambiguates.

For services on unique ports (e.g., :5432 for PostgreSQL, :22 for SSH), the ingress binds one socket per port. This is the same model Traefik uses (`entryPoints`) and what every cloud load balancer does.

> **⚠️ Port conflict policy:** If a second ingress rule tries to claim the same `(host:port)` as an already-registered rule, the conflict is **rejected at admission time** (when the Ingress resource is written to the store) with a clear error message: `"port 5432 already bound by ingress <name> in namespace <ns>"`. At runtime, if the same port is claimed by two rules (e.g., due to different namespaces being processed concurrently), a `warn!` log is emitted and the second rule is skipped. No silent "first-match wins."

Configuration:
```
z8s --ingress-ports 80,443,22,5432,3306,6379
```

Default: `80,443` only. Additional ports added via CLI or ingress resource (dynamic bind on demand).

---

## 4. End-to-End Data Flows

| Scenario | Path | Overhead |
|---|---|---|
| Browser → `svc1.z8s.emo.net` | Cloudflare A → host:80 → ingress (Host match) → `10.42.0.2:8080` | splice zero-copy |
| Pod A → Pod B | `10.42.0.2:80` → z8s0 bridge → `10.42.0.3:80` | L2 kernel forward, no userspace |
| Pod A → ClusterIP | `10.42.0.2:80` → `10.96.0.3:80` (lo) → proxy → `10.42.0.3:80` | splice zero-copy |
| `psql` → `db.svc1.z8s.emo.net` | Cloudflare A → host:5432 → ingress (SNI match `db.svc1`) → `10.42.0.5:5432` | splice zero-copy |
| `ssh user@z8s.emo.net` | Cloudflare A → host:22 → ingress (port match) → `10.42.0.6:22` | splice zero-copy |
| External curl → pod | host route → z8s0 → `10.42.0.2:80` | kernel route + L2, no userspace |
| Pod → Internet | `10.42.0.2` → z8s0 → host → eth0 (ip_forward + MASQUERADE) | kernel forward + SNAT via iptables |

**Key point:** The ingress never touches the data after the route decision. `copy_bidirectional` hands off to the kernel's `splice` which copies directly between the TCP stacks. The overhead per byte is zero — only per-connection setup matters.

---

## 5. Multi-Node Architecture

Each node runs z8s with its own `z8s0` bridge. Pod IPs are allocated from node-local /24 subnets carved from the /16.

```
Node A (10.0.0.1)              Node B (10.0.0.2)
┌──────────────────────┐       ┌──────────────────────┐
│ z8s0                  │       │ z8s0                  │
│ 10.42.1.1/24          │       │ 10.42.2.1/24          │
│                       │       │                       │
│ Pod A1: 10.42.1.2    │       │ Pod B1: 10.42.2.2    │
│ Pod A2: 10.42.1.3    │       │ Pod B2: 10.42.2.3    │
│                       │       │                       │
│ Route: 10.42.2.0/24  │───────│ Route: 10.42.1.0/24  │
│   via 10.0.0.2       │       │   via 10.0.0.1       │
│                       │       │                       │
│ Ingress (built-in)    │       │ Ingress (built-in)    │
│ :80, :443, :22, :5432 │       │ :80, :443, :22, :5432 │
└──────────────────────┘       └──────────────────────┘
```

### 5.1 Cross-Node Pod Communication

Pod A1 (`10.42.1.2`) → Pod B1 (`10.42.2.2:80`):

```
Pod A1 ──► veth ──► z8s0 (Node A)
           └── host route lookup: 10.42.2.0/24 via 10.0.0.2
           └── eth0 (Node A) ──► network ──► eth0 (Node B)
           └── host route: 10.42.2.0/24 dev z8s0
           └── z8s0 (Node B) ──► veth ──► Pod B1
```

**No overlay, no VXLAN, no tunnels.** Just host routing. The overhead is exactly one extra L3 hop — the same as bare-metal Kubernetes with a real CNI (Calico, Cilium in legacy mode).

### 5.2 Cross-Node Ingress Routing

Each node's built-in ingress runs independently. A service's ingress rules are replicated to all nodes (from the store). When Node A receives a request for a service that has all backends on Node B:

**Option A: Direct proxy (chosen)**
Node A's ingress proxies to `10.42.2.X:port` directly — one extra network hop. Same `splice` zero-copy, no userspace overhead.

**Option B: DNS pinning (future optimization)**
z8s advertises different DNS records per node based on backend locality:
```
svc1.z8s.emo.net  A  10.0.0.1  (if backends on Node A)
svc1.z8s.emo.net  A  10.0.0.2  (if backends on Node B)
```
Cloudflare / DNS resolver returns both. Client picks one. If all replicas are on Node A, Node B doesn't advertise — traffic never hairpins.

### 5.3 Service Proxy + Multi-Node

The ClusterIP proxy (`10.96.0.3:80`) runs on every node (same as today). It finds backends across all nodes by their pod IPs (`10.42.1.X` or `10.42.2.X`). Round-robin works the same whether the backend is local or remote.

> **⚠️ NOTE (Phase 6 gap):** The store is per-node in-memory. `find_endpoints()` on Node A will **only return pods running on Node A** unless a cross-node reconciliation mechanism exists. This means the ClusterIP proxy on Node A will silently only route to local backends in multi-node mode. Fixing this requires a future consensus layer or external store — documented in §18 Open Questions.

### 5.4 Security: No Default Network Isolation

> **⚠️ NOTE:** The bridge + host route design provides **zero network policy isolation**. Any pod on any node can reach any other pod on any other node by IP. There is no default-deny, no firewall between namespaces, no Kubernetes NetworkPolicy enforcement. This is identical to Flannel and Weave in their default (no-policy) modes, but it means that multi-tenant workloads on the same z8s cluster have no network isolation between them. Adding network policy enforcement is a significant future phase (likely via eBPF or iptables) and is out of scope for this plan.

### 5.5 Node Discovery

For MVP, multi-node is configured statically:
```
z8s --node-name node-a --node-ip 10.0.0.1 --peers node-b=10.0.0.2,node-c=10.0.0.3
```

Peers communicate over the host network. The store is per-node (in-memory) — cross-node pod discovery needs a future consensus layer or external store.

---

## 6. Netlink Operations

All netlink messages are constructed as raw byte buffers and sent via `libc::sendmsg` on an `AF_NETLINK` / `NETLINK_ROUTE` socket — same technique as `ensure_loopback_alias` in `service_proxy.rs:52-131`.

### 6.1 Create Bridge — `RTM_NEWLINK`

```
struct nlmsghdr {
    nlmsg_len:  (len)
    nlmsg_type: RTM_NEWLINK = 16
    nlmsg_flags: NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL
    nlmsg_seq:  1
    nlmsg_pid:  0
}
struct ifinfomsg {
    ifi_family: AF_UNSPEC = 0
    ifi_type:   0  (ether)
    ifi_index:  0  (auto)
    ifi_flags:  0
    ifi_change: 0
}
[IFLA] {
    IFLA_IFNAME: "z8s0\0"
    IFLA_LINKINFO: [nested] {
        IFLA_INFO_KIND: "bridge\0"
    }
}
```

Called once at startup. Idempotent: if `z8s0` already exists, `NLM_F_EXCL` returns `EEXIST` which is silently ignored.

### 6.2 Create Veth Pair — `RTM_NEWLINK`

```
struct nlmsghdr {
    nlmsg_len:  (len)
    nlmsg_type: RTM_NEWLINK = 16
    nlmsg_flags: NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL
    nlmsg_seq:  1
    nlmsg_pid:  0
}
struct ifinfomsg {
    ifi_family: AF_UNSPEC
    ifi_type:   0
    ifi_index:  0
    ifi_flags:  0
    ifi_change: 0
}
[IFLA] {
    IFLA_IFNAME: "veth-<pod>\0"
    IFLA_LINKINFO: [nested] {
        IFLA_INFO_KIND: "veth\0"
        IFLA_INFO_DATA: [nested] {
            VETH_INFO_PEER: [nested] {
                struct ifinfomsg { 0, 0, 0, 0, 0 }
                [IFLA] {
                    IFLA_IFNAME: "eth0\0"
                    IFLA_NET_NS_FD: fd of /proc/<child_pid>/ns/net
                }
            }
        }
    }
}
```

The `IFLA_NET_NS_FD` places the peer end directly inside the pod's network namespace using the ns fd we already capture during fork.

### 6.3 Attach Veth Host End to Bridge — `RTM_NEWLINK`

```
NLM_F_REQUEST (no CREATE/EXCL — modifies existing link)
IFLA_IFNAME: "veth-<pod>\0"
IFLA_MASTER: <bridge_ifindex>
```

Single `RTM_NEWLINK` with `IFLA_MASTER` sets the bridge port. Combined with `ioctl(SIOCGIFFLAGS | IFF_UP)` to bring the link up.

### 6.4 Assign IP to Pod Interface — `RTM_NEWADDR` (inside netns)

Reuse the same byte-packing from `ensure_loopback_alias` but:
- Interface is `eth0` (not `lo`)
- Address is `10.42.<node>.X/16` (not a loopback ClusterIP)
- No `IFA_LOCAL`+`IFA_ADDRESS` distinction needed (single address)

Run inside the child process after `CLONE_NEWNET` but before exec.

### 6.5 Add Default Route Inside Pod — `RTM_NEWROUTE`

```
struct rtmsg {
    rtm_family:  AF_INET = 2
    rtm_dst_len: 0         (default route = /0)
    rtm_table:   RT_TABLE_MAIN = 254
    rtm_protocol: RTPROT_STATIC = 2
    rtm_scope:   RT_SCOPE_UNIVERSE = 0
    rtm_type:    RTN_UNICAST = 1
}
[RTA] {
    RTA_GATEWAY: 10.42.<node>.1  (bridge IP)
    RTA_OIF:     eth0_ifindex
}
```

### 6.6 Add Host Route for Pod CIDR — `RTM_NEWROUTE`

```
[RTA] {
    RTA_DST:     10.42.<node>.0    (node's /24)
    RTA_OIF:     z8s0_ifindex
}
```

Makes this node's pods reachable from outside the bridge. For multi-node, also add routes for peer nodes' /24 subnets via their host IPs.

---

## 7. Pod CIDR Configuration

Same pattern as `--service-cidr` in `config.rs`:

```
z8s --pod-cidr 10.42.0.0/16 --node-name node-a --node-ip 10.0.0.1
```

`--pod-cidr` is parsed identically to `--service-cidr`. Default: `10.42.0.0/16`.

The node slices its /24 from the /16:
- Node A (`10.0.0.1`): `10.42.1.0/24`
- Node B (`10.0.0.2`): `10.42.2.0/24`
- Default (single node): `10.42.0.0/24`

Pod IP allocation within the /24 is sequential, stored in `pod.status.pod_ip`. No IP reuse for MVP — 254 addresses per node is sufficient.

---

## 8. Integration with `spawn_container()` Flow

Current:
```
unshare(... | CLONE_NEWNET)
  → child: setup_loopback()
  → parent: publish_ports(pid, container_ports)
```

New flow:
```
// Startup (once):
ensure_bridge("z8s0", "10.42.<node>.1/24")
add_host_route("10.42.<node>.0/24", "z8s0")

// Per container:
fork + unshare(... | CLONE_NEWNET)
  → child:
      write sync byte (PID)   // child sends PID to parent
      wait for parent ack     // wait until veth is created
      setup_loopback()
      assign_ip("10.42.<node>.X/16", "eth0")
      add_default_route("10.42.<node>.1", "eth0")
      // proceed with chroot/exec
  → parent:
      read sync byte (child PID)
      create_veth("veth-<pod>", "eth0", child_ns_fd)
      attach_to_bridge("veth-<pod>", bridge_ifindex)
      bring_up("veth-<pod>")
      write ack byte          // signal child to continue
```

This maps onto the existing sync-byte protocol in `child_enter_ns_root()` — just adds one extra round-trip.

---

## 9. Built-in Ingress Implementation Sketch

### 9.1 Ingress Resource

New CRD stored in `ResourceStore`, same pattern as Pod/Service/Deployment:

```rust
pub struct Ingress {
    pub metadata: ObjectMeta,
    pub spec: IngressSpec,
}

pub struct IngressSpec {
    pub rules: Vec<IngressRule>,
}

pub struct IngressRule {
    pub host: Option<String>,          // svc1.z8s.emo.net
    pub port: Option<u16>,             // 80, 443, 22, 5432 (default: 80)
    pub protocol: Option<String>,      // HTTP, TCP (default: HTTP)
    pub tls: Option<IngressTLS>,       // TLS config + SNI routing
    pub backend: IngressBackend,
}

pub struct IngressBackend {
    pub service_name: String,
    pub service_port: IntOrString,
}
```

### 9.2 Ingress Controller

New module `src/ingress/mod.rs`:

```
IngressController {
    store: Arc<ResourceStore>,
    listeners: HashMap<String, JoinHandle<()>>,  // key: "0.0.0.0:80"

    // Lifecycle:
    - watch ingress resources from store
    - on create/update: bind port, register route
    - on delete: unbind if last rule on that port

    // HTTP routing (port 80/443):
    - axum Router with dynamic Host-based routing
    - each ingress rule adds a route to the router
    - handler does find_endpoints() + copy_bidirectional()

    // TCP routing (other ports):
    - per-port tokio TcpListener
    - accept loop: read ClientHello (if TLS) or match port → backend
    - SNI parsing: read first 5 bytes, extract hostname length
    - handler does same copy_bidirectional()

    // Backend resolution:
    - reuses find_endpoints() from service_proxy
    - connects to pod IPs directly (10.42.<node>.X:port)
}
```

### 9.3 Performance Characteristics

| Metric | Expected |
|---|---|
| Per-HTTP-request overhead | ~5 syscalls (accept, recv headers, connect, 2×splice) |
| Per-TCP-connection overhead | ~4 syscalls (accept, connect, 2×splice) |
| Latency added | ~0.1ms (route decision) + network RTT to backend |
| Memory per connection | ~4KB (TCP buffer, no userspace copy buffer) |
| Max connections | Limited by kernel (`fs.nr_open`, RAM for socket bufs) |
| TLS termination | Future — for MVP, pass through to backend for SNI routing |

---

## 10. Multi-Node Data Flows

### 10.1 Cross-Node Ingress

```
Browser → svc1.z8s.emo.net → Cloudflare A → Node A (10.0.0.1):80
  → ingress on Node A: route by Host header → svc1 backend
  → find_endpoints() returns [10.42.2.2:8080] (pod on Node B)
  → splice to 10.42.2.2:8080 via host routing
  → Node A kernel: 10.42.2.0/24 via 10.0.0.2
  → Node B kernel: 10.42.2.2 dev z8s0 → veth → pod
```

One extra L3 hop. No proxy forwarding through Node A's userspace — the splice happens in the kernel, and the packets are routed independently.

### 10.2 Local-Only DNS Pinning (Future)

Each node's embedded DNS returns node-local IPs for services that have backends on that node:

```
Node A DNS: svc1.z8s.emo.net → 10.0.0.1
Node B DNS: svc1.z8s.emo.net → 10.0.0.2
```

This avoids the extra L3 hop entirely for clients that can choose which node to connect to. For external clients (Cloudflare), the DNS resolution returns both A records and the client picks one.

---

## 11. Implementation Phases

### Phase 1 — Bridge + Veth Netlink Helpers

**New file:** `src/network/bridge.rs`

| Function | Netlink op |
|---|---|
| `create_bridge(name, ip_cidr)` | RTM_NEWLINK bridge + RTM_NEWADDR |
| `delete_bridge(name)` | RTM_DELLINK |
| `create_veth(host_name, peer_name, netns_fd)` | RTM_NEWLINK veth with IFLA_NET_NS_FD |
| `attach_to_bridge(ifname, bridge_idx)` | RTM_NEWLINK IFLA_MASTER |
| `link_up(ifname)` | ioctl SIOCGIFFLAGS + SIOCSIFFLAGS |
| `add_route(dst_cidr, gateway, ifindex)` | RTM_NEWROUTE |
| `clean_orphan_veths(bridge_ifindex)` | RTM_GETLINK to enumerate; RTM_DELLINK to remove stale `veth-*` entries on the bridge |
| `enable_ip_forward()` | Write `"1\n"` to `/proc/sys/net/ipv4/ip_forward` (simple file write, no deps) |

Testable independently with unit tests + a dummy netns (created via `unshare(CLONE_NEWNET)` in a temp thread).

**Startup sequence:**
1. `enable_ip_forward()` — required for bridge routing and outbound internet
2. `create_bridge("z8s0", "10.42.<node>.1/24")` — idempotent (EEXIST ignored)
3. `clean_orphan_veths("z8s0")` — remove veths from crashed pods
4. `add_route("10.42.<node>.0/24", "z8s0")` — expose pod CIDR locally

### Phase 2 — Wire Veth into Pod Lifecycle + SNAT + IP Pool

**Files:** `src/container/rootfs.rs`, `src/supervisor/process.rs`, `src/network/bridge.rs`

- **SNAT for outbound traffic — `rustables` crate (raw netlink, no host deps):**
  Use `rustables` to add a MASQUERADE rule via netfilter netlink directly — no shelling out, no `iptables` or `nft` package required.
  ```rust
  use rustables::{Batch, Chain, HookClass, Rule, Table, ProtocolFamily, ChainType, ChainPolicy};
  use rustables::expr::{MetaExpr, MetaKey, CmpExpr, CmpOp, MasqExpr};
  use std::net::Ipv4Addr;
  ```
  Equivalent nftables rule:
  ```
  nft add table nat
  nft add chain nat postrouting { type nat hook postrouting priority -150; }
  nft add rule nat postrouting ip saddr 10.42.0.0/16 oif != "z8s0" masquerade
  ```
  Applied once at bridge creation. The `rustables` batch API ensures atomic rule insertion. If the kernel nf_tables module is unavailable, log a warning: "SNAT not configured — pods cannot reach the internet. Use hostNetwork or native processes."

- **IP pool allocator — `PodIpAllocator`:**
  ```rust
  struct PodIpAllocator {
      free: std::collections::BTreeSet<u8>,  // 1..254 within the /24
  }
  impl PodIpAllocator {
      fn allocate(&mut self) -> Option<u8>;  // pop smallest free
      fn release(&mut self, ip: u8);          // return to pool
      fn count_free(&self) -> usize;
  }
  ```
  Initialized with all `1..254` on bridge creation. On pod start: `allocate()` before fork. On pod end: `release()` in the cleanup path (~10 lines, no dependencies).

- Allocate pod IP before fork
- Create veth in parent after child reveals PID via sync byte
- Assign IP + add default route in child
- Store `pod_ip` in `ContainerInstance` and `pod.status.pod_ip`
- Keep old `PortPublish` path as fallback behind `--net-backend=setns`

> **Why SNAT in Phase 2, not Phase 7:** Without outbound masquerade, any pod that calls an external API (package downloads, webhooks, DB connections to managed services) will send packets with source `10.42.x.x` — unroutable on the public internet. This makes the bridge networking look broken even when it's working correctly. The iptables approach is a 1-line shell command with graceful degradation if unavailable.

### Phase 3 — Update Service Proxy to Use Pod IPs

**Files:** `src/network/service_proxy.rs`, `src/network/mod.rs`

- `find_endpoints()` connects to `10.42.<node>.X:port` instead of `127.0.0.1:<host_port>`
- Remove `supervisor.backend_connect_port()` calls
- Remove `PortPublish::append_ports()` from `reconcile_network_for_service()`

### Phase 4 — Remove Port Publishing Forwarders

**Files:** `src/network/port_publish.rs`

- Delete `run_forwarder()`, `connect_tcp_in_netns()`, `publish_ports()`
- Delete `PortPublish` struct entirely
- Keep `setup_loopback()` — move to `src/network/bridge.rs`

### Phase 5 — Built-in Ingress Controller

**New module:** `src/ingress/`

- Ingress CRD struct + store integration
- `IngressController` — watch, bind, route
- HTTP routing via axum (`Host` header)
- TCP routing via tokio (`port` match + SNI parsing)
- Dynamic port binding (bind on first rule, unbind on last)
- Backend resolution via `find_endpoints()`

### Phase 6 — External Access & Multi-Node

- `--pod-cidr` config flag
- `--node-name`, `--node-ip`, `--peers` flags
- Per-node /24 subnet allocation
- Cross-node host routes on startup
- DNS pinning per node (embed node IP in DNS responses for services with local backends)

---

## 12. What Stays the Same

| Component | Reason |
|---|---|
| **Service proxy (ClusterIP)** | Port → `10.42.X.Y:port` instead of `127.0.0.1:<host_port>`. Same architecture. |
| **NodePort proxy** | Unchanged. Binds `0.0.0.0:3xxxx`. |
| **Loopback alias** (`ensure_loopback_alias`) | Unchanged. Still adds ClusterIP to `lo`. |
| **Embedded DNS** | Unchanged. Resolves `<svc>.<ns>.svc.cluster.local` → ClusterIP. External domains forwarded to upstream DNS. |
| **`kubectl exec`** | Unchanged. Still uses `setns`. |
| **`kubectl logs`** | Unchanged. Ring buffer on host. |
| **Manifest watcher / controller** | Unchanged. Just add Ingress to the watch list. |
| **`copy_bidirectional`** | Same tokio utility. Just connected to pod IPs now. |

---

## 13. What Changes

| Current | New |
|---|---|
| `port_publish::publish_ports()` per pod | Deleted. Veth creation + pod IP. |
| `run_forwarder()` setns-per-connection | Deleted. Direct routing through bridge. |
| `PortPublish::append_ports()` | Deleted. Bridge handles late port publishing. |
| `supervisor.backend_connect_port()` | Deleted. Proxy connects to pod IP directly. |
| `pod.status.pod_ip = "127.0.0.1"` | `pod.status.pod_ip = "10.42.<node>.X"` |
| Child: `setup_loopback()` only | Child: `setup_loopback()` + `assign_pod_ip()` + `add_default_route()` |
| Parent: `publish_ports()` | Parent: `create_veth()` + `attach_to_bridge()` |
| Host: no bridge | Host: `z8s0` bridge with all pod veth ports |
| No ingress | Built-in HTTP + TCP ingress with domain routing |
| No multi-node | Static peer config, cross-node host routes |

---

## 14. Error Handling

| Failure | Handling |
|---|---|---|
| `create_bridge` fails (EEXIST) | Ignore — bridge already exists |
| `create_bridge` fails (other) | Log warning, fall back to old port publishing path |
| `create_veth` fails | Log warning, pod runs without eth0 (lo only) |
| `RTM_NEWADDR` / `RTM_NEWROUTE` in child fails | Log warning, pod has lo only |
| `rustables` MASQUERADE fails | Log warning: "SNAT not configured — pods cannot reach internet. Is CONFIG_NF_TABLES enabled?" |
| Stale veth on pod restart (`EEXIST`) | `clean_orphan_veths()` at startup removes orphans before create |
| Ingress port bind fails (EADDRINUSE) | Log warning, skip that port |
| Ingress port conflict (duplicate rule) | Reject at admission with clear error message |
| SNI parse fails (non-TLS on TLS port) | Fall back to port-based routing |
| Backend unreachable | Ingress returns 502 / connection refused |
| Cross-node route fails | Log warning, pods on other nodes unreachable |

---

## 15. Host Package Dependencies (New)

| Package | Required? | Purpose | Alternative if missing |
|---|---|---|---|
| `iproute2` | **Not required** | All netlink done via raw `libc` FFI or `rustables` | N/A — never installed |
| `iptables` / `nftables` | **Not required** | SNAT done via `rustables` raw netlink | N/A — kernel nf_tables only |

All dependencies (bridge, veth, routing, IP assignment, SNAT) use direct netlink FFI with zero shelling out.

---

## 16. Config Reference (New Flags)

```
z8s [OPTIONS]

Networking:
  --pod-cidr <CIDR>           Pod IP allocation range    [default: 10.42.0.0/16]
  --service-cidr <CIDR>       ClusterIP allocation range  [default: 10.96.0.0/16]
  --ingress-ports <PORTS>     Ingress listen ports       [default: 80,443]
  --node-name <NAME>          This node's name           [default: hostname]
  --node-ip <IP>              This node's host IP        [auto: IP of default route interface]
                              Auto-detection reads /proc/net/route, finds the entry
                              with destination 0.0.0.0, and returns the source IP of
                              that interface. Explicit configuration recommended on
                              multi-homed hosts (VMs with management + data NICs).
  --peers <PEERS>             Other nodes (k=v pairs)    [default: none]
  --net-backend <MODE>        Network backend            [default: bridge]
                              Values: bridge, setns
```

---

## 17. Test Plan

| Test | What it validates |
|---|---|
| Pod gets `eth0` with pod IP | `kubectl exec <pod> -- ip addr show eth0` |
| Pod can ping bridge | `kubectl exec <pod> -- ping -c1 10.42.0.1` |
| Pod A can ping Pod B | `kubectl exec podA -- ping -c1 10.42.0.3` |
| Pod A → Pod B container port direct | `curl http://10.42.0.3:80` from pod A |
| HTTP ingress route by Host header | `curl -H "Host: svc1.z8s.emo.net" http://localhost` |
| TCP ingress route by port | `psql -h localhost -p 5432` → postgres backend |
| TLS ingress route by SNI | `curl --resolve 'svc1.z8s.emo.net:443:127.0.0.1' https://svc1.z8s.emo.net` |
| Service proxy ClusterIP | `kubectl exec podA -- curl http://10.96.0.3:80` |
| NodePort | `curl http://localhost:3xxxx` |
| External pod reachability | `curl http://10.42.0.2:80` from host |
| Deep subdomain routing | `curl -H "Host: products.svc1.z8s.emo.net" http://localhost` |
| Cross-node pod communication | Pod on Node A pings pod IP on Node B |
| Cross-node ingress | Browser → Cloudflare → Node A → backend on Node B |
| Port publishing code removed | No references to `PortPublish` in runtime |
| Rollback (setns backend) | `--net-backend=setns` works identically to current |

---

## 18. Migration Strategy

1. **Phase 1** — Bridge/veth netlink helpers, testable standalone
2. **Phase 2** — Wire into pod lifecycle behind `--net-backend=bridge` flag (default: `setns`)
3. **Phase 3** — Service proxy uses pod IPs when `bridge`, host port when `setns`
4. **Phase 4** — After one release of dual-path, flip default to `bridge`
5. **Phase 5** — Delete `setns` path, delete port publishing code
6. **Phase 6** — Add built-in ingress controller (pure add-on, no migration needed)
7. **Phase 7** — Add multi-node flags and cross-node routing

Rollback at any step: `--net-backend=setns` restores the old behavior entirely.

---

## 19. Open Questions

| Question | Decision |
|---|---|---|
| Pod CIDR? | `10.42.0.0/16`, configurable via `--pod-cidr` |
| Bridge name? | `z8s0` |
| Destroy bridge on shutdown? | Yes — `RTM_DELLINK` in stop handler |
| Outbound internet (SNAT)? | Phase 2 — best-effort iptables MASQUERADE at bridge creation. Warn if unavailable. |
| TLS termination in ingress? | Phase 7+. For MVP, TLS pass-through with SNI routing. |
| Ingress resource CRD or static config? | CRD in store, same pattern as Pod/Service. |
| Multi-node consensus/store? | Future phase. MVP: static peer list, no cross-node pod discovery. |
| Network policy / isolation? | Future phase. MVP: no isolation — all pods can reach all pods. |
| Keep old port publishing code? | Yes — `--net-backend=setns` for one release after bridge is stable. |
| IP reclamation? | Yes — `PodIpAllocator` with `BTreeSet<u8>` free-list from Phase 2. |
