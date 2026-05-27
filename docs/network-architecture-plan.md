# Network Architecture Plan

> **Date:** 2026-05-27 (v4 — unified L3/L4 engine, final design)
> **Goal:** Replace setns-based port publishing with a single nftables-backed network engine — real pod IPs, zero-copy forwarding, full NetworkPolicy + NSG, Azure-style VNet/Subnet, ClusterIP via DNAT, multi-node, no host package dependencies.
> **Contracts:**
> - No shelling out to `ip`, `iptables`, `nft`, or any host binary
> - All L3/L4 enforcement via `rustables` (nfnetlink FFI)
> - All L7 logic via built-in axum (ingress, API gateway)
> - Pod attachment via veth pairs with host routes — **no bridge**
> - rtnetlink operations (`RTM_NEWLINK`, `NEWADDR`, `NEWROUTE`) via minimal `libc` FFI (~50 lines)
> - One unified pool allocator for all resource types — no special cases in the engine

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

## 2. Proposed Architecture — Unified Network Engine

```
┌─────────────────────────────────────────────────────────────────────┐
│  Z8S HOST                                                          │
│                                                                    │
│  ┌───────────────────────────────────────────────────────────────┐ │
│  │  L7 INGRESS (axum) — separate process, NOT part of engine     │ │
│  │  :80  HTTP  Host header → backend pod IP                      │ │
│  │  :443 TLS  SNI          → backend pod IP     (Phase 7)        │ │
│  │  :22       port match   → backend pod IP                      │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                    │
│  ┌───────────────────────────────────────────────────────────────┐ │
│  │  NETWORK ENGINE — unified, resource-agnostic                   │ │
│  │                                                               │ │
│  │  Pool Allocator (one code path for everything):               │ │
│  │    pod-cidr   (10.0.0.0/8)   → per-VNet /20 sub-allocation   │ │
│  │    svc-cidr   (10.96.0.0/16) → flat per-ClusterIP            │ │
│  │    public-v4  (<user>)       → host interface                 │ │
│  │    public-v6  (<user>/64)    → host interface                 │ │
│  │                                                               │ │
│  │  Engine::register(resource, pool):                            │ │
│  │    1. allocate IP from pool                                   │ │
│  │    2. mark in-use (BTreeSet free-list)                        │ │
│  │    3. return IP — nothing else                                │ │
│  │                                                               │ │
│  │  Engine::program(resource):                                   │ │
│  │    if veth peer exists        → add /32 route via veth        │ │
│  │    if Service references pod  → add DNAT map entry            │ │
│  │    if Ingress references svc  → add L7 route                  │ │
│  │    if DNS requested           → add A/AAAA record             │ │
│  │    if labels match NetworkPolicy → add to nftables set        │ │
│  │    if PublicIP is assigned    → assign to host iface + DNAT   │ │
│  │    if none of the above       → nothing extra (job = done)    │ │
│  │                                                               │ │
│  │  nftables (via rustables):                                    │ │
│  │    nat:      prerouting (DNAT) + postrouting (SNAT)           │ │
│  │    filter:   forward (NSG + NetworkPolicy)                    │ │
│  │    sets:     dynamic pod membership for NetworkPolicy         │ │
│  │    maps:     ClusterIP → backend pod IPs (round-robin)        │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                    │
│  ┌───────────────────────────────────────────────────────────────┐ │
│  │  POD ATTACHMENT — veth pairs, no bridge (pure L3)             │ │
│  │                                                               │ │
│  │  host route: 10.80.0.2/32 dev veth-podA                      │ │
│  │  host route: 10.80.0.3/32 dev veth-podB                      │ │
│  │  host route: 10.80.16.0/24 via 10.0.0.2 (multi-node)        │ │
│  │                                                               │ │
│  │  veth-podA ── peer ──► podA (10.80.0.2/20, default route)   │ │
│  │  veth-podB ── peer ──► podB (10.80.0.3/20, default route)   │ │
│  │                                                               │ │
│  │  No bridge. No L2. Pure L3 routed.                           │ │
│  └───────────────────────────────────────────────────────────────┘ │
│                                                                    │
│  ip_forward=1 (written once at startup)                            │
└─────────────────────────────────────────────────────────────────────┘
```

### Key Design Decisions

