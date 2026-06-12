# Network Module — Remaining Gaps & TODO

## Status: 144 tests passing (85 unit + 59 integration), clippy clean

## Completed Items

- [x] A1: Service CIDR local route (RTN_LOCAL) — `rtnetlink.rs:add_local_service_cidr()`
- [x] A2: Kubernetes API DNS record — `plan.rs:plan_dns()` adds `kubernetes.default.svc.cluster.local`
- [x] A3: NSG default deny trailing rule — `plan.rs:plan_nsgs()` appends drop rule
- [x] A4: Conntrack module check — `rtnetlink.rs:conntrack_available()`
- [x] B3: ARP announce hardening — `lib.rs:enable_arp_announce()`
- [x] B4: Loopback bring-up — `rtnetlink.rs:ensure_loopback_up()`
- [x] B5: Multi-op batch sending — `syscalls.rs:NlSocket::send_batch()`
- [x] B7: Named port resolution — `target_port` is already `Option<u16>` in core types
- [x] B8: RouteTable resource application — `plan.rs:plan_route_tables()`
- [x] B9: Subnet resource registration — `plan.rs:plan_subnets()`
- [x] M6: IpPool::expand() — `ipam.rs:IpPool::expand()`
- [x] M7: ReconcileReport — `engine.rs:ReconcileReport::from_ops()`
- [x] M9: configure_pod_netns() standalone — `rtnetlink.rs:RouteSocket::configure_pod_netns()`
- [x] M10: list_veth_interfaces() public — `rtnetlink.rs:list_veth_interfaces()`

---

## A. CRITICAL — Cluster networking will not work without these

### A1. Service CIDR local route (RTN_LOCAL)
**Old code:** `src2/netmux/applier/netlink/route.rs:add_local_service_cidr()` + `mod.rs:init_nft()`
**Impact:** ClusterIP addresses (e.g. `10.96.0.10`) are NOT locally routable on this node.
Packets destined for ClusterIPs will be dropped by the kernel before DNAT can process them.
**Fix:** Install a `RTN_LOCAL` route for the service CIDR on loopback during network init.
The route must be added via RTNETLINK with `rt_type = RTN_LOCAL` (type=2) and `scope = RT_SCOPE_HOST`.

### A2. Kubernetes API DNS record
**Old code:** `src2/netmux/planner.rs:plan_dns_in_cluster_api()`
**Impact:** `kubernetes.default.svc.cluster.local` does not resolve. In-cluster API access broken.
**Fix:** Add `kubernetes.default.svc.cluster.local → <api_server_ip>` and
`kubernetes.default.svc → <api_server_ip>` DNS records in `plan_dns()`.
The API server IP is typically the first IP in the service CIDR or a configured value.

### A3. NSG default deny trailing rule
**Old code:** `src2/netmux/mod.rs:apply_nsg_rules()` adds `0.0.0.0/0 → 0.0.0.0/0` drop at end.
**New code:** `plan.rs:plan_nsgs()` creates nsg-rules chain but does NOT add a trailing
default-deny drop rule. Without it, traffic that doesn't match any NSG rule will be accepted
(by chain policy Accept), defeating whitelist semantics.
**Fix:** After adding all NSG rules to the nsg-rules chain, append a final
`NftRule::drop().with_comment("nsg-default-deny")` rule.

### A4. Established/related rule — verify conntrack module is loaded
**Old code:** Uses `ct state` which requires the `nf_conntrack` kernel module.
**New code:** Has the rule but no module-load check.
**Fix:** Either modprobe `nf_conntrack` before applying rules, or add a fallback that
skips the ct rule if conntrack is unavailable.

---

## B. HIGH — Important features missing

### B1. DNS improvements
**Old code:** `src2/netmux/dns.rs` (full-featured DNS server)
**New code:** `network/src/dns.rs` (minimal)

