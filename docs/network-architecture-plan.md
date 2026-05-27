# Network Architecture Plan

> **Date:** 2026-05-27 (v3 — single rustables engine)
> **Goal:** Replace setns-based port publishing with a unified nftables-backed L3 network engine — real pod IPs, zero-copy forwarding, full NetworkPolicy, Azure-style VNet/Subnet/NSG, ClusterIP via DNAT, multi-node, no host package dependencies.
> **Contracts:**
> - No shelling out to `ip`, `iptables`, `nft`, or any host binary
> - All L3/L4 enforcement via `rustables` (nfnetlink FFI)
> - All L7 logic via built-in axum (ingress, API gateway)
> - All pod attachment via veth pairs with host routes (no bridge)
> - All netlink operations use the same `libc` FFI pattern as `ensure_loopback_alias`

---

## 1. Current Architecture (Baseline)

```
HOST NETNS                      POD NETNS (#42)
┌──────────────────────┐        ┌──────────────────┐
│                      │        │                  │
│  Service Proxy       │        │  nginx           │
│  10.96.0.3:80        │        │  127.0.0.1:80    │
│  (userspace TCP)     │        │                  │
│  Port Forwarder      │        │  lo (UP)         │
│  127.0.0.1:20000 ────┼─setns──┤  127.0.0.1       │
└──────────────────────┘        └──────────────────┘
```

**Problems:**
- Pod IP is always `127.0.0.1` — no real identity
- Inter-pod requires Service proxy (no direct pod IP routing)
- External clients cannot reach pods directly
- Port publishing uses per-connection `setns(CLONE_NEWNET)` thread
- ClusterIP uses userspace proxy + loopback alias — per-packet userspace hop
- No NetworkPolicy, no isolation, no VNet/subnet abstraction
- No foundation for multi-node

---

## 2. Proposed Architecture — Single Engine

```
┌──────────────────────────────────────────────────────────────────┐
│  Z8S HOST                                                        │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │  L7 INGRESS (axum) — separate, not nftables                  ││
│  │  :80 HTTP Host header → backend pod IP                      ││
│  │  :443 TLS SNI          → backend pod IP                     ││
│  │  :22  port match       → backend pod IP                     ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │  RUSTABLES ENGINE — unified L3/L4 dataplane                  ││
│  │                                                              ││
│  │  ┌─────────────┐ ┌──────────────┐ ┌──────────────────────┐  ││
│  │  │ SNAT         │ │ ClusterIP    │ │ NetworkPolicy  + NSG │  ││
│  │  │ masquerade   │ │ DNAT maps    │ │ pod-selector sets   │  ││
│  │  │ postrouting  │ │ prerouting   │ │ subnet CIDR sets    │  ││
│  │  └─────────────┘ └──────────────┘ └──────────────────────┘  ││
│  │                                                              ││
│  │  nftables tables:                                            ││
│  │    nat:     prerouting (DNAT) + postrouting (SNAT)           ││
│  │    filter:  forward (policy + isolation)                     ││
│  │                                                              ││
│  │  Atomic batch updates via rustables::Batch                   ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ┌──────────────────────────────────────────────────────────────┐│
│  │  POD ATTACHMENT — veth pairs, no bridge                      ││
│  │                                                              ││
│  │  host route: 10.42.0.2/32 dev veth-podA                     ││
│  │  host route: 10.42.0.3/32 dev veth-podB                     ││
│  │  host route: 10.42.1.0/24 via 10.0.0.2 (multi-node)        ││
│  │                                                              ││
│  │  veth-podA ── peer ──► podA (10.42.0.2/16, default route)   ││
│  │  veth-podB ── peer ──► podB (10.42.0.3/16, default route)   ││
│  │                                                              ││
│  │  No bridge. No L2 forwarding. Pure L3 routed.               ││
│  └──────────────────────────────────────────────────────────────┘│
│                                                                  │
│  ip_forward=1  (written once at startup)                         │
└──────────────────────────────────────────────────────────────────┘
```