| Decision | Rationale |
|---|---|
| **No bridge** | Pods are L3 neighbors via host routes. No L2 isolation leaks, no ARP tables, no STP. VNet boundary = route aggregate boundary. |
| **veth for attachment only** | Each pod gets one veth pair. Host end is a plain interface with a /32 route. No bridge port, no L2 forwarding. |
| **`rustables` for all L3/L4** | SNAT, DNAT, NetworkPolicy, NSG — single engine, atomic batch updates, zero-copy kernel path. |
| **ClusterIP via nftables DNAT** | Replace userspace TCP proxy + loopback alias with kernel DNAT map. No `ensure_loopback_alias` needed. Zero per-packet userspace. |
| **Unified pool allocator** | One code path for pods, services, public IPs, jobs. No special cases in the engine. |
| **Resource-agnostic programming** | Engine does the same `allocate → program` for every resource. Programs by reference (if a Service references a pod, DNAT is added; if nothing references a job, no extra rules). |
| **L7 ingress separate (axum)** | nftables is L3/L4 only. HTTP Host/SNI/path routing stays in userspace where it belongs. |
| **No host package deps** | Everything through `libc` FFI or `rustables`. Zero shell commands. |

---

## 3. CIDR & Pool Architecture

### 3.1 Cluster Pod CIDR — `10.42.0.0/16` (default)

Default. All pod IPs come from this range. Configurable via `--pod-cidr`. Use a larger range like `10.0.0.0/8` only if you have no IP overlap with existing cloud/on-prem 10.x.x.x networks. The `/16` default avoids the most common deployment conflict.

```
Reserved:
  10.0.0.0/8  → Pod IPs only
  10.96.0.0/16 → ClusterIPs (Service CIDR)
```

The CIDR is sub-allocated into VNets:

```
10.42.0.0/16 (65534 IPs)
  ├── VNet "prod"         : 10.42.0.0/20   (4094 IPs, auto)
  ├── VNet "staging"      : 10.42.16.0/20  (4094 IPs, auto)
  ├── VNet "shared"       : 10.42.32.0/20  (4094 IPs, auto)
  └── ...up to 16 VNets at /20

With --pod-cidr 10.0.0.0/8:
  10.0.0.0/8 (16.7M IPs)
    ├── VNet "prod"         : 10.80.0.0/20   (4094 IPs, auto)
    ├── VNet "staging"      : 10.80.16.0/20  (4094 IPs, auto)
    └── ...up to 256 VNets at /20
```

| Resource | Pool | Allocation | Scope |
|---|---|---|---|
| Pods (long-lived) | `--pod-cidr` | Sub-allocated per VNet as /20 | Per-VNet |
| Jobs (ephemeral) | Same pool, same VNet | Same allocator, no special pool | Same VNet |
| ClusterIP | `10.96.0.0/16` | Flat pool, not per-VNet | Cluster-wide |
| Public IPv4 | User-provided | Assigned to host interface | Host-level |
| Public IPv6 | User-provided /64 | Assign /128 per service to host interface | Host-level |

### 3.2 VNet → Namespace Mapping

Every namespace gets a **default VNet** at namespace creation:

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: production
  annotations:
    z8s.io/vnet: prod     # default: namespace name
    z8s.io/vnet-cidr: 10.80.0.0/20  # auto-allocated if not set
```

Users can override:
- Move a namespace to a different VNet via annotation
- Expand the VNet CIDR (e.g., from /20 to /19) without recreating pods
- Assign the same VNet to multiple namespaces (shared network)

The VNet is the **network boundary** — pods in different VNets cannot communicate by default (enforced by nftables forward chain).

### 3.3 Subnets (Optional)

Subnets are not required. A VNet with no subnets = one flat /20 where all pods communicate freely.

```yaml
apiVersion: z8s.io/v1
kind: Subnet
metadata:
  name: web
spec:
  vnet: prod
  cidr: 10.80.0.0/24
```

Subnets exist to:
- Apply NSG rules between groups within the same VNet
- Organize pods by function (web, app, db)
- Default between subnets depends on NSG rules (no implicit isolation)

---

## 4. Data Flows

### 4.1 Pod → Pod (same VNet, same node)

```
Pod A (10.80.0.2) → 10.80.0.3:80
  └── veth-podA → host routing (10.80.0.3/32 dev veth-podB)
  └── nftables filter forward chain (policy check)
  └── veth-podB → Pod B
```

One L3 kernel forward. No userspace. No bridge.

### 4.2 Pod → Pod (different VNet, same node)

```
Pod A (10.80.0.2, VNet=prod) → 10.80.20.3:80 (VNet=staging)
  └── veth → host routing (10.80.20.3/32 dev veth-podX)
  └── nftables filter forward chain:
        ip saddr @vnet-prod ip daddr @vnet-staging drop  ← NSG default deny
  └── DROPPED (unless explicit NSG allow or NetworkPolicy override)