| Gap | Old behavior | New behavior | Fix |
|-----|-------------|-------------|-----|
| CNAME records | `cname_record()` for ExternalName services | Only A records | Add CNAME wire encoding + ExternalName handling |
| AAAA/ANY queries | Returns empty NOERROR for AAAA, handles ANY | Falls through to upstream or NXDOMAIN | Handle TYPE_AAAA (28) and TYPE_ANY (255) explicitly |
| DNS compression pointers | `parse_name()` handles `0xC0..` pointer bytes | Rejects compression | Add pointer parsing in `parse_query()` |
| Upstream auto-detection | Reads `/etc/resolv.conf` for nameservers | Requires explicit config | Add resolv.conf parser |
| Default upstream fallback | Falls back to 1.1.1.1, 8.8.8.8 | Returns NXDOMAIN | Add fallback list |
| Multi-format name parsing | Handles 5 formats: full FQDN, short, bare, svc.ns, svc.ns.svc | Exact zone lookup only | Add `parse_service_name()` with format matching |
| Configurable port with fallback | Tries configured port, then 53, then 5353 | Single bind address | Add fallback binding logic |

### B2. Ingress host DNS records
**Old code:** `src2/netmux/planner.rs:plan_dns_from_ingress()`
**Impact:** Ingress hostnames don't resolve to the gateway IP.
**Fix:** Process `Ingress` resources from the snapshot, extract host→backend mappings,
and add DNS records mapping hostnames to the node's gateway IP.

### B3. ARP announce hardening
**Old code:** `src2/netmux/applier/netlink/sysctl.rs:harden_sysctl()` sets `arp_announce=2`
**New code:** Only sets `rp_filter=1`
**Fix:** Add to `harden_sysctl()`:
```rust
for param in &["net/ipv4/conf/all/arp_announce", "net/ipv4/conf/default/arp_announce"] {
    let path = format!("/proc/sys/{}", param);
    std::fs::write(&path, "2\n").ok();
}
```

### B4. Loopback bring-up
**Old code:** `src2/netmux/applier/netlink/sysctl.rs:ensure_loopback_up()` uses
`SIOCGIFFLAGS`/`SIOCSIFFLAGS` ioctls to bring up the `lo` interface.
**New code:** No equivalent.
**Fix:** Add `ensure_loopback_up()` using rustix ioctl or the existing rtnetlink
`set_link_up` (ifindex=1 for loopback).

### B5. Multi-op batch sending
**Current:** `NlSocket::send()` sends one op per batch (BEGIN + op + END).
**Old:** Rustables `Batch` accumulates many ops then sends in one `sendmsg`.
**Impact:** Performance — reconcile ticks with many ops send many small batches.
**Fix:** Add `NlSocket::send_batch(ops: &[NetlinkOp])` that builds a single batch
buffer with one BEGIN, all ops, one END, then sends once.

### B6. Error suppression for non-fatal netlink errors
**Old code:** `NftEngine::send()` silently ignores ENOENT (2), EBUSY (16), EEXIST (17).
**New code:** All errors propagate.
**Impact:** Cleanup of non-existent chains during reconcile returns errors.
**Fix:** In `Netmux::apply()`, when `dry_run=false`, check for errno 2/16/17 and
continue instead of failing.

### B7. Named port resolution (IntOrString)
**Old code:** `src2/netmux/sync_network.rs:resolve_named_port()` resolves `"http"` → `8080`
by looking up pod container port specs.
**New code:** `target_port` is treated as raw `u16` in the planner.
**Fix:** Add a port name→number resolution step in `plan_services()` that looks up
the pod's container port specs when `target_port` is a string.

### B8. RouteTable resource application
**Old code:** `src2/netmux/sync_network.rs:apply_route_tables()` applies RouteTable resources.
**New code:** Not processed by the planner.
**Fix:** Add `plan_route_tables()` in `plan.rs` that reads RouteTable resources from
the snapshot and adds corresponding entries to `state.routes`.

### B9. Subnet resource registration
**Old code:** `src2/netmux/sync_network.rs:apply_subnets()` registers named subnet pools.
**New code:** Only creates a single "pods" pool.
**Fix:** Add `plan_subnets()` that reads Subnet resources and registers them as
named pools in `state.ip_pools`.

---

## MEDIUM — Nice to have, partial functionality acceptable

### M1. NetworkPolicy controller (runtime set updates)
**Old code:** `src2/netmux/np_controller.rs` has `update_pod()`/`remove_pod()` that
dynamically add/remove IPs from nftables sets when pods change.
**New code:** Sets are created statically from the snapshot. Pod changes require a
full reconcile tick to update sets.
**Impact:** Slight delay in policy enforcement after pod changes (one reconcile tick).
**Fix for full parity:** Add set element add/del ops to `NetlinkOp` and have the
engine update sets incrementally. OR accept the reconcile-tick delay.

