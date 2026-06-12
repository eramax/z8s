# Network Module Feature Comparison: z8s vs pelagos

## Legend

| Symbol | Meaning |
|--------|---------|
| ✅ | Implemented, tested against real kernel |
| 🔶 | Implemented, wire-format verified only |
| 🔸 | Implemented, unit tested only |
| ❌ | Not implemented |
| 🗑️ | Removed / deferred (see notes) |

---

## 1. nftables Wire Encoding (nftables netlink)

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| Batch envelope (BEGIN/END) | ✅ | ✅ | |
| Table create/delete | ✅ | ✅ | |
| Chain create (base with hook+policy) | ✅ | ✅ | |
| Chain create (regular, no hook) | ✅ | ✅ | |
| Chain delete | ✅ | ✅ | |
| Chain flush (via DELRULE) | ✅ | ✅ | pelagos only |
| Rule add | ✅ | ✅ | |
| Rule delete (by handle) | ✅ | ✅ | |
| Rule dump (GETRULE) | ❌ | ✅ | pelagos only |
| Set create | ✅ | 🔸 | z8s fully tested |
| Set element add (NEWSETELEM) | ✅ | ❌ | z8s only |
| Set delete | ✅ | ❌ | z8s only |
| Set flush | ✅ | ❌ | z8s only |
| Counter object | ✅ | ❌ | z8s only |
| Expression: payload | ✅ | ✅ | |
| Expression: cmp (EQ/NEQ) | ✅ | ✅ | |
| Expression: meta (iifname/oifname/l4proto) | ✅ | ✅ | |
| Expression: bitwise (CIDR mask) | ✅ | ✅ | |
| Expression: immediate (verdict + data) | ✅ | ✅ | |
| Expression: verdict (accept/drop/jump) | ✅ | ✅ | |
| Expression: NAT (DNAT) | ✅ | ✅ | |
| Expression: masquerade | ✅ | ✅ | |
| Expression: numgen (LB) | ✅ | ❌ | z8s only |
| Expression: conntrack (ct state) | ✅ | ❌ | z8s only |
| Expression: goto | 🔶 | ❌ | z8s encoded, never used |
| Expression: return | 🔶 | ❌ | z8s encoded, never used |
| NFTA_RULE_USERDATA (comments) | ✅ | ❌ | z8s uses libnftnl udata TLV format |
| Response parsing (jump handle detection) | ❌ | ✅ | pelagos only |

## 2. RTNETLINK (Linux kernel networking)

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| Socket open (NETLINK_ROUTE) | ✅ | ✅ | |
| Veth pair creation | ✅ | ✅ | z8s via iproute2, pelagos raw netlink |
| Link bring up (IFF_UP) | ✅ | ✅ | |
| Link delete | ✅ | ✅ | |
| Link move to netns | 🔸 | ✅ | pelagos fully tested |
| Address add (IPv4) | ✅ | ✅ | |
| Address add (IPv6) | 🔸 | ✅ | pelagos fully tested |
| Route add (IPv4 unicast) | ✅ | ✅ | |
| Route add (default IPv4) | ✅ | ✅ | |
| Route add (default IPv6) | ❌ | ✅ | pelagos only |
| Route add (RTN_LOCAL for service CIDR) | ✅ | ❌ | z8s only |
| Route delete | ✅ | ❌ | z8s only |
| Bridge creation (ioctl) | ❌ | ✅ | pelagos only |
| Bridge link attach (ioctl) | ❌ | ✅ | pelagos only |
| Neighbor add (IPv6 NDP) | ❌ | ✅ | pelagos only |
| Netns create (unshare + bind-mount) | 🔸 | ✅ | pelagos fully tested |
| Netns delete (umount + unlink) | 🔸 | ✅ | pelagos fully tested |
| Netns run closure (setns) | 🔸 | ✅ | pelagos fully tested |
| sysctl: ip_forward | ✅ | ❌ | z8s only |
| sysctl: rp_filter | 🔸 | ❌ | z8s only |
| sysctl: arp_announce | 🔸 | ❌ | z8s only |

## 3. Container Orchestration

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| VNet (virtual network) create/teardown | ✅ | ✅ | z8s = nftables isolation; pelagos = bridge+veth+NAT |
| IPAM (IPv4 pool allocate/release) | ✅ | ✅ | z8s = in-memory; pelagos = file-locked |
| IPAM (IPv6 pool) | 🔸 | ✅ | pelagos fully tested |
| Pod attach (veth + IP + route + netns) | 🔸 | ✅ | pelagos fully tested |
| Pod detach (veth + route cleanup) | 🔸 | ✅ | pelagos fully tested |
| Pod netns configuration | 🔸 | ✅ | pelagos fully tested |
| Service CIDR on loopback (DNAT target) | ✅ | ❌ | z8s only |
| Orphan veth cleanup | 🔸 | ✅ | pelagos fully tested |
| Bridge networking | ❌ | ✅ | pelagos only |
| Secondary network attachment | ❌ | ✅ | pelagos only |
| Pasta user-mode networking | ❌ | ✅ | pelagos only |
| Port mapping / DNAT | ❌ | ✅ | pelagos only |
| Userspace TCP proxy (tokio async) | ❌ | ✅ | pelagos only |
| Userspace UDP proxy (std threads) | ❌ | ✅ | pelagos only |
| Network create/list/remove/inspect (CLI) | ❌ | ✅ | pelagos only |
| File-locked NAT refcounting | ❌ | ✅ | pelagos only |
| Crash-safe stale entry eviction | ❌ | ✅ | pelagos only |