### Key Design Decisions

| Decision | Rationale |
|---|---|
| **No bridge** | Pods are L3 neighbors via host routes. No L2 isolation leaks, no ARP tables, no STP, no MAC learning. VNet boundary = route aggregate boundary. |
| **veth for attachment only** | Each pod gets one veth pair. Host end is a plain interface with a /32 route. The pod end is `eth0` inside the netns. |
| **`rustables` for all L3/L4** | SNAT, ClusterIP DNAT, NetworkPolicy, NSG filtering — single engine, atomic batch updates, zero-copy kernel path. |
| **ClusterIP via nftables DNAT** | Replace userspace TCP proxy + loopback alias with kernel DNAT map. No `ensure_loopback_alias` needed. Zero per-packet userspace. |
| **L7 ingress separate (axum)** | nftables is L3/L4 only. HTTP Host/SNI/path routing stays in userspace where it belongs. |
| **No host package deps** | Everything through `libc` FFI (`RTM_NEWLINK`, `RTM_NEWADDR`, `RTM_NEWROUTE`) + `rustables` (nfnetlink). |

---

## 3. Data Flows

### 3.1 Pod → Pod (same host)

```
Pod A (10.42.0.2) → 10.42.0.3:80
  └── veth-podA → host routing (10.42.0.3/32 dev veth-podB)
  └── nftables filter forward chain (policy check)
  └── veth-podB → Pod B
```

**One L3 forward through the host kernel. No userspace. No bridge.**

### 3.2 Pod → ClusterIP Service

```
Pod A (10.42.0.2) → 10.96.0.3:80
  └── veth-podA → host
  └── nftables prerouting: ip daddr 10.96.0.3 dnat to 10.42.0.3:8080
  └── host routing: 10.42.0.3/32 dev veth-podB
  └── nftables filter forward chain (policy check)
  └── veth-podB → Pod B
```

**ClusterIP DNAT map (round-robin):**
```
nft add rule ip nat prerouting ip daddr 10.96.0.3 tcp dport 80 \
  dnat to numgen inc mod 2 map { 0 : 10.42.0.3, 1 : 10.42.0.4 }
```

No `ensure_loopback_alias`. No userspace proxy. No per-packet overhead.

### 3.3 External → Ingress → Pod

```
Client → svc1.z8s.emo.net (Cloudflare A → host IP)
  └── host:80 (axum listener)
  └── match Host header → backend service
  └── splice to 10.42.0.2:8080 (kernel splice, zero-copy)
```

L7 ingress is the only userspace touch. The `splice` path still never copies data through userspace after the route decision.

### 3.4 Pod → Internet

```
Pod A (10.42.0.2) → 1.1.1.1:80
  └── veth-podA → host
  └── host routing → eth0 (default route)
  └── nftables postrouting: masquerade (SNAT to host IP)
```

SNAT via `rustables` MASQUERADE rule. Source rewritten to host's external IP.

### 3.5 Pod → Pod (cross-node)

```
Pod A (Node A, 10.42.1.2) → 10.42.2.3:80
  └── veth → host routing (10.42.2.0/24 via 10.0.0.2)
  └── eth0 → Node B
  └── host routing (10.42.2.3/32 dev veth-podC)
  └── nftables filter forward chain (policy check)
  └── veth → Pod C
```

One extra L3 hop. Same nftables policy enforcement applies. No overlay, no tunnel.

---

## 4. rustables Integration — Unified L3/L4 Engine

### 4.1 nftables Tables Layout

```
table ip nat {
  chain prerouting  { type nat hook prerouting  priority -100; }
  chain postrouting { type nat hook postrouting priority -100; }
}

table ip filter {
  chain forward { type filter hook forward priority 0; policy drop; }
  chain input   { type filter hook input   priority 0; policy accept; }
  chain output  { type filter hook output  priority 0; policy accept; }
}
```

The `forward` chain has default policy `drop` — this is the baseline isolation. Everything that passes must be explicitly allowed.