```

### 4.3 Pod → ClusterIP Service (via nftables DNAT)

```
Pod A (10.80.0.2) → 10.96.0.3:80
  └── veth → host
  └── nftables prerouting: ip daddr 10.96.0.3 tcp dport 80
        dnat to numgen inc mod 2 map { 0 : 10.80.0.3:8080, 1 : 10.80.0.4:8080 }
  └── host routing → veth → backend pod
```

No `ensure_loopback_alias`. No userspace proxy. Atomically updated via `rustables::Batch`.

### 4.4 External → Ingress → Pod

```
Client → svc1.z8s.emo.net (Cloudflare A → host IP)
  └── host:80 (axum listener)
  └── match Host header → backend service
  └── resolve backend pod IPs
  └── splice to 10.80.0.3:8080 (kernel splice, zero-copy)
```

### 4.5 Pod → Internet

```
Pod A (10.80.0.2) → 1.1.1.1:80
  └── veth → host routing → eth0 (default route)
  └── nftables postrouting: masquerade (SNAT to host IP)
```

MASQUERADE rule applies per-VNet. Hub VNet has SNAT, spoke VNets don't.

### 4.6 Pod → Pod (cross-node)

```
Node A (10.0.0.1)                Node B (10.0.0.2)
Pod (10.80.0.2)                  Pod (10.80.20.3)
  └── veth                          └── veth
  └── host routing                   └── host routing
  └── 10.80.16.0/20 via 10.0.0.2    └── 10.80.0.0/20 via 10.0.0.1
  └── eth0 ──── network ──── eth0
```

One extra L3 hop. Same nftables policy enforcement on both nodes.

---

## 5. Pool Allocator — Unified for All Resources

```rust
struct IpPool {
    cidr: Ipv4Cidr,           // e.g., 10.80.0.0/20
    free: BTreeSet<u32>,       // available IPs (host addresses)
}

impl IpPool {
    fn allocate(&mut self) -> Option<Ipv4Addr>;
    fn release(&mut self, ip: Ipv4Addr);
    fn count_free(&self) -> usize;
    fn expand(&mut self, new_cidr: Ipv4Cidr);  // add new range to free set
}
```

Pool is per-VNet (for pod CIDRs) and single (for service CIDR). The engine calls `allocate()` when any resource requests an IP. The resource type is irrelevant to the allocator.

The only difference between a pod and a job: the job releases its IP on completion. The pod releases its IP on deletion. Same allocator, same pool, no special cases.

---

## 6. nftables Integration — Unified L3/L4 Engine

### 6.1 Table Layout

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

Default policy on forward chain is `drop`. All forwarding is explicit.

**System hardening on bridge/VNet hosts:**

- `net.ipv4.conf.all.rp_filter = 1` — strict reverse-path filtering prevents IP spoofing between VNets. A pod in VNet A cannot send packets with a VNet B source IP.
- `net.ipv4.conf.all.forwarding = 1` — already set by `enable_ip_forward()`.
- `net.ipv4.conf.all.arp_announce = 2` — always use the best local address for ARP replies, prevents cross-VNet ARP leaks on multi-homed hosts.

### 6.2 Rule Order — Priority Model

```
chain forward {
  # 0. Connection tracking — MUST be first
  ct state {established, related} accept

  # 1. NSG baseline (subnet CIDR rules)
  ip saddr @vnet-prod ip daddr @vnet-staging drop

  # 2. NetworkPolicy (pod selector sets)
  ip saddr @np:frontend ip daddr @np:backend accept

  # 3. Hub-and-Spoke transit
  ip saddr @spoke-a ip daddr @spoke-b meta mark 0x1 accept

  # 4. Default: drop (inherited from chain policy)
}
```

The `ct state` rule at position 0 is critical — without it, return traffic for established connections is dropped by the default policy, breaking all TCP flows.

| Layer | Controls | Compiled from |
|---|---|---|
| NSG | Subnet-to-subnet by CIDR | VNet/Subnet CRDs |
| NetworkPolicy | Pod-to-pod by selector | `k8s-openapi` `NetworkPolicy` |
| Hub-and-Spoke | Transit routing between VNets | VNet hub/spoke CRDs |

**Conflict resolution:** more specific (narrower match) rule wins. NSG is CIDR-based (broad), NetworkPolicy is pod IP-based (specific). If NSG denies subnet A→B but NetworkPolicy allows pod X→Y, the NP rule is more specific and wins.

### 6.3 ClusterIP DNAT Maps

```rust
// Per-service nftables map, updated atomically
map svc:nginx-80 {
  type ipv4_addr . inet_service : ipv4_addr . inet_service
  elements = {
    10.96.0.3 . 80 : 10.80.0.3 . 8080,
    10.96.0.3 . 80 : 10.80.0.4 . 8080,
  }
}

