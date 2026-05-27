can a deployment has pods on multi-node ?
if the pod crashed a new pod will be created, will it get new ip or use the old one, and what if a pod keeps crashing will be reach end of ips for its namespace?
does pods of same deployment can see each others and do they in same ip range, even if they distrubited around multi-node?
can our reverse proxy work for private vnets / subnets so we can have ingress by domain but accepts only from another subnet / vent so we can have truly hub and spoke ? so we might have a subdomain for the db service liiving in A1 subnet and another service lives in subnet A2 and a home service lives in A3 subnet and home service is the hub and others are spoke and home service want to access db on db.myapp and we need to route this to the db on A1 subnet. and this route is only supoorted to be handled from the hub to spoke and not verse versa. so we have our reverse dns can even work for private nets / subnets which doesnt face internet and maybe the hub only has internet access. (does my question right?) 
can our plan support a node to host apps and another node has the db and so on, so the requests get routed fine and could be streaming, high throuput data.
I think Node Discovery is required.
I beleive phase 0 should be creating tests (senario scripts with yaml content) so we track our progress and achivements.
can we avoid Libc FFI? does rustables can cover us?
since NSG conflict with NetworkPolicy and More specific rule wins by evaluation order , i would like if we start by only supporting NSG and make it cover same NetworkPolicy stuff. but i want NSG to handle IPs, subnets, domain and subdomains, and wildcard domains, ports, protocols, intranet, internet, custom headers in http/s requests, method type (get,post,..) like a firewall. 
Does this plan supports tls like https or it can be restricted to http?
I have some servers which has IPv6 /64 IPs. its own IP list, with a single IPv4. In our plan we should support assigning an IPv6 or IPv4 to resource (ingress) or to a node or what is the best here? imagine azure when it assign public ip for resource like loadbalancer of so on, we need to be able to do so exactly. 

1. Can a deployment have pods on multi-node?
Yes. The deployment controller creates pods across nodes. Each pod gets an IP from its node's /24 (e.g., Node A → 10.42.1.X, Node B → 10.42.2.X). They're all routable within the /16. The deployment doesn't care which /24 a pod lands in. The question is really about the scheduler — do we distribute replicas round-robin, or do we have a smarter scheduling strategy (e.g., anti-affinity)?
2. Crash-loop pod: new IP or old? What about exhaustion?
New IP from the free-list. The crashed pod's IP goes back to the pool. Even with rapid crash-looping on a single node, the BTreeSet recycler means 254 IPs per node is very hard to exhaust in practice. The only risk is if you somehow have 254 unique pods in a crash-loop simultaneously, which is implausible. Implementation question: should we add an ipv4 field to ContainerInstance so the DNAT maps just get a batch update on restart? Yes — trivial.
3. Same-deployment pods see each other across nodes?
Yes. They're all in the same /16. Pod A on Node A (10.42.1.5) and Pod B on Node B (10.42.2.3) can reach each other directly via host routing. Whether they should see each other is a NetworkPolicy question, not a routing question.
4. Hub-and-spoke with private DNS + hub-only internet access.
This is the most architecturally significant question. You're describing:
Internet ──► Hub VNet (A3, has SNAT, has ingress)
               │
               │──► Spoke A (A1, DB)  — no internet, hub can reach it
               │──► Spoke B (A2, app) — no internet, hub can reach it
               │
               Internal DNS: db.myapp → 10.42.1.5 (inside A1)
                            app.myapp → 10.42.2.3 (inside A2)