### 4.2 Rule Compilation Order

Each rule set compiles to nftables rules in a specific position. The priority model (most specific wins) is enforced by ordering:

```
chain forward {
  # 1. VNet/Subnet NSG baseline (topology)
  ip saddr 10.42.1.0/24 ip daddr 10.42.2.0/24 drop       # deny spoke→spoke
  ip saddr 10.42.0.0/16 ip daddr 10.42.0.0/16 accept      # same-VNet allow

  # 2. NetworkPolicy selectors (workload policy)
  ip saddr @np:ns1.frontend ip daddr @np:ns1.backend accept
  ip saddr @np:default-deny drop

  # 3. Hub-and-Spoke transit rules
  ip saddr @hub-vnet ip daddr @spoke-vnet accept

  # 4. Default: drop (inherited from chain policy)
}
```

| Layer | Controls | Compiled from |
|---|---|---|
| NSG | Subnet-to-subnet by CIDR | VNet/Subnet CRDs |
| NetworkPolicy | Pod-to-pod by selector | `k8s-openapi` `NetworkPolicy` structs |
| Hub-and-Spoke | Transit routing between VNets | VNet hub/spoke CRDs |

### 4.3 Dynamic Sets for NetworkPolicy

Each `NetworkPolicy` with a `podSelector` creates an nftables set:

```
set np:default:allow-from-frontend {
  type ipv4_addr
  elements = { 10.42.0.2, 10.42.0.5 }  # pods matching frontend selector
}
```

When a pod starts or stops, its IP is added to or removed from all sets that its labels match. This happens atomically via `rustables::Batch`.

### 4.4 ClusterIP DNAT Maps

Each Service creates an nftables map:

```
map svc:nginx-80 {
  type ipv4_addr . inet_service : ipv4_addr . inet_service
  elements = {
    10.96.0.3 . 80 : 10.42.0.2 . 8080,
    10.96.0.3 . 80 : 10.42.0.3 . 8080,
  }
}
```

The prerouting rule uses the map directly:
```
ip daddr . tcp dport vmap @svc:nginx-80
```

When backends change (pod scale, rolling update), the map is updated atomically via `Batch`. No userspace proxy. No loopback alias. No `ensure_loopback_alias`.

---

## 5. Dual API — Kubernetes-Native + Azure CRDs

### 5.1 API Layers

| API | Source CRDs | Compiles to | Purpose |
|---|---|---|---|
| **Kubernetes-native** | `k8s-openapi` `Service`, `NetworkPolicy`, `Ingress`, `Pod` | nftables DNAT maps, filter sets, L7 routes | Workload-level networking |
| **Azure-style** | `VNet`, `Subnet`, `NSG`, `RouteTable`, `Hub`, `Spoke` | nftables forward rules, CIDR sets, route tables | Topology & isolation |

### 5.2 Priority Model

```
NSG baseline (subnet CIDR)            ─── evaluates first (top of forward chain)
NetworkPolicy (pod selector sets)     ─── evaluates second (more specific)
```

**Conflict resolution:** The more specific rule wins. Since nftables evaluates top-down:
- NSG drops traffic at subnet boundary → that traffic never reaches NetworkPolicy rules
- NetworkPolicy allows specific pod-to-pod → that rule is more specific than the NSG deny and appears later but with a narrower match

**Example — NSG denies subnet A→B, NetworkPolicy allows a specific pod:**
```
chain forward {
  ip saddr 10.42.1.0/24 ip daddr 10.42.2.0/24 drop            # NSG: deny subnet A→B
  ip saddr 10.42.1.5    ip daddr 10.42.2.3    accept           # NP: allow specific pod
}
```
Pod A (10.42.1.5) → Pod B (10.42.2.3): matches rule 2 (more specific), **allowed**.
Pod A (10.42.1.6) → Pod B (10.42.2.3): matches rule 1 but not rule 2, **dropped**.

This is the correct semantics: NSG is a baseline that can be overridden by explicit workload policy.

