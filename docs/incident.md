# k3s Build Pod — TCP Connectivity Incident Report

## 1. Symptoms
- **Host**: `curl`, `apt-get`, and other TCP tools failed with **"No route to host"** to any external IP/port
- **Pod**: same error, preventing k3s from pulling images (`ImagePullBackOff`) and pod `apt-get` from downloading packages
- **Chrome**: worked normally (uses QUIC/UDP on port 443)
- **Ping (ICMP)**: worked both on host and in pods

## 2. What Worked vs What Did Not

| Protocol | Host → Router | Host → Internet | Pod → Router | Pod → Internet |
|----------|:----------:|:-------------:|:----------:|:------------:|
| ICMP     | ✅ | ✅ | ✅ | ✅ |
| UDP/443  | ✅ | ✅ | N/A | N/A |
| TCP      | ✅ | ❌ | ✅ | ❌ |

## 3. Timeline

### Step 1 — Diagnostics
- `stop k3s` → host TCP instantly worked → **root cause was inside k3s**
- `start k3s` → host TCP broke again
- Flushed all iptables rules, restarted k3s → **host TCP fixed** (fresh iptables rules)
- Pod TCP still broken even after host fix

### Step 2 — iptables Analysis (after flush + restart)
```
FORWARD (policy ACCEPT)
  → KUBE-ROUTER-FORWARD      → jumps to KUBE-POD-FW-XXXXX
     → KUBE-NWPLCY-COMMON     → ICMP allowed, TCP falls through
     → KUBE-NWPLCY-DEFAULT    → marks packet 0x10000
     → mark check             → mark IS 0x10000, so no REJECT
     → clears mark, sets mark 0x20000
  → KUBE-FORWARD (empty)
  → KUBE-SERVICES (empty)
  → KUBE-EXTERNAL-SERVICES (empty)
  → mark 0x20000 → ACCEPT
  → FLANNEL-FWD → ACCEPT
  → POSTROUTING (MASQUERADE) → SNAT to host IP
```

### Step 3 — Root Cause Found (2026-05-30)
After source code analysis (`src/netmux/nftables.rs`):
- z8s creates its own FORWARD chain in the `z8s_filter` nftables table with **policy DROP**
- nftables chains with the same priority (0) are evaluated in **alphabetical table order**: `z8s_filter` < `kube-*`
- Every forwarded TCP SYN from a k3s-managed pod hit z8s's FORWARD chain FIRST
- The packet did NOT match any z8s rule (no NSG match, catch-all only accepts `10.100.0.0/16`)
- z8s's **default DROP policy killed the packet** before k3s's chains ever saw it

### Step 4 — Fix Applied
Changed forward chain policy from `DROP` to `ACCEPT` in `nftables.rs`:
```rust
ChainPolicy::Drop  →  ChainPolicy::Accept
```

## 4. Root Cause
**z8s's nftables FORWARD chain had `policy drop`, which intercepted and discarded all forwarded TCP traffic before k3s's own firewall rules could evaluate it.**

| Factor | Detail |
|--------|--------|
| **z8s table** | `z8s_filter` (alphabetically before `kube-*`) |
| **z8s priority** | 0 (same as k3s filter chains) |
| **z8s default** | `drop` — anything not matching z8s rules is killed |
| **k3s traffic** | TCP SYN from k3s pod → z8s chain first → no match → **DROPPED** |
| **ICMP worked** | ICMP has its own rules in k3s's chains, hit BEFORE z8s |
| **UDP worked** | Same as ICMP — explicit accept before z8s chain |

The `nft flush ruleset` commands run during debugging destroyed all nftables tables,
which meant k3s's `kube-*` chains no longer existed. After a k3s restart, k3s recreated
its chains, but the z8s chain (with `policy drop`) was still intercepting traffic.

## 5. Why Reboot Fixed It
Rebooting cleared the z8s process and its nftables rules. k3s started fresh without
z8s's drop-policy chain in the way. TCP worked again.

## 6. IPv6
- Disabled as a debugging step (`sysctl net.ipv6.conf.all.disable_ipv6=1`)
- Was a **false lead** — the issue was purely with IPv4 TCP
- Can be re-enabled if desired

## 7. Current State (Post-Fix)

| Component | Status |
|-----------|--------|
| Host TCP to internet | ✅ Working |
| Pod TCP to internet | ✅ Working |
| k3s pods | All healthy |
| z8s forward policy | `accept` (was `drop`) |

## 8. Recommendations
1. **Never run `sudo nft flush ruleset`** while z8s is running — it destroys both z8s and k3s tables, requiring a full reboot to recover
2. If TCP issues recur, check `sudo nft list chain ip z8s_filter forward` — if `policy drop` is present, z8s is running an old binary
3. Keep z8s forward chain policy as `accept` — NSG rules still work (evaluated before the accept policy via `jump nsg-rules`)
4. For spoke isolation, use explicit NSG deny rules instead of relying on default-drop