// Pterouting rule uses the map
ip daddr . tcp dport vmap @svc:nginx-80
```

When backends change (scale up/down, rolling update), the map is replaced atomically via `Batch`.

### 6.4 Dynamic NetworkPolicy Sets

```rust
// Each podSelector creates a named set
set np:default:allow-from-frontend {
  type ipv4_addr
  elements = { 10.80.0.2, 10.80.0.5 }  // pods matching labels
}
```

On pod start/stop: add/remove IP from all matching sets atomically via `Batch`.

---

## 7. Dual API Architecture

| API | CRDs | Compiles to | Purpose |
|---|---|---|---|
| **Kubernetes-native** | `k8s-openapi` `Service`, `NetworkPolicy`, `Ingress`, `Pod` | nftables DNAT maps, filter sets, L7 routes | Workload-level networking |
| **Azure-style** | `VNet`, `Subnet`, `NSG`, `RouteTable`, `PublicIP`, `Hub`, `Spoke` | nftables forward rules, CIDR sets, route tables | Topology & isolation |

Two independent controllers, both writing to the same nftables tables via `rustables::Batch`:

```
VNetController:
  watches: VNet, Subnet, NSG, Hub, Spoke, PublicIP
  writes: forward chain (CIDR rules), route table entries, host interface assignment
  scope: topology-level

NetworkPolicyController:
  watches: Service, NetworkPolicy, Pod, Ingress
  writes: nat chain (DNAT maps), forward chain (selector sets), L7 routes
  scope: workload-level
```

Both controllers use the same `PoolAllocator`. Both call `Engine::register()` and `Engine::program()`.

---

## 8. Hub-and-Spoke Topology

```
Internet ──► Hub VNet (10.80.0.0/20)
               │  SNAT: yes
               │  Ingress: yes
               │
               │──► Spoke A (10.80.16.0/20) — DB, no internet
               │──► Spoke B (10.80.32.0/20) — app, no internet
               │
               DNS: global — all names resolve from any VNet
               NSG enforces the actual access
```

### 8.1 NSG Rules for Hub-and-Spoke

```
chain forward {
  # Spoke → Spoke: denied
  ip saddr @spoke-a ip daddr @spoke-b drop
  ip saddr @spoke-b ip daddr @spoke-a drop

  # Hub → Spoke: allowed (hub can reach services)
  ip saddr @hub ip daddr @spoke-a accept
  ip saddr @hub ip daddr @spoke-b accept

  # Spoke → Hub: allowed (spoke can reach hub for transit)
  ip saddr @spoke-a ip daddr @hub accept

  # Spoke → Internet: denied (no MASQUERADE)
  ip saddr @spoke-a oif eth0 drop

  # Hub → Internet: allowed (MASQUERADE applies)
  oif eth0 accept
}
```

### 8.2 Private DNS + Spoke Transit

- DNS is global — `db.myapp` resolves from any VNet
- NSG is the enforcement boundary
- Hub has ingress that routes to spoke backends (via ClusterIP or direct pod IP)
- Spoke default route only covers internal CIDR (`10.0.0.0/8`), no default gateway

---

## 9. L7 Ingress (Phase 7)

| Feature | Status |
|---|---|
| HTTP Host header routing | MVP |
| TCP port routing | MVP |
| TLS SNI passthrough | MVP |
| TLS termination (decrypt at ingress) | Phase 7 |
| Auto-TLS via Let's Encrypt | Phase 7 — investigate Rust ACME crates vs implement HTTP-01 ourselves |
| mTLS to backend | Phase 7 — depends on TLS termination |
| L7 NSG rules (method, header, cookie filtering) | Phase 7 — at ingress layer, not nftables |
| PublicIP CRD (allocate + attach to resource) | Phase 7 |
| API Gateway CRD (rate limiting, auth) | Future |

---

## 10. Multi-Node

### 10.1 Node Discovery (Deferred — Will Investigate at Phase 6)

Approaches to investigate at Phase 6:

| Approach | Pros | Cons |
|---|---|---|
| **SWIM gossip** (memberlist) | Fully decentralized, no SPOF, production-grade library | ~300 lines to implement; memberlist is Go — need Rust implementation |
| **Join-handshake push** | Simple (~100 lines), explicit control | Less resilient to partitions |
| **k3s-style token join** | Proven model, tokens provide auth | Needs one node to accept the join |
| **DNS-based discovery** | Zero extra mechanism | DNS TTL race conditions |

**Note:** Node discovery determines how IPAM Level 1 (the /24 bitmap) is managed. See §10.3.

### 10.2 Cross-Node Dataplane (Design Locked)

Regardless of discovery mechanism, once a node knows a peer's pod CIDR:

```
Each node maintains:
  kernel routes:  10.42.X.0/24 via <peer-host-ip>
  nftables DNAT:  local maps include peer pod IPs as backends
  nftables NSG:   same rules apply to cross-node traffic
```

The discovery mechanism only affects how the route table and DNAT maps are populated — the dataplane itself is identical whether the peer was discovered via gossip, static config, or token join.

### 10.3 Multi-Node IPAM — Two-Level Hierarchical (Design Locked)

Single-node IPAM uses a local BTreeSet (see §5). Multi-node extends this with a two-level scheme that piggybacks on node discovery:

```
Level 1 — global:  bitmap of /24 blocks per VNet, owned by the "oldest" node
Level 2 — local:   BTreeSet on each node, sub-allocates within its /24
```

**Join flow:**
```
Node B → Node A: POST /join { host_ip, token, node_name }
Node A:
  1. validates token
  2. picks a free /24 from the bitmap for Node B's VNet
  3. records: node-b → 10.42.1.0/24
  4. returns: { assigned_cidr: "10.42.1.0/24", peer_list: [...] }

Now Node A and Node B route each other's /24.
Node B allocates pod IPs locally from its BTreeSet within 10.42.1.0/24.
No contention, no coordination per pod.
```

**On node death:** Heartbeat from node discovery detects death (~60s timeout). Bitmap owner marks the dead node's /24 as free.

**Bitmap ownership:** The first node to start owns the bitmap. On join, the bitmap owner is known. If the bitmap-owner node dies, the next node takes over (the join handshake transfers ownership).

**Exhaustion:** If a node exhausts its /24, it requests an additional /24 from the bitmap owner — same join flow.

**Why this over alternatives:**

| Approach | Why not chosen |
|---|---|
| Gossip-based CRDT | Too complex for <50 nodes; convergence delays |
| etcd/Consul | Adds external dependency |
| Per-pod CRD (Whereabouts) | CRD sprawl, needs k8s API |
| Static pre-config | Fragile, no dynamic join |
| **Two-level + join handshake** | **Chosen — ~300 lines, no deps, works with any discovery mechanism** |

### 10.2 Cross-Node Dataplane (Design Locked)

Regardless of discovery mechanism, once a node knows a peer's pod CIDR:

```
Each node maintains:
  kernel routes:  10.80.X.0/24 via <peer-host-ip>
  nftables DNAT:  local maps include peer pod IPs as backends
  nftables NSG:   same rules apply to cross-node traffic
```

The discovery mechanism only affects how the route table and DNAT maps are populated — the dataplane itself is identical whether the peer was discovered via gossip, static config, or token join.

---

## 11. IPv6

| Feature | Status |
|---|---|
| Each pod gets IPv6 /128 from the user's /64 | Later phase |
| Ingress binds on both `[::]:80` and `0.0.0.0:80` | Later phase |
| nftables DNAT supports AF_INET6 | Later phase (rustables supports it) |
| PublicIP CRD for IPv6 allocation | Later phase |

Concrete approach to investigate when implementing: `--ipv6-prefix 2001:db8::/64` flag, allocator gives each pod a `/128` from it alongside the IPv4 address. The engine treats IPv6 identically to IPv4 — same pool, same programming, same nftables rules with `ip6` family.

---

## 12. Resource Type Summary

| Resource | Gets IP? | Gets /32 route? | Gets DNAT? | Gets DNS? | Gets L7 route? | Gets policy set? | Pool |
|---|---|---|---|---|---|---|---|
| Pod | Yes | Yes | Only if Service matches | Only if headless | No | Yes, if labels match NetworkPolicy | VNet |
| Job | Yes | Yes | No | No | No | Yes, if labels match NetworkPolicy | VNet |
| Service (ClusterIP) | Yes | No | N/A (source) | Yes | No | No | `--service-cidr` |
| Service (NodePort) | No | No | N/A (host port) | Yes | No | No | N/A |
| PublicIP | Yes | N/A | Yes (to backend) | No (external DNS) | No | No | User-provided |
| Ingress | No | No | N/A | No | Yes | No | N/A |
| VNet | Yes (CIDR) | No | N/A | No | No | No | From pod CIDR |

The engine doesn't have a match statement on resource types. It checks **what references the resource** to decide what to program:

```
fn program(resource, ctx):
  for each ref in store.references(resource):
    match ref.type:
      Service   → add_dnat(ref, resource.ip)
      Ingress   → add_l7_route(ref, resource.ip)
      PolicySet → add_to_set(ref, resource.ip)
      DNSRecord → add_dns_record(ref, resource.ip)
  if resource.has_veth:
    add_route(resource.ip, resource.veth_ifindex)
