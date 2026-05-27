I agree with the **single engine** idea, but I would **not** go all the way to “pure L3, no bridge” as stated. The best design is a **routing-first engine with nftables as the policy/NAT dataplane**, while keeping the pod attachment model simple and Linux-native. [wiki.nftables](https://wiki.nftables.org/wiki-nftables/index.php/Load_balancing)

## Core view

Using one engine for pod reachability, Service NAT, subnet filtering, and NetworkPolicy is a strong direction because nftables can handle filtering, sets, DNAT/SNAT, and atomic rule updates in one place. That aligns well with your need to support Kubernetes-style NetworkPolicy manifests and Azure-like VNet/subnet/NSG semantics in the same control plane. [techcommunity.microsoft](https://techcommunity.microsoft.com/discussions/azurearchitecture/best-practices-for-designing-a-hub-and-spoke-architecture-in-azure/4383087)

Where I disagree is the phrase “the bridge provides zero value.” A bridge still gives you a very simple pod attachment domain and neighbor handling, while nftables can enforce L3/L4 policy on top of it; bridge and nftables are not mutually exclusive. Also, Kubernetes NetworkPolicy semantics are L3/L4 allow rules tied to pod/namespace selectors, so your policy engine still needs selector resolution regardless of whether packets traverse a bridge or a routed host path. [github](https://github.com/RHsyseng/bridge-filtering-cni/)

## Better architecture

The strongest production design here is:

- **Pod attachment**: veth per pod.
- **Forwarding model**: routed dataplane, not L2-open east-west by default.
- **Policy/NAT engine**: nftables/rustables for forward/input/output, DNAT/SNAT, sets, maps, and atomic batches.
- **Control abstractions**:
  - Kubernetes `NetworkPolicy` using k8s-openapi structs.
  - Azure-style `VNet`, `Subnet`, `NSG`, `RouteTable`, `LoadBalancer`, `TrafficManager`, `APIGateway` as higher-level CRDs that compile down to nftables rules plus service/controller state. [roykim](https://roykim.ca/2021/12/23/building-a-hub-and-spoke-with-site-to-site-vpn-part-5-network-security/)

That gives you one enforcement engine without forcing the whole system to imitate Azure literally at the packet layer. Azure constructs are product abstractions; your kernel dataplane should stay Linux-native. [techcommunity.microsoft](https://techcommunity.microsoft.com/discussions/azurearchitecture/best-practices-for-designing-a-hub-and-spoke-architecture-in-azure/4383087)

## Where your proposal is right

Your idea to make **subnets = CIDRs/sets** and **NSGs = filter rules between those sets** is very good. That is exactly the right substrate for VNet/subnet isolation, Hub-and-Spoke routing constraints, and future multi-node route distribution. [roykim](https://roykim.ca/2021/12/23/building-a-hub-and-spoke-with-site-to-site-vpn-part-5-network-security/)

Your idea to move **ClusterIP to nftables DNAT with atomic updates** is also strong. nftables supports NAT and even map-based load balancing, so per-service reconciliation through batched updates is a better long-term direction than keeping a userspace proxy for the common path. [skudonet](https://www.skudonet.com/knowledge-base/nftlb/what-is-nftlb/)

## Where I would change it

### 1. Don’t drop the bridge for the wrong reason

A bridge is not your security boundary; **policy is**. Saying “bridge leaks L2” is true if left unfiltered, but that does not mean the bridge itself is bad. If you want a routed mental model, use one, but choose it because it simplifies your control plane, not because the bridge is inherently incompatible with isolation. [github](https://github.com/RHsyseng/bridge-filtering-cni/)

### 2. Don’t model Kubernetes policy only as subnet rules

Kubernetes NetworkPolicy is selector-based and namespaced, not subnet-based. If you must support the same manifests, then subnet isolation can be a **baseline default**, but it cannot be the final semantics engine. You need named sets for pods selected by namespaceSelector/podSelector and rule compilation from those selectors into nftables sets. [github](https://github.com/kubernetes-sigs/kube-network-policies/issues/31)

### 3. Don’t force Azure abstractions to replace Kubernetes ones

You need both:
- **Kubernetes-native API** for workloads, Services, Ingress, NetworkPolicy.
- **Cloud-style network CRDs** for VNet/Subnet/NSG/Hub-Spoke features.

The right design is a compiler layer where both APIs target the same nftables backend. That avoids inventing one abstraction that fits neither world perfectly. [kubernetes](https://kubernetes.io/docs/concepts/services-networking/network-policies/)

## Best target model

I’d recommend this exact stack:

| Layer | Recommendation |
|---|---|
| Pod connectivity | veth pairs with routed pod IPs |
| Dataplane enforcement | nftables only, via rustables batches |
| Service networking | ClusterIP/NodePort via nftables DNAT/maps, not userspace proxy for steady state |
| NetworkPolicy | Full Kubernetes semantics compiled to nftables sets/rules |
| VNet/Subnet/NSG | Separate CRDs compiled to subnet sets + forward-chain policy |
| Hub-and-Spoke | Route domains + default deny between spokes; hub as explicit transit path |
| Ingress/API Gateway | Separate L7 controller, not nftables; nftables is L3/L4 only |
| Traffic Manager | DNS/control-plane feature, not nftables |

This is the clean split because nftables is excellent at L3/L4 policy and NAT, but it is not your API gateway or global traffic manager. [wiki.nftables](https://wiki.nftables.org/wiki-nftables/index.php/Load_balancing)

## What I’d do now

Phase the system this way:

1. **Unify host-to-pod, pod-to-pod, and SNAT/DNAT under rustables** with a routed pod IP model. [wiki.nftables](https://wiki.nftables.org/wiki-nftables/index.php/Load_balancing)
2. Add **subnet/VNet/NSG CRDs** and compile them to CIDR sets and forward rules. [techcommunity.microsoft](https://techcommunity.microsoft.com/discussions/azurearchitecture/best-practices-for-designing-a-hub-and-spoke-architecture-in-azure/4383087)
3. Implement **full Kubernetes NetworkPolicy** with selector resolution to nftables sets, keeping k8s-openapi manifests intact. [github](https://github.com/kubernetes-sigs/kube-network-policies/issues/31)
4. Move **ClusterIP** from loopback/userspace proxy to nftables NAT/maps. [wiki.nftables](https://wiki.nftables.org/wiki-nftables/index.php/Load_balancing)
5. Keep **Ingress / API Gateway / Traffic Manager** as higher-layer controllers that program nftables only where L4 exposure is needed. [techcommunity.microsoft](https://techcommunity.microsoft.com/discussions/azurearchitecture/best-practices-for-designing-a-hub-and-spoke-architecture-in-azure/4383087)

So: **yes to one single rustables-backed network engine, yes to routed L3-first design, but no to treating subnet rules as a replacement for full NetworkPolicy, and no to pushing every cloud concept into nftables itself**. The better idea is a **single nftables dataplane with multiple control-plane APIs compiled into it**.