### 5.3 Reconciliation

Two independent controllers, both writing to the same nftables tables via `rustables::Batch`:

```
VNetController {
  watches: VNet, Subnet, NSG, Hub, Spoke CRDs
  writes: forward chain (CIDR rules), route table entries
  scope: topology-level
}

NetworkPolicyController {
  watches: Service, NetworkPolicy, Pod CRDs
  writes: nat chain (DNAT maps), forward chain (selector sets)
  scope: workload-level
}
```

Both use the same `rustables::Batch` API. Ordering within the forward chain is fixed (NSG rules first, NetworkPolicy rules second).

---

## 6. Multi-Node Architecture

```
Node A (10.0.0.1)              Node B (10.0.0.2)
┌──────────────────┐           ┌──────────────────┐
│ veth-podA ──► podA│           │ veth-podC ──► podC│
│  10.42.1.2/32     │           │  10.42.2.2/32     │
│ veth-podB ──► podB│           │ veth-podD ──► podD│
│  10.42.1.3/32     │           │  10.42.2.3/32     │
│                   │           │                   │
│ route:            │           │ route:            │
│  10.42.2.0/24 via │◄───────► │  10.42.1.0/24 via │
│  10.0.0.2         │           │  10.0.0.1         │
│                   │           │                   │
│ rustables:        │           │ rustables:        │
│  SNAT + DNAT + NP │           │  SNAT + DNAT + NP │
│  (same rules)     │           │  (same rules)     │
│                   │           │                   │
│ ingress (axum)    │           │ ingress (axum)    │
│ :80,:443,:22,:5432│           │ :80,:443,:22,:5432│
└──────────────────┘           └──────────────────┘
```

### 6.1 Cross-Node Traffic

Pod A (Node A, 10.42.1.2) → Pod C (Node B, 10.42.2.2):
1. Pod A sends to 10.42.2.2, goes through default route to veth
2. Host routing on Node A: 10.42.2.0/24 via 10.0.0.2 → eth0
3. Node B receives, host routing: 10.42.2.2/32 dev veth-podC
4. nftables forward chain on **both nodes** enforces policy
5. Pod C receives

### 6.2 Cross-Node ClusterIP DNAT

Same DNAT maps on every node. When a pod on Node A sends to a ClusterIP whose backends are on Node B:
1. Node A prerouting DNAT rewrites dest to 10.42.2.2:port
2. Host routing on Node A forwards to Node B
3. Node B routing delivers to pod

### 6.3 Cross-Node Ingress

Ingress on Node A accepts a connection for a service with all backends on Node B:
1. Node A axum accepts, Host header match → backend service
2. `find_endpoints()` returns 10.42.2.2:port
3. `splice` from client → 10.42.2.2:port through host routing
4. Extra L3 hop, but same splice zero-copy path

### 6.4 Node Discovery (MVP)

Static config:
```
z8s --node-name node-a --node-ip 10.0.0.1 --pod-cidr 10.42.0.0/16 \
    --peers node-b=10.0.0.2,node-c=10.0.0.3
```

Each node gets a /24 slice from the /16 (node-a: 10.42.1.0/24, node-b: 10.42.2.0/24). Cross-node routes added via `RTM_NEWROUTE` at startup.

> **⚠️ NOTE:** The store is per-node in-memory. `find_endpoints()` on Node A only returns pods on Node A. Cross-node DNAT map entries require a future shared store or consensus layer. For MVP, Services with cross-node backends work via DNS pinning (each node advertises only its local backends).

---

## 7. Implementation Phases

### Phase 1 — Veth Attachment + Host Routing Infrastructure

**Files:** `src/network/bridge.rs` → rename to `src/network/l3.rs`

