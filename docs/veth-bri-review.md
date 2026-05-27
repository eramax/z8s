This is an excellent and well-structured networking plan. Here is a deep technical review with identified improvements and what's already solid.

***

## Overall Assessment

The plan is **well-designed and production-grade in architecture**. The phased migration strategy, rollback path, and focus on raw netlink (no new crates) are all strong choices. However, there are several gaps worth addressing. [ppl-ai-file-upload.s3.amazonaws]

***

## What's Excellent ✅

- **Zero-copy splice path** — using `copy_bidirectional` with the kernel's `splice(2)` for ingress is the right call; it matches what Nginx/Traefik do internally [ppl-ai-file-upload.s3.amazonaws]
- **Phased rollback** — keeping `--net-backend=setns` as a fallback throughout migration is solid defensive engineering [ppl-ai-file-upload.s3.amazonaws]
- **Raw netlink FFI** — staying consistent with `ensure_loopback_alias` without pulling in `rtnetlink` crate is the right constraint to honor [ppl-ai-file-upload.s3.amazonaws]
- **Phase ordering** — Phase 1 (helpers) → Phase 2 (pod lifecycle) → Phase 3 (service proxy) before any deletion is a textbook safe migration sequence [ppl-ai-file-upload.s3.amazonaws]

***

## Issues & Improvements

### 🔴 Critical: No SNAT for Pod Outbound Traffic

The plan explicitly defers SNAT to "Phase 7+" and says to use `hostNetwork: true` as a workaround. This is a real operational blocker — any pod that needs to call an external API or pull data will have packets leaving via `eth0` with a source IP of `10.42.x.x`, which is unroutable on the internet. You should add **at minimum a `nftables` or `iptables MASQUERADE` rule** for pods going out via the host's default interface early (Phase 2 or 3), not Phase 7. [ppl-ai-file-upload.s3.amazonaws]

```
# Minimal SNAT — add via netlink nfnetlink or shell fallback:
iptables -t nat -A POSTROUTING -s 10.42.0.0/16 ! -d 10.42.0.0/16 -j MASQUERADE
```

### 🔴 Critical: IP Leak Across Nodes Without Network Policy

The plan routes pod CIDR via plain host routes with no filtering. Pod A on Node A can directly reach any pod on Node B with no restriction. There is no mention of **network policy** or even a basic default-deny story. For a system that can serve public HTTP traffic, this is a security gap worth at least noting — even if deferred. [ppl-ai-file-upload.s3.amazonaws]

### 🟠 Important: No IP Reclamation / Pool Management

The plan says "No IP reuse for MVP — 254 addresses per node is sufficient". This is fine for MVP, but the current wording gives no path forward. If a node runs pods in a long-lived production system, addresses will exhaust. A simple **bitmap or `BTreeSet<u8>`** free-list takes ~10 lines and should be included in Phase 2 — not deferred silently. [ppl-ai-file-upload.s3.amazonaws]

### 🟠 Important: Ingress `first-match wins` on Port Conflicts is Silent

The plan states "If multiple services claim the same port, first-match wins" for raw TCP ingress. This will surprise operators. You should log a **clear warning** when a second ingress rule tries to claim an already-bound port, and ideally reject it at admission time (when the resource is written to the store), not silently at runtime. [ppl-ai-file-upload.s3.amazonaws]

### 🟡 Moderate: `--node-ip` Auto-Detection is Underspecified

The config table says `--node-ip` defaults to `[auto-detected]`, but the plan never explains how. On hosts with multiple NICs (a VM with `eth0` for management and `eth1` for pod traffic), auto-detection can pick the wrong interface. Specify the detection algorithm — e.g., "IP of the interface that holds the default route" — or require explicit configuration. [ppl-ai-file-upload.s3.amazonaws]

### 🟡 Moderate: Veth Name Collision on Pod Restart

The veth host-end is named `veth-<pod>`. If a pod crashes and restarts with the same name before the old veth is cleaned up (e.g., the deletion netlink message failed), `NLM_F_EXCL` will return `EEXIST` and the new pod silently gets no `eth0`. The current error handling says "Log warning, pod has lo only" — but this should be an **active cleanup**: detect stale veths by checking the bridge ports on startup and removing orphans with `RTM_DELLINK`. [ppl-ai-file-upload.s3.amazonaws]

### 🟡 Moderate: SNI Parsing "First ~5 bytes" is Imprecise

Section 3.2 says "reads the ClientHello (first ~5 bytes)". The TLS record header is 5 bytes, but the SNI extension is deep inside the `ClientHello` handshake message — it can be hundreds of bytes in. You need to **read the full ClientHello** (up to the extensions) before you can extract SNI. The current description understates the actual read size and could lead to a buggy implementation. [ppl-ai-file-upload.s3.amazonaws]

### 🟢 Minor: No Mention of `ip_forward` Enablement

Section 4 mentions pods need `ip_forward=1` for outbound internet, but the plan never specifies *who sets it* or *when*. Add a line to Phase 1 startup code: `write "1" to /proc/sys/net/ipv4/ip_forward` — this is required for the bridge to route between pod CIDR and the host network. [ppl-ai-file-upload.s3.amazonaws]

### 🟢 Minor: Multi-Node Store Is Per-Node In-Memory

The plan acknowledges "cross-node pod discovery needs a future consensus layer". This means `find_endpoints()` on Node A will never return pods running on Node B unless there's a reconciliation mechanism. The ClusterIP proxy will silently only serve local backends in multi-node mode. This deserves a **clear NOTE in the relevant section**, not just in the Open Questions table. [ppl-ai-file-upload.s3.amazonaws]

***

## Suggested Additions to the Plan

| Gap | Recommended Action |
|---|---|
| Outbound SNAT | Add minimal `MASQUERADE` rule in Phase 2, not Phase 7 |
| IP pool management | Add `PodIpAllocator` (bitmap) in Phase 2 |
| Stale veth cleanup | Add `clean_orphan_veths()` at bridge startup |
| `ip_forward=1` | Explicit syscall in Phase 1 startup |
| Port conflict logging | Warn + reject on ingress rule conflict |
| SNI parsing | Clarify full ClientHello read, not just 5 bytes |
| `--node-ip` detection | Document algorithm or require explicit value |
| Multi-node endpoint gap | Add explicit NOTE to Phase 6 and §5.3 |

***

## Phase Priority Recommendation

The current phase ordering is correct, but consider pulling **SNAT (outbound masquerade)** into Phase 2 alongside veth creation. Without it, any real workload that calls out to the internet (package downloads, webhook calls, database connections to managed services) will silently fail — which will make Phase 2 look broken even when the networking itself is working correctly.