nftables NSG rules:
chain forward {
  # hub → spoke: allowed
  ip saddr @vnet-hub ip daddr @vnet-spoke-a accept
  ip saddr @vnet-hub ip daddr @vnet-spoke-b accept
  # spoke → spoke: denied
  ip saddr @vnet-spoke-a ip daddr @vnet-spoke-b drop
  # spoke → internet: denied (no MASQUERADE on spoke subnets)
  ip saddr @vnet-spoke-a oif eth0 drop
  # hub → internet: allowed (MASQUERADE rule matches hub subnet)
}
Plus SNAT rules only apply to the hub subnet. The embedded DNS resolves db.myapp → ClusterIP or pod IP within the private VNet. The hub's ingress routes to spoke services.
Yes, the plan supports this fully — NSG CIDR rules + selective SNAT + internal DNS. The hub is just a VNet with internet access and the ingress controller.
5. Node A = apps, Node B = DB, streaming/high-throughput fine?
One extra L3 hop. Kernel handles it. For Redis replication, Postgres streaming, etc., this is fine as long as the inter-node link has enough bandwidth. The bottleneck is the physical network, not the routing. If they need zero network overhead, a scheduling anti-affinity rule or an explicit node selector can force them onto the same physical machine if the user wants.
6. Node Discovery — agreed, static list is not enough.
MVP-viable alternatives:
- UDP multicast — each node broadcasts its host IP + pod CIDR on a well-known port. Simple, zero config, but doesn't work across subnets.
- Gossip protocol (e.g., SWIM) — each node maintains a membership list with periodic probes. More complex but production-grade.
- External store (etcd-like) — overkill for MVP.
The --peers flag can seed the initial cluster, and a lightweight SWIM-style gossip can handle dynamic membership after that.
7. Phase 0: tests first.
Agree. Creates the definition of done before implementation. Integration tests should cover each data flow: pod connectivity, ClusterIP DNAT, cross-node, NetworkPolicy allow/deny, NSG deny, hub-and-spoke.
8. Can rustables replace all libc FFI?
No. rustables only covers nfnetlink (nftables subsystem). It cannot create veth pairs (RTM_NEWLINK), add routes (RTM_NEWROUTE), assign IPs (RTM_NEWADDR), or bring interfaces up (ioctl). These are rtnetlink operations — a different netlink subsystem.
We'd need either:
- Keep the ~50 lines of proven libc FFI for RTM_* + ioctl (what we already have)
- Add the rtnetlink crate as another dependency
I recommend keeping the libc FFI — it's small, well-understood, and already written.
9. NSG covering everything: IPs, ports, proto, domains, HTTP methods, headers.
nftables can handle L3/L4 (IPs, ports, protocols) and simple L7 at fixed offsets (e.g., TLS SNI). But variable-length HTTP header matching, wildcard domains, methods, cookies — these require userspace L7 processing. NSG cannot replace a WAF.
My suggestion: NSG covers L3/L4 policy (IPs, subnet, ports, protocol). The L7 ingress controller (axum) adds HTTP-level filtering on top. Together they give you the full firewall semantics you want, with a clean boundary:
NSG (nftables)        → L3/L4: IPs, ports, protocols, SNI, subnet isolation
L7 Ingress (axum)     → L7: HTTP methods, headers, cookies, paths, wildcard domains
Both are enforced, both are policy. NSG = network boundary, Ingress = application boundary.
10. TLS/HTTPS support — just HTTP or full TLS?
Current plan: TLS pass-through (SNI routing, no decryption). You're asking about full TLS termination at the ingress with certificate management. That's doable in the L7 axum layer — OpenSSL is already linked. But it requires:
- Certificate storage (Secret CRD already exists)
- Auto-TLS (Let's Encrypt-like)
- mTLS for backend connections
This is significant scope. For MVP, pass-through is enough — the backend terminates TLS itself. Do you want full termination in Phase 7 or earlier?
11. IPv6 — static /64, assign to resources like Azure public IPs.
Currently the plan is IPv4-only. Your /64 changes the picture — each pod could get a /80 subnet within it, or we could give each pod a single IPv6 address from the /64. The Azure-style "public IP resource" model (allocate an IP, attach it to a load balancer/ingress/NIC) is a good abstraction.
For MVP, I'd suggest:
- Each pod gets both an IPv4 (10.42.X.X) and an IPv6 (from the /64) address
- The ingress binds on both [::]:80 and 0.0.0.0:80
- Public IP CRD: allocate and attach to ingress or load balancer
- The nftables DNAT maps support both address families