- `create_veth(host_name, peer_name, netns_fd)` — RTM_NEWLINK veth with IFLA_NET_NS_FD (unchanged)
- `link_up(ifname)` — ioctl SIOCGIFFLAGS + SIOCSIFFLAGS (unchanged)
- `add_route(dst_cidr, gateway, ifindex)` / `del_route()` — RTM_NEWROUTE / RTM_DELROUTE
- `add_pod_route(pod_ip, veth_ifindex)` — /32 route for pod via host veth end
- `clean_orphan_veths()` — enumerate interfaces, RTM_DELLINK stale veth-* entries
- `enable_ip_forward()` — write `"1\n"` to `/proc/sys/net/ipv4/ip_forward`
- `assign_pod_ip(ip, prefix, ifname)` — RTM_NEWADDR inside pod netns
- `add_default_route(gateway, ifname)` — RTM_NEWROUTE inside pod netns

**No bridge creation.** No `ensure_loopback_alias`. Pod attachment is: create veth with peer in netns → add /32 route on host → assign IP + default route in child.

### Phase 2 — rustables Engine: SNAT + ClusterIP DNAT

**New crate dependency:** `rustables`

**New module:** `src/network/nftables.rs`

- Initialize nftables tables (`ip nat`, `ip filter`) with baseline chains
- `add_snat(pod_cidr, host_ifindex)` — MASQUERADE rule for pod outbound traffic
- `add_clusterip_dnat(cluster_ip, port, backends: Vec<(Ipv4Addr, u16)>)` — atomic DNAT map update via Batch
- `remove_clusterip_dnat(cluster_ip, port)` — remove map entry
- `update_clusterip_backends(cluster_ip, port, backends)` — swap backends atomically

**Also:**
- Add `PodIpAllocator` with `BTreeSet<u8>` free-list
- Remove `ensure_loopback_alias` — no longer needed
- Remove userspace ClusterIP proxy from `service_proxy.rs`

### Phase 3 — VNet / Subnet / NSG CRDs

**New CRDs in store:**
- `VNet` — CIDR + hub/spoke role
- `Subnet` — CIDR range + parent VNet
- `NSG` — security rules (allow/deny, src/dst CIDR or service tag, port/protocol)
- `Hub` / `Spoke` — topology references

**New module:** `src/network/vnet.rs`

- Compile NSG rules to nftables forward chain CIDR rules
- Compile Hub-and-Spoke topology to forward chain transit rules
- Default: deny all cross-subnet traffic except explicit NSG allows

### Phase 4 — Kubernetes NetworkPolicy

**No new CRDs** — uses existing `k8s-openapi` `NetworkPolicy` struct.

**New module:** `src/network/network_policy.rs`

- Watch `NetworkPolicy` + `Pod` resources from store
- For each `NetworkPolicy`:
  - Resolve `podSelector` → list of matching pod IPs
  - Resolve `namespaceSelector` → all pods in matching namespaces
  - Resolve `ipBlock` → CIDR set
  - Compile `ingress`/`egress` rules to nftables forward chain rules with dynamic sets
- On pod start/stop: atomically update all sets that its labels match
- On policy create/update: re-resolve all selectors and compile new batch

### Phase 5 — Remove Old Code

- Delete `port_publish.rs` entirely
- Delete `service_proxy.rs` entirely (replaced by nftables DNAT)
- Delete `ensure_loopback_alias` from `service_proxy.rs` (if not already removed)
- Remove `--net-backend=setns` flag

### Phase 6 — Multi-Node

- `--node-name`, `--node-ip`, `--peers` flags
- Per-node /24 subnet allocation
- Cross-node host routes at startup
- DNS pinning per node (local backends only)

### Phase 7 — L7 Ingress + API Gateway

- Ingress CRD with HTTP Host header, TLS SNI, and TCP port routing
- axum-based L7 controller (separate from nftables engine)
- API Gateway CRD (future — rate limiting, auth, path rewriting)

---

## 8. What Stays the Same