## 4. DNS

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| Built-in DNS server (UDP) | ✅ | ✅ | z8s = in-process; pelagos = separate daemon |
| DNS query parsing (RFC 1035) | ✅ | ✅ | |
| DNS response building (A record) | ✅ | ✅ | |
| NXDOMAIN for unknown names | ✅ | ✅ | |
| DNS record zone (in-memory) | ✅ | 🔸 | |
| DNS record live update | 🔸 | ✅ | pelagos fully tested |
| dnsmasq backend | ❌ | ✅ | pelagos only |
| Upstream forwarding | 🔸 | ❌ | z8s implemented, not tested |
| Per-network DNS config files | ❌ | ✅ | pelagos only |
| DNS firewall rules (INPUT chain) | ❌ | ✅ | pelagos only |

## 5. Reconciliation / State Management

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| Declarative desired-state model | ✅ | ❌ | z8s only (Kubernetes-style) |
| Pure reconcile function (diff engine) | ✅ | ❌ | z8s only |
| Minimal op generation (add/remove only) | ✅ | ❌ | z8s only |
| Idempotent reconcile (same state = zero ops) | ✅ | ❌ | z8s only |
| Chain shape-change detection (recreate) | ✅ | ❌ | z8s only |
| Rule handle tracking | ✅ | ❌ | z8s only |
| Set wholesale replace | ✅ | ❌ | z8s only |
| Counter reconciliation | ✅ | ❌ | z8s only |
| Route reconciliation | ✅ | ❌ | z8s only |
| ReconcileReport (per-resource op counts) | ✅ | ❌ | z8s only |
| Generation counter | ✅ | ❌ | z8s only |
| NetworkEngine async trait | ✅ | ❌ | z8s only |
| Imperative (apply each op directly) | ❌ | ✅ | pelagos only |

## 6. Kubernetes-Specific Features

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| VNet isolation (drop forward unless internet) | ✅ | ❌ | z8s only |
| ClusterIP DNAT (per-service) | ✅ | ❌ | z8s only |
| NodePort DNAT | 🔸 | ❌ | z8s only (plan tests) |
| Load balancing (numgen per-backend) | ✅ | ❌ | z8s only |
| NSG (Network Security Group) rules | ✅ | ❌ | z8s only |
| NSG default-deny trailing rule | ✅ | ❌ | z8s only |
| Established/related stateful firewall | ✅ | ❌ | z8s only |
| Pod CIDR masquerade | ✅ | ❌ | z8s only |
| Catch-all pod-to-pod accept | ✅ | ❌ | z8s only |
| Remote pod route (via peer gateway) | ✅ | ❌ | z8s only |
| Service DNS records (FQDN) | ✅ | ❌ | z8s only |
| Kubernetes API DNS record | ✅ | ❌ | z8s only |
| Plan from StoreSnapshot | ✅ | ❌ | z8s only |
| Backend resolution (label selectors) | ✅ | ❌ | z8s only |

## 7. iptables-nft Compat

| Feature | z8s | pelagos | Notes |
|---------|-----|---------|-------|
| iptables-nft FORWARD chain compat | ❌ | ✅ | pelagos only |
| iptables-nft INPUT chain compat | ❌ | ✅ | pelagos only |

---

## Test Coverage Summary

| Test Suite | z8s | pelagos |
|------------|-----|---------|
| Unit tests (no kernel) | 85 | ~40 |
| Wire-format tests (encode/decode) | 59 | ~8 |
| Kernel integration tests (real nftables) | 18 | 0 direct |
| Kernel apply test (full lifecycle) | 1 | manual/compose |
| Pod deploy tests (veth + netns) | 2 | 1 (netns only) |
| **Total** | **165** | **~50** |

---

## Removed / Deferred Features

| Feature | Status | Why |
|---------|--------|-----|
| `NftExpr::Goto` | 🔶 Encoded, unused | No rule builder uses goto yet |
| `NftExpr::Return` | 🔶 Encoded, unused | No rule builder uses return yet |
| `NftFamily::Ip6/Inet/Netdev` | 🔶 Encoded, untested | All kernel tests use IPv4 only |
| `SNAT` (nat_type=0) | ❌ Not tested | Only DNAT (nat_type=1) tested |
| `NftChainKind::Route` | 🔸 Defined, unused | Never used in any plan or rule |

---

## Feature Gap Analysis

### What z8s has that pelagos doesn't:
- **Full declarative reconciliation** (pure diff engine, idempotent, handle-tracking)
- **Kubernetes-specific networking** (ClusterIP, NodePort, NSG, VNet isolation, numgen LB)
- **Stateful firewall** (established/related conntrack matching)
- **Counter objects** and reconciliation
- **Route reconciliation** (local service CIDR, remote pod routes)
- **DNS server** (in-process, no external daemon)
- **sysctl hardening** (ip_forward, rp_filter, arp_announce)
- **More expression types** (numgen, conntrack)

### What pelagos has that z8s doesn't:
- **Container networking orchestration** (bridge, veth, port mapping, proxies)
- **IPv6 full stack** (dual-stack, NDP, default routes)
- **Pasta user-mode networking**
- **Userspace TCP/UDP proxies** (tokio async + std threads)
- **dnsmasq backend** for DNS
- **Bridge creation** (ioctl)
- **Network CLI** (create/list/remove/inspect)
- **File-locked IPAM** (crash-safe, multi-process)
- **iptables-nft compat chains**
- **Rule dump/parsing** (GETRULE with NLM_F_DUMP)
- **Chain flush** (delete all rules from chain)

### Neither implements:
- Full IPv6 nftables rules (only pelagos has IPv6 routes/addresses)