### M2. Namespace selector in NetworkPolicy
**Old code:** `np_controller.rs::apply_network_policy()` handles `namespaceSelector`.
**New code:** Only handles `podSelector`.
**Fix:** Add namespace selector matching by creating namespace-to-label sets and
using lookup expressions against them.

### M3. IP block with except in NetworkPolicy
**Old code:** Handles `ipBlock` with `except` CIDRs (allow range, deny sub-ranges).
**New code:** No ipBlock support.
**Fix:** Add ipBlock handling in the planner that creates CIDR-based allow/deny rules.

### M4. matchExpressions operators
**Old code:** Supports In, NotIn, Exists, DoesNotExist.
**New code:** Only matchLabels (exact key-value).
**Fix:** Add matchExpression evaluation to `resolve_backends()` in plan.rs.

### M5. L7 HTTP Ingress / Reverse Proxy
**Old code:** `src2/netmux/ingress.rs` — TCP listener on port 80, HTTP Host header
extraction, host-based routing, TCP bidirectional proxy.
**New code:** Entirely absent.
**Note:** This is a significant subsystem. May be better implemented as a separate
crate or in the controller. Mark as not-in-scope for the network crate.

### M6. IpPool::expand()
**Old code:** `pool.rs::expand()` adds an adjacent CIDR range to an existing pool.
**New code:** Not present.
**Fix:** Add `expand(&mut self, additional: Ipv4Cidr)` that checks adjacency and
merges into the free list.

### M7. ReconcileReport
**Old code:** `reconciler.rs::ReconcileReport` has per-resource-type counts.
**New code:** Returns total op count only.
**Fix:** Add a `ReconcileReport` struct to `engine.rs::reconcile()` that counts
ops by type (dnat, nsg, route, set, etc.).

### M8. rollback_veth()
**Old code:** `mod.rs:rollback_veth()` cleans up partially-created veth on failure.
**New code:** Error propagation leaves partial state.
**Fix:** In `RouteSocket::attach_pod()`, catch errors after partial creation and
clean up any created resources before returning the error.

### M9. configure_pod_netns() (standalone)
**Old code:** `mod.rs:configure_pod_netns()` — standalone method to enter a pod's
netns, assign IP, bring up, add default route.
**New code:** Bundled into `attach_pod()`. No standalone reconfiguration.
**Fix:** Extract the netns configuration portion of `attach_pod()` into a public
`configure_pod_netns(pid, pod_ip, gateway)` method.

### M10. list_veth_interfaces() (public)
**Old code:** `mod.rs:list_veth_interfaces()` enumerates all veth-* from /sys/class/net.
**New code:** `clean_orphan_veths()` does this inline but privately.
**Fix:** Extract the enumeration into a public function.

---

## LOW — Minor improvements

### L1. DNS configurable port with fallback
**Old:** Tries configured port, then 53, then 5353.
**New:** Single bind address.
**Fix:** Add fallback binding loop in `DnsServer::serve()`.

### L2. APPLY_ORDER documentation
**Old:** `reconciler.rs:APPLY_ORDER` constant documents execution order.
**New:** Implicit in code.
**Fix:** Add a comment or constant documenting the intended order.

### L3. Service deletion path
**Old:** `sync_network.rs:remove_service()` explicitly cleans up DNAT rules.
**New:** Declarative model handles it via reconcile diff.
**Impact:** None (declarative is actually better), but no explicit API for it.
**Fix:** Optionally add `Netmux::remove_service(name, namespace)` as a convenience.

### L4. sync_service() / remove_service() methods
**Old:** `NetworkEngine` trait has per-service sync/removal methods.
**New:** `NetworkEngine` trait only has reconcile/current/seed.
**Fix:** Optionally add these as convenience methods on the trait.

### L5. PodResolver trait
**Old:** `network.rs` has `PodResolver` with `is_pod_alive()` and `backend_connect_port()`.
**New:** Not present.
**Fix:** Implement if the controller needs pod liveness checks.