| Component | Notes |
|---|---|
| **Embedded DNS** | Unchanged. Resolves `<svc>.<ns>.svc.cluster.local` → ClusterIP. External domains forwarded to upstream DNS. |
| **`kubectl exec`** | Unchanged. Still uses `setns`. |
| **`kubectl logs`** | Unchanged. Ring buffer on host. |
| **Manifest watcher / controller** | Unchanged. Just add new CRD types to watch list. |
| **`copy_bidirectional`** | Still used by L7 ingress for splice forwarding. |
| **Loopback setup in pods** | `setup_loopback()` still called in child — `lo` is always needed. |
| **Existing sync-byte protocol** | Still used for parent-child ordering during veth creation. |

---

## 9. What Changes

| Current | New |
|---|---|
| Bridge (`z8s0`) | **Removed.** Pods attached via veth + /32 route only. |
| `ensure_loopback_alias` (RTM_NEWADDR on lo) | **Removed.** No ClusterIP on loopback. |
| Userspace ClusterIP TCP proxy | **Removed.** Replaced by nftables prerouting DNAT map. |
| `port_publish::publish_ports()` | **Removed.** Pod is directly reachable via its /32 route. |
| `run_forwarder()` + `connect_tcp_in_netns()` | **Removed.** No setns per connection. |
| `service_proxy.rs` `find_endpoints()` | **Removed.** Backend selection done by nftables map. |
| No L3/L4 policy | **Added.** Full nftables forward chain with NSG + NetworkPolicy. |
| No VNet abstraction | **Added.** VNet/Subnet/NSG/Hub/Spoke CRDs. |
| No per-pod /32 routes | **Added.** Each pod gets a /32 host route via its veth interface. |
| `iptables` / `nft` shell commands | **Never used.** All nftables via `rustables` FFI. |
| `--net-backend=setns` | **Removed.** Only one path. |

---

## 10. Config Reference (New Flags)

```
z8s [OPTIONS]

Networking:
  --pod-cidr <CIDR>           Pod IP allocation range         [default: 10.42.0.0/16]
  --service-cidr <CIDR>       ClusterIP allocation range       [default: 10.96.0.0/16]
  --ingress-ports <PORTS>     L7 ingress listen ports         [default: 80,443]
  --node-name <NAME>          This node's name                [default: hostname]
  --node-ip <IP>              This node's host IP             [auto: default route iface]
  --peers <PEERS>             Other nodes (k=v pairs)         [default: none]
```

---

## 11. Dependencies

### Rust Crates (new)

| Crate | Version | Purpose | License |
|---|---|---|---|
| `rustables` | 0.8 | nftables nfnetlink FFI (SNAT, DNAT, policy) | GPL-3.0 |
| `ipnetwork` | 0.21 | CIDR parsing (transitive via `rustables`) | MIT/Apache-2.0 |

All other operations use existing `libc` + `nix` for `RTM_NEWLINK`, `RTM_NEWADDR`, `RTM_NEWROUTE`.

### Host Packages

| Package | Required? |
|---|---|
| `iproute2` | **Not required** — all netlink via `libc` FFI |
| `iptables` / `nftables` | **Not required** — nftables via `rustables` raw nfnetlink |
| Kernel module `nf_tables` | **Required** — standard in all modern kernels |
| Kernel module `nf_nat` | **Required** — for DNAT/SNAT, standard in all modern kernels |

---

## 12. Error Handling

| Failure | Handling |
|---|---|
| `create_veth` fails | Log warning, pod runs with lo only |
| `RTM_NEWADDR` / `RTM_NEWROUTE` in child fails | Log warning, pod has lo only |
| `rustables` table init fails | Log error, abort startup — nftables is required |
| `rustables` DNAT map update fails | Log warning, update retried on next reconcile |
| `rustables` MASQUERADE fails | Log warning: "SNAT not configured — pods cannot reach internet" |
| Stale veth on pod restart | `clean_orphan_veths()` at startup removes orphans |
| ClusterIP DNAT map has no backends | Packets to ClusterIP are dropped (no DNAT match) — same as current behavior |
| NSG conflict with NetworkPolicy | More specific rule wins by evaluation order (see §5.2) |
| Cross-node route add fails | Log warning, pods on other nodes unreachable |

---

## 13. Test Plan