```

If nothing references the resource, nothing extra happens. Jobs naturally get nothing extra. No special case needed.

---

## 13. Implementation Phases

### Phase 0 — Test Scenarios

Write integration test scripts (shell + YAML) that define success for every feature before implementation begins. Each test:
1. Starts z8s with the relevant config
2. Applies YAML manifests
3. Asserts expected behavior (connectivity, isolation, DNS, etc.)
4. Cleans up

### Phase 1 — Veth Attachment + Host Routing

- `create_veth()` — RTM_NEWLINK (existing pattern, no bridge)
- `add_route()` / `del_route()` — RTM_NEWROUTE
- `add_pod_route(pod_ip, veth_ifindex)` — /32 route via host veth end
- `assign_pod_ip()` + `add_default_route()` — inside pod netns (RTM_NEWADDR)
- `enable_ip_forward()` — write `"1\n"` to `/proc/sys/net/ipv4/ip_forward`
- `clean_orphan_veths()` — RTM_DELLINK stale veth-* entries
- Pool allocator with `BTreeSet<u8>` free-list

### Phase 2 — Pool Allocator + nftables Engine (SNAT + ClusterIP DNAT)

- **Validate rustables early:** Before building the full engine, write a small validation script that exercises the three operations we need (SNAT MASQUERADE, DNAT map, dynamic set add/remove). If rustables fails any of these, implement the fallback raw-netlink approach before committing the architecture.
- New crate: `rustables`
- Pool allocator (unified for all resource types)
- nftables table init (`ip nat`, `ip filter`, baseline chains) with conntrack rules
- `add_snat(pod_cidr)` — MASQUERADE rule via rustables
- `add_clusterip_dnat(cluster_ip, port, backends)` — DNAT map via rustables Batch
- `remove_clusterip_dnat()`, `update_clusterip_backends()`
- `PodIpAllocator` moved to unified pool
- Remove `ensure_loopback_alias`, remove userspace proxy code

### Phase 3 — VNet / Subnet / NSG CRDs

- `VNet`, `Subnet`, `NSG`, `Hub`, `Spoke`, `RouteTable` CRDs
- Compile NSG rules to nftables forward chain CIDR rules
- Compile Hub-and-Spoke to forward chain transit rules
- Default: deny all cross-VNet traffic
- VNet CIDR allocation from cluster pod CIDR (/20 default per VNet)

### Phase 4 — Kubernetes NetworkPolicy

- Full `k8s-openapi` `NetworkPolicy` support
- Dynamic nftables sets per `podSelector` / `namespaceSelector`
- On pod start/stop: update all matching sets atomically
- NSG + NetworkPolicy priority model (more specific wins)

### Phase 5 — Remove Old Code

- Delete `port_publish.rs`, `service_proxy.rs`, `ensure_loopback_alias`
- Remove `--net-backend=setns` flag

### Phase 6 — Multi-Node

- Investigate discovery approaches: SWIM gossip vs join-handshake push vs k3s-style token
- Cross-node host routes
- Cross-node DNAT map synchronization
- DNS pinning per node

### Phase 7 — L7 Ingress + TLS + Public IP

- Ingress CRD with HTTP Host header, TCP port, TLS SNI routing
- TLS termination + re-encryption (investigate tokio-rustls vs tokio-native-tls)
- Auto-TLS via Let's Encrypt (investigate Rust ACME libs)
- PublicIP CRD (allocate IPv4/IPv6, attach to service/ingress)
- L7 NSG rules (HTTP method, headers, cookies — at axum layer)
- Dual-stack listener (IPv4 + IPv6)

---

## 14. Test Matrix (Phase 0)

### A. Pod Attachment & Basic Connectivity

| # | Test | Expected |
|---|---|---|
| A1 | Pod starts with `eth0` from its VNet CIDR | `ip addr` shows expected `10.X.Y.Z/XX` |
| A2 | Pod pings same-VNet peer (same node) | Success |
| A3 | Pod pings same-VNet peer (cross-node) | Success |
| A4 | Pod cannot ping different-VNet pod (default deny) | Failure |
| A5 | Pod gets new IP after delete/recreate | Different IP |
| A6 | Host has /32 route for each pod via veth | `ip route` |
| A7 | No bridge interfaces | `ip link show type bridge` empty |
| A8 | Job gets IP + route, no DNAT, no DNS | Route exists, no DNAT entry |

### B. ClusterIP DNAT

| # | Test | Expected |
|---|---|---|
| B1 | Pod reaches ClusterIP:port, hits backend | Successful response |
| B2 | Round-robin across multiple backends | Traffic distributed |
| B3 | DNAT updates on backend crash+replace | New IP in map within ~1s |
| B4 | Empty ClusterIP drops traffic | Timeout |
| B5 | NodePort works | External access succeeds |
| B6 | No ClusterIP on any interface | `ip addr show` clean |
| B7 | No userspace proxy for ClusterIP | `ss -tlnp` clean |

### C. SNAT & Outbound

| # | Test | Expected |
|---|---|---|
| C1 | Pod (hub VNet) reaches internet | Success, source = host IP |
| C2 | Pod (spoke VNet) cannot reach internet | Timeout |
| C3 | Pod-to-pod traffic not SNATted | Source = pod IP |

### D. VNet / Subnet / NSG

| # | Test | Expected |
|---|---|---|
| D1 | Default VNet per namespace | Pods in namespace get VNet IPs |
| D2 | Different VNets cannot communicate | Blocked |
| D3 | NSG allow subnet A→B port 5432 | Specific port allowed, others blocked |
| D4 | NSG deny between subnets | All traffic blocked |
| D5 | NSG + NetworkPolicy override | Specific pod allowed despite NSG deny |
| D6 | Hub reaches spoke | Allowed |
| D7 | Spoke cannot reach spoke (direct) | Blocked |
| D8 | Spoke reaches spoke via hub transit | Allowed with meta mark |

### E. NetworkPolicy

| # | Test | Expected |
|---|---|---|
| E1 | `podSelector` allow | Matching pods can communicate |
| E2 | `namespaceSelector` allow | All pods in namespace can communicate |
| E3 | `ipBlock` allow/deny | External CIDR filtered |
| E4 | Dynamic set update on pod start/stop | Set membership updates atomically |

### F. DNS

| # | Test | Expected |
|---|---|---|
| F1 | `<svc>.<ns>.svc.cluster.local` → ClusterIP | Resolves |
| F2 | External domains from pods | Forwarded to upstream DNS |
| F3 | Private VNet names resolve globally | `db.myapp` resolves from any VNet |
| F4 | NSG enforces access (not DNS) | IP resolves but traffic blocked by NSG |

### G. L7 Ingress

| # | Test | Expected |
|---|---|---|
| G1 | HTTP by Host header | Correct backend |
| G2 | TLS by SNI | Correct backend |
| G3 | TCP by port | Correct backend |
| G4 | TLS termination + re-encryption | Works |
| G5 | Auto-TLS cert provisioning | HTTPS works without manual config |
| G6 | L7 NSG — block method/header | 403 |
| G7 | Ingress → spoke backend (hub-and-spoke) | Works |

### H. Multi-Node

| # | Test | Expected |
|---|---|---|
| H1 | Node joins cluster | Route + DNAT sync established |
| H2 | Node fails | Peers detect and remove entries |
| H3 | Cross-node pod-to-pod | Works (one L3 hop) |
| H4 | Cross-node ClusterIP | Works |
| H5 | Cross-node ingress | Works |

### I. Edge Cases

| # | Test | Expected |
|---|---|---|
| I1 | VNet CIDR full | Pod creation fails with clear error |
| I2 | nftables init fails | Z8s refuses to start |
| I3 | SNAT fails | Warning logged, pods no internet |
| I4 | 1000 concurrent connections through DNAT | All succeed |
| I5 | 10 pods/sec start/stop for 60s | All IPs reclaimed, no leaks |

---

## 15. Error Handling

| Failure | Behaviour | Graceful? |
|---|---|---|
| `create_veth` fails | Pod starts with lo only, warning logged | Yes |
| `add_route` fails (RTM_NEWROUTE) | Pod starts with lo only, warning logged | Yes |
| `rustables` table init fails | z8s refuses to start — nftables is required | No |
| `add_snat` MASQUERADE fails | Warning logged: "SNAT not configured — pods cannot reach internet" | Yes |
| `add_clusterip_dnat` fails | Warning logged, retried on next reconcile cycle | Yes |
| Pool allocator exhausted (VNet full) | Pod creation fails with clear error: "no IPs available in VNet `<name>`" | No |
| Stale veth on restart | `clean_orphan_veths()` at startup removes orphans before creates | Yes |
| DNAT map has no backends | Packet dropped at prerouting (no DNAT match = no route) | Yes |
| Cross-node route fails | Warning logged, peer node unreachable | Yes |
| Node join with invalid token | Rejected with authentication error | Yes |
| Node fails / network partition | Other nodes detect via timeout, remove routes and DNAT entries | Yes |
| `rustables::Batch` conflict (two controllers simultaneously) | Batch ordering is deterministic — later batch overwrites earlier on same rule | Yes |
| ClusterIP DNAT update during backend churn | Map updated atomically via Batch — no partial state | Yes |

### SNAT Idempotency

Applying the MASQUERADE rule via `rustables::Batch` is idempotent — the `add` operation creates the rule if it doesn't exist. Calling `add_snat()` multiple times on restart does not duplicate rules. No `-C` check needed (unlike iptables).

---

## 16. Dependencies

### Rust Crates

| Crate | Version | Purpose | Affects phases |
|---|---|---|---|---|
| `rustables` | 0.8 | nftables nfnetlink FFI | Phase 2+ |
| `ipnetwork` | 0.21 | CIDR parsing (transitive via rustables) | Phase 2+ |

Kept: `libc` + `nix` for rtnetlink FFI (veth, routes, IP assignment).

**rustables risk note:** The crate is maintained but acknowledges "rough edges." Alternatives investigated:
- `nftables-rs` (JSON API) — shells out to `nft` binary, requires host package ❌
- Mullvad `nftnl` (FFI bindings to libnftnl C library) — requires `libnftnl-dev` system dep at build time ❌
- **Fallback:** If rustables proves insufficient for our specific operations (SNAT, DNAT maps, sets, forward rules), implement the needed operations via raw `NFNL_SUBSYS_NFTABLES` netlink using the same `libc` FFI pattern as rtnetlink. The surface area is small (~100 lines).

### Host Packages

| Package | Required? |
|---|---|
| `iproute2` | **Not required** |
| `iptables` / `nftables` | **Not required** |
| Kernel module `nf_tables` | Required (standard in all modern kernels) |
| Kernel module `nf_nat` | Required (standard in all modern kernels) |

---

## NOTE: One IP Per Pod

Each Pod resource gets exactly **one IP** from the pool. All containers within a Pod share the same network namespace (Linux kernel behaviour). The pool allocator tracks Pod resources, not containers. Jobs are Pods in terms of networking — same one-IP-per-resource model.

---

## 17. Config Reference

```
z8s [OPTIONS]