### L6. compute_endpoints() / compute_endpointslices()
**Old:** `NetworkEngine` trait computes Endpoints/EndpointSlices.
**New:** Not present.
**Fix:** Implement in the controller or API layer, not in the network crate.

---

## C. Implementation Order (recommended)

```
Priority 1 (do first):
  A1  Service CIDR local route          — without this, DNAT won't work at all
  A2  Kubernetes API DNS record         — in-cluster API access is broken
  A3  NSG default deny trailing rule    — whitelist semantics are broken
  B1  DNS AAAA/compression/resolv.conf  — DNS is partially broken
  B5  Multi-op batch sending            — performance improvement
  B6  Error suppression                 — cleanup failures cascade

Priority 2 (do next):
  A4  Conntrack module check            — verify ct state works
  B2  Ingress host DNS records          — ingress DNS broken
  B3  ARP announce hardening            — security gap
  B4  Loopback bring-up                 — may fail in fresh netns
  B7  Named port resolution             — services with named ports broken
  B8  RouteTable resource application   — custom routes ignored
  B9  Subnet resource registration      — custom subnets ignored

Priority 3 (when needed):
  M1-M10  Medium priority items
  L1-L6   Low priority items
```

---

## D. Files to modify

| File | Changes needed |
|------|---------------|
| `network/src/rtnetlink.rs` | Add `add_local_service_cidr()`, `ensure_loopback_up()`, `list_veth_interfaces()` |
| `network/src/plan.rs` | Add service CIDR route, kubernetes API DNS, ingress DNS, RouteTable/Subnet processing, named port resolution, default deny rule |
| `network/src/dns.rs` | Add CNAME, AAAA, compression pointers, resolv.conf parsing, fallback upstreams, multi-format name parsing |
| `network/src/lib.rs` | Add `arp_announce`, `ensure_loopback_up()` |
| `network/src/engine.rs` | Add error suppression, `ReconcileReport`, `send_batch()` |
| `network/src/ipam.rs` | Add `IpPool::expand()` |
| `network/src/model.rs` | (no changes needed) |
| `network/src/syscalls.rs` | (no changes needed — wire encoding is correct) |

---

## E. Test coverage gaps

| Scenario | Status | Notes |
|----------|--------|-------|
| Pod-to-pod same node | ✅ Tested | veth + route |
| Pod-to-pod cross node | ✅ Tested | remote route via peer |
| Pod-to-internet (SNAT) | ✅ Tested | masquerade rule |
| Service ClusterIP DNAT | ✅ Tested | DNAT rules + DNS |
| Service NodePort DNAT | ✅ Tested (builder) | nodeport_dnat_rule |
| NSG allow/deny | ✅ Tested | nsg_filter_rule |
| NSG default deny | ⚠️ Rule added | Need test verifying trailing drop |
| VNet isolation | ✅ Tested | no-internet drop |
| NetworkPolicy set match | ✅ Tested (builder) | lookup expression |
| Established/related | ✅ Tested | ct + bitwise + cmp |
| Catch-all pod CIDR | ✅ Tested | jump chain |
| Service CIDR route | ❌ Not tested | A1 not implemented |
| Kubernetes API DNS | ❌ Not tested | A2 not implemented |
| DNS AAAA | ❌ Not tested | B1 not implemented |
| DNS compression | ❌ Not tested | B1 not implemented |
| Loopback bring-up | ❌ Not tested | B4 not implemented |
| ARP announce | ❌ Not tested | B3 not implemented |
| Error suppression | ❌ Not tested | B6 not implemented |
| Multi-op batch | ❌ Not tested | B5 not implemented |
| IpPool expand | ❌ Not tested | M6 not implemented |
| Named port resolution | ❌ Not tested | B7 not implemented |
| RouteTable resources | ❌ Not tested | B8 not implemented |
| Subnet resources | ❌ Not tested | B9 not implemented |
| Service deletion cleanup | ❌ Not tested | Declarative handles it |
| Reconcile idempotency | ✅ Tested | seed + reconcile |
| Full lifecycle empty | ✅ Tested | plan + reconcile |
| Full lifecycle with svc | ✅ Tested | plan + reconcile |
| Full lifecycle with pods | ✅ Tested | plan + reconcile |
| Full lifecycle with NSG | ✅ Tested | plan + reconcile |