| Test | What it validates |
|---|---|
| Pod gets `eth0` with pod IP | `kubectl exec <pod> -- ip addr show eth0` |
| Pod can ping host gateway | `kubectl exec <pod> -- ping -c1 10.42.0.1` (host) |
| Pod A can ping Pod B (same host) | `kubectl exec podA -- ping -c1 10.42.0.3` |
| Pod A → Pod B port (direct) | `curl http://10.42.0.3:80` from pod A |
| ClusterIP service | `kubectl exec podA -- curl http://10.96.0.3:80` |
| ClusterIP updates on backend change | Scale deployment up/down, verify DNAT map via `nft list map` |
| SNAT for outbound traffic | `kubectl exec podA -- curl http://example.com` |
| HTTP ingress by Host header | `curl -H "Host: svc1.z8s.emo.net" http://localhost` |
| TCP/SNI ingress | `curl --resolve 'svc1.z8s.emo.net:443:127.0.0.1' https://svc1.z8s.emo.net` |
| NodePort | `curl http://localhost:3xxxx` |
| NetworkPolicy allow | Pod A can reach Pod B after `podSelector` match |
| NetworkPolicy deny | Pod A cannot reach Pod C (not in `podSelector`) |
| NSG deny subnet→subnet | Pod in subnet A cannot ping pod in subnet B |
| NSG + NetworkPolicy override | NSG denies subnet→subnet but NetworkPolicy allows specific pod pair |
| VNet hub-and-spoke | Spoke A → hub → Spoke B works; Spoke A → Spoke B direct fails |
| Cross-node pod-to-pod | Pod on Node A reaches pod on Node B |
| Cross-node ClusterIP | Pod on Node A reaches ClusterIP with backends on Node B |
| No bridge interfaces | `ip link show type bridge` returns empty |
| No `ensure_loopback_alias` | No `RTM_NEWADDR` for ClusterIPs on `lo` |
| No `PortPublish` code | All old port publishing code removed |

---

## 14. Migration Strategy

1. **Phase 1** — Veth + host route attachment (same as old Phase 1, no bridge)
2. **Phase 2** — `rustables` engine for SNAT + ClusterIP DNAT (replace userspace proxy)
3. **Phase 3** — VNet/Subnet/NSG CRDs + nftables forward rules
4. **Phase 4** — Kubernetes NetworkPolicy with dynamic pod selector sets
5. **Phase 5** — Delete old code (port publishing, service proxy, loopback alias)
6. **Phase 6** — Multi-node with cross-node routing
7. **Phase 7** — L7 ingress + API Gateway

No rollback to `--net-backend=setns` — the old port publishing architecture is replaced entirely. The bridge is never created. The loopback alias is never added.

---

## 15. Open Questions

| Question | Decision |
|---|---|
| Pod CIDR? | `10.42.0.0/16`, configurable via `--pod-cidr` |
| Only IPv4 for now? | Yes. IPv6 post-MVP. |
| Destroy veths on shutdown? | Yes — enumerate `veth-*` interfaces, RTM_DELLINK each. z8s.sh stop does the same. |
| ClusterIP DNAT or userspace proxy? | **DNAT via rustables.** Userspace proxy deleted entirely. |
| Bridge yes/no? | **No bridge.** Pure L3 via veth + host routes. |
| NetworkPolicy implementation? | Dynamic nftables sets compiled from `NetworkPolicy` `podSelector`. |
| VNet/Subnet/NSG as CRDs? | Yes — separate CRDs compiling to nftables forward chain rules. |
| Dual API conflict resolution? | More specific (narrower match) rule wins. NSG = CIDR, NetworkPolicy = pod IP. |
| Multi-node store? | MVP: per-node in-memory. DNS pinning for local backends only. Future: external store. |
| Outbound internet (SNAT)? | Phase 2 — `rustables` MASQUERADE rule. No host package needed. |
| TLS termination in ingress? | Phase 7+. For MVP, TLS pass-through with SNI routing via axum. |