Networking:
  --pod-cidr <CIDR>             Pod IP allocation range         [default: 10.42.0.0/16]
  --service-cidr <CIDR>         ClusterIP allocation range       [default: 10.96.0.0/16]
  --vnet-cidr-size <PREFIX>     Default VNet CIDR size           [default: 20]
  --ipv6-prefix <PREFIX>        IPv6 /64 block (future)          [default: none]
  --ingress-ports <PORTS>       L7 ingress listen ports          [default: 80,443]
  --node-name <NAME>            This node's name                 [default: hostname]
  --node-ip <IP>                This node's host IP              [auto: default route iface]
  --peers <PEERS>               Other nodes (k=v pairs)          [default: none]
```

---

## 18. Pending Decisions (Investigate Later)

| Topic | Options to investigate | Affects phase |
|---|---|---|
| **Node discovery** | SWIM gossip / join-handshake push / k3s-style token / DNS-based | Phase 6 |
| **Multi-node IPAM bitmap ownership** | First-node-owns vs elected leader vs replicated via gossip | Phase 6 |
| **TLS termination** | `tokio-rustls` (pure Rust) vs `tokio-native-tls` (OpenSSL, already linked) vs custom | Phase 7 |
| **Auto-TLS / ACME** | Existing Rust crate vs implement HTTP-01 ourselves vs shelling out to certbot | Phase 7 |
| **mTLS backend re-encryption** | Cluster-internal CA vs per-service certs | Phase 7 |
| **IPv6 per-pod allocation** | Sequential vs SLAAC vs from /64 prefix | Later |
| **PublicIP CRD lifecycle** | How does allocation + attachment + detachment work for IPv4/IPv6 | Later |
