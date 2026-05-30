# Hub-and-Spoke Topology Implementation

## Goal
Implement Azure-style hub-and-spoke topology using VNet/Subnet/NSG/RouteTable CRDs. Pods get IPs from their subnet's CIDR pool (not the global pool), and NSG rules enforce unidirectional hub→spoke traffic with spoke internet isolation.

## Architecture

```
VNet: test-vnet (10.200.0.0/16)
├── Hub-sub:    10.200.0.0/24  — internet_access=true
├── Spoke-1:    10.200.1.0/24  — spoke role, no internet
└── Spoke-2:    10.200.2.0/24  — spoke role, no internet

Hub deployment  (app=hub)     :8080 → Python proxy returning Hub(Spoke1,Spoke2)
Spoke1 deployment (app=spoke1):8080 → "Spoke1"
Spoke2 deployment (app=spoke2):8080 → "Spoke2"

svc-hub     NodePort 30005  → hub:8080
svc-spoke1  ClusterIP       → spoke1:8080
svc-spoke2  ClusterIP       → spoke2:8080

Ingress: hub1.local.cluster → svc-hub:80
```

## CRD Structure

| CRD | Kind | Fields | API |
|---|---|---|---|
| VNet | `VNet` | cidr, internet_access, role | `z8s.io/v1` |
| Subnet | `Subnet` | vnet, cidr | `z8s.io/v1` |
| NSG | `NSG` | target_vnets, rules[name, action, src_cidrs, dst_cidrs, ports, protocol] | `z8s.io/v1` |
| RouteTable | `RouteTable` | rules[name, methods[], paths[], headers[], action] | `z8s.io/v1` |

## NSG Rules (L3/L4)

Directionality enforced via forward chain:
```
allow hub-sub → spoke-1-sub : tcp/80  (hub reaches spoke1)
allow hub-sub → spoke-2-sub : tcp/80  (hub reaches spoke2)
allow hub-sub → 0.0.0.0/0   : tcp/*  (hub internet access)
deny  * → *                  : */*    (default — enforced by chain policy drop)
```

Spoke→hub is blocked by default (no allow rule). Spoke internet is blocked by default.

## RouteTable Rules (L7)

Hub RouteTable enforces HTTP method filtering:
```
allow GET /
deny  POST, PUT, DELETE, PATCH
```

## Forward Chain Order

```
1. ct state established/related accept
2. jump catch-all  (host→pod for NodePort — ip daddr pod_cidr accept)
3. NSG allow rules (added by VNet controller via apply_nsg)
4. Default: drop
```

No blanket `ip saddr pod_cidr accept` — all pod→any must be explicitly allowed via NSG.

## Per-Subnet IP Allocation

When a Subnet CRD is applied, its CIDR is registered as an `IpPool` in `NetMux::subnet_pools`. Pods with `z8s.io/subnet: <name>` annotation allocate from that pool instead of the global pool.

```
spec_builder → reads z8s.io/subnet from pod annotations
           ↓
ContainerSpec.subnet → ContainerSpawnCtx.subnet
           ↓
runtime.rs → netmux.attach_pod(pod_uid, pid, subnet_name)
           ↓
NetMux → subnet_pools[subnet_name].allocate()  → 10.200.1.2
```

## Test Scenarios (`tests/netmux/test_hub_spoke.sh`)

| # | Test | Expected |
|---|---|---|
| 1 | Hub → svc-spoke1 | "Spoke1" |
| 2 | Hub → svc-spoke2 | "Spoke2" |
| 3 | Hub → internet | Connected (NSG allow rule) |
| 4 | Spoke1 → hub | Timeout (no NSG allow) |
| 5 | Spoke2 → hub | Timeout (no NSG allow) |
| 6 | Spoke1 → internet | Timeout (default drop) |
| 7 | Spoke2 → internet | Timeout (default drop) |
| 8 | NodePort 30005 | "Hub(Spoke1,Spoke2)" |
| 9 | Ingress hub1.local.cluster | "Hub(Spoke1,Spoke2)" |

## Files Changed

### Components
- `src/components/network/vnet.rs` — VNetResource (dedicated, moved from CrdWatcher)
- `src/components/network/subnet.rs` — SubnetResource (registers subnet CIDR pool)
- `src/components/network/nsg.rs` — NsgResource (dedicated, moved from CrdWatcher)
- `src/components/network/routetable.rs` — RouteTableResource (L7 HTTP filtering rules)

### API Handlers
- `src/api/handlers/vnet.rs` — VNet CRUD
- `src/api/handlers/subnet.rs` — Subnet CRUD
- `src/api/handlers/nsg.rs` — NSG CRUD
- `src/api/handlers/routetable.rs` — RouteTable CRUD
- `src/api/handlers/ingress.rs` — Ingress CRUD (split from networking.rs)
- `src/api/handlers/networkpolicy.rs` — NetworkPolicy CRUD (split from networking.rs)

### Core Engine
- `src/netmux/mod.rs` — Added subnet_pools, register_subnet_cidr, subnet-aware attach_pod/release_ip
- `src/netmux/nftables.rs` — Forward chain cleanup (removed broad accept rules), catch-all sub-chain, nodeport_jump_track
- `src/netmux/netlink.rs` — Default route RTA_DST fix, RTNH_F_ONLINK, errno reporting

### Container Runtime
- `src/cri/spec.rs` — Added `subnet: Option<String>` to ContainerSpec
- `src/cri/runtime.rs` — Threaded subnet through spawn_container → attach_pod
- `src/components/compute/spec_builder.rs` — Reads `z8s.io/subnet` annotation

### CRD Types
- `src/netmux/crds.rs` — RouteTable changed from L3 routes (destination/next_hop) to L7 rules (methods/paths/headers)

### Integration
- `src/main.rs` — Component/handler registration
- `src/api/handlers/mod.rs`, `src/api/handlers/system.rs` — Module and API group registration
- `src/components/network/mod.rs` — Module registration
- `src/api/server.rs` — Route registration

## Remaining Work

- NSG rule accumulation: `add_forward_allow`/`add_forward_deny` append rules without cleaning old ones
- `expect()` replacements in non-netmux code (tests, other modules)
- DNS F2 fix: pod resolv.conf still shows 127.0.0.1 for non-isolated pods
