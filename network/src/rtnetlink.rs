//! # RTNETLINK — veth, addresses, routes, network namespaces
//!
//! The data-plane half that the nftables engine ([`crate::syscalls`]) does not
//! cover: creating veth pairs for pods, assigning IPs, installing routes, and
//! moving interfaces between network namespaces. We talk to `NETLINK_ROUTE`
//! directly — no `iproute2`, no `libc`, no `nix`.
//!
//! ## Why a fresh socket per call
//!
//! A netlink socket is bound to the network namespace it was created in. When
//! [`RouteSocket::attach_pod`] switches into a pod's netns with `setns`, any
//! pre-existing socket would still target the host netns. So every helper here
//! opens a short-lived socket, which binds to the *current* netns at call time.
//! This keeps namespace handling correct and the code stateless.
//!
//! ## Wire format
//!
//! Each message is `nlmsghdr(16) | family-struct | attrs`, where the
//! family-struct is `ifinfomsg` (links), `ifaddrmsg` (addresses), or `rtmsg`
//! (routes). Attributes use the standard `rtattr` layout (4-byte aligned).

use std::net::Ipv4Addr;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

use anyhow::{anyhow, Context, Result};
use nix::sys::socket::{
    self, AddressFamily, MsgFlags, NetlinkAddr, SockFlag, SockProtocol, SockType,
};
use rustix::thread::LinkNameSpaceType;
use tracing::{debug, info, warn};

use crate::model::{NetlinkOp, RouteSpec, VethPair};

// ═══════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_NEWROUTE: u16 = 24;
const RTM_DELROUTE: u16 = 25;

const NLM_F_REQUEST: u16 = 0x01;
const NLM_F_ACK: u16 = 0x04;
const NLM_F_EXCL: u16 = 0x200;
const NLM_F_CREATE: u16 = 0x400;

const NLMSG_ERROR: u16 = 2;

const RTN_UNICAST: u8 = 1;
const RT_TABLE_MAIN: u8 = 254;
const RT_SCOPE_UNIVERSE: u8 = 0;
const RT_SCOPE_LINK: u8 = 253;
const RTPROT_BOOT: u8 = 3;
const RTM_F_GATEWAY_FLAG: u32 = 4;

const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_LINKINFO: u16 = 18;
const IFLA_NET_NS_PID: u16 = 19;
const IFLA_INFO_KIND: u16 = 1;
const IFLA_INFO_DATA: u16 = 2;
const IFLA_VETH_PEER: u16 = 1;

const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;

const AF_INET: u8 = 2;
const IFF_UP: u32 = 1;

// ═══════════════════════════════════════════════════════════════════════════
// Attribute helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Encode a single `rtattr` carrying raw bytes (4-byte aligned, padded).
fn rtattr(kind: u16, data: &[u8]) -> Vec<u8> {
    let len = 4 + data.len();
    let padded = (len + 3) & !3;
    let mut buf = Vec::with_capacity(padded);
    buf.extend_from_slice(&(len as u16).to_ne_bytes());
    buf.extend_from_slice(&kind.to_ne_bytes());
    buf.extend_from_slice(data);
    buf.resize(padded, 0);
    buf
}

/// Encode an `rtattr` wrapping a u32 value.
fn rtattr_u32(kind: u16, val: u32) -> Vec<u8> {
    rtattr(kind, &val.to_ne_bytes())
}

/// Encode a nested `rtattr` (the children are already-encoded attrs).
fn rtattr_nested(kind: u16, children: &[u8]) -> Vec<u8> {
    rtattr(kind, children)
}

/// Build a 16-byte `nlmsghdr` followed by `body`.
fn nlmsghdr(msg_type: u16, flags: u16, seq: u32, pid: u32, body: &[u8]) -> Vec<u8> {
    let total = 16 + body.len();
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(&(total as u32).to_ne_bytes());
    buf.extend_from_slice(&msg_type.to_ne_bytes());
    buf.extend_from_slice(&flags.to_ne_bytes());
    buf.extend_from_slice(&seq.to_ne_bytes());
    buf.extend_from_slice(&pid.to_ne_bytes());
    buf.extend_from_slice(body);
    buf
}

// ═══════════════════════════════════════════════════════════════════════════
// Low-level socket transaction (one request, one reply)
// ═══════════════════════════════════════════════════════════════════════════

/// Open a fresh `NETLINK_ROUTE` socket bound to the current netns and run a
/// single request/reply transaction. Returns the reply bytes (the caller
/// parses interface indices etc.); errors carry the kernel errno.
fn txn(build: impl FnOnce(u32) -> Vec<u8>, what: &str) -> Result<Vec<u8>> {
    let sock = socket::socket(
        AddressFamily::Netlink,
        SockType::Raw,
        SockFlag::empty(),
        SockProtocol::NetlinkRoute,
    )
    .map_err(|e| anyhow!("{what}: open NETLINK_ROUTE socket: errno {}", e as i32))?;
    // Bind to (pid=0, groups=0). pid=0 means "auto-assign" by the kernel.
    let addr = NetlinkAddr::new(0, 0);
    socket::bind(sock.as_raw_fd(), &addr)
        .map_err(|e| anyhow!("{what}: bind route socket: errno {}", e as i32))?;
    // Use the bound pid as nlmsg_pid (nft CLI / rustables use 0; RTNETLINK
    // routing is more forgiving either way, but 0 is the userspace convention).
    let pid = 0u32;
    let msg = build(pid);

    let raw = sock.as_raw_fd();
    socket::sendto(raw, &msg, &NetlinkAddr::new(0, 0), MsgFlags::empty())
        .map_err(|e| anyhow!("{what}: send: errno {}", e as i32))?;

    let mut reply = vec![0u8; 8192];
    let n = socket::recv(raw, &mut reply, MsgFlags::empty())
        .map_err(|e| anyhow!("{what}: recv: errno {}", e as i32))?;
    reply.truncate(n);
    check_error(&reply, what)?;
    Ok(reply)
}

/// Inspect an `NLMSG_ERROR` reply: errno 0 is an ACK, anything else fails.
fn check_error(reply: &[u8], what: &str) -> Result<()> {
    if reply.len() >= 20 {
        let msg_type = u16::from_ne_bytes([reply[4], reply[5]]);
        if msg_type == NLMSG_ERROR {
            let errno = i32::from_ne_bytes([reply[16], reply[17], reply[18], reply[19]]);
            if errno != 0 {
                return Err(anyhow!("{what}: netlink errno {}", -errno));
            }
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Link operations
// ═══════════════════════════════════════════════════════════════════════════

/// Create a veth pair. When `peer_pid` is set, the peer is created directly in
/// that process's network namespace (so it lands in the pod with no extra
/// move). Returns the host-side interface index; the peer index is `0` when it
/// was placed into another netns and must be resolved there.
fn create_veth_pair(host: &str, peer: &str, peer_pid: Option<u32>) -> Result<(u32, u32)> {
    // IFLA_VETH_PEER carries an embedded ifinfomsg (16 zero bytes) + attrs.
    let mut peer_data = vec![0u8; 16];
    peer_data.extend_from_slice(&rtattr(IFLA_IFNAME, peer.as_bytes()));
    if let Some(pid) = peer_pid {
        peer_data.extend_from_slice(&rtattr_u32(IFLA_NET_NS_PID, pid));
    }
    let peer_nested = rtattr_nested(IFLA_VETH_PEER, &peer_data);

    let mut info_data = rtattr(IFLA_INFO_KIND, b"veth\0");
    info_data.extend_from_slice(&rtattr_nested(IFLA_INFO_DATA, &peer_nested));
    let linkinfo = rtattr_nested(IFLA_LINKINFO, &info_data);

    let mut body = vec![0u8; 16]; // ifinfomsg, all zero
    body.extend_from_slice(&rtattr(IFLA_IFNAME, host.as_bytes()));
    body.extend_from_slice(&rtattr_u32(IFLA_MTU, 1500));
    body.extend_from_slice(&linkinfo);

    let flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK;
    txn(
        |pid| nlmsghdr(RTM_NEWLINK, flags, 1, pid, &body),
        "create_veth",
    )?;

    let host_idx = get_ifindex(host)?;
    let peer_idx = if peer_pid.is_some() { 0 } else { get_ifindex(peer)? };
    Ok((host_idx, peer_idx))
}

/// Resolve an interface index by name in the current netns.
fn get_ifindex(name: &str) -> Result<u32> {
    let mut body = vec![0u8; 16];
    body[0] = AF_INET;
    body.extend_from_slice(&rtattr(IFLA_IFNAME, name.as_bytes()));
    let reply = txn(
        |pid| nlmsghdr(RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK, 1, pid, &body),
        "get_ifindex",
    )?;
    if reply.len() >= 24 {
        let msg_type = u16::from_ne_bytes([reply[4], reply[5]]);
        if msg_type != NLMSG_ERROR {
            // ifinfomsg.ifi_index is at offset 16+4 = 20.
            let idx = u32::from_ne_bytes([reply[20], reply[21], reply[22], reply[23]]);
            if idx != 0 {
                return Ok(idx);
            }
        }
    }
    Err(anyhow!("get_ifindex: interface {name} not found"))
}

/// Bring an interface up.
fn set_link_up(ifindex: u32) -> Result<()> {
    let mut body = vec![0u8; 16];
    body[0] = AF_INET;
    body[4..8].copy_from_slice(&ifindex.to_ne_bytes()); // ifi_index
    body[8..12].copy_from_slice(&IFF_UP.to_ne_bytes()); // ifi_flags
    body[12..16].copy_from_slice(&IFF_UP.to_ne_bytes()); // ifi_change
    txn(
        |pid| nlmsghdr(RTM_NEWLINK, NLM_F_REQUEST | NLM_F_ACK, 1, pid, &body),
        "set_link_up",
    )?;
    Ok(())
}

/// Delete an interface by index (also destroys its veth peer).
fn del_link(ifindex: u32) -> Result<()> {
    let mut body = vec![0u8; 16];
    body[0] = AF_INET;
    body[4..8].copy_from_slice(&ifindex.to_ne_bytes());
    txn(
        |pid| nlmsghdr(RTM_DELLINK, NLM_F_REQUEST | NLM_F_ACK, 1, pid, &body),
        "del_link",
    )?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Address operations
// ═══════════════════════════════════════════════════════════════════════════

/// Assign an IPv4 address to an interface.
fn add_addr(ifindex: u32, ip: Ipv4Addr, prefix: u8) -> Result<()> {
    // ifaddrmsg: family, prefixlen, flags, scope, index (u32).
    let mut body = vec![AF_INET, prefix, 0, 0];
    body.extend_from_slice(&ifindex.to_ne_bytes());
    let octets = ip.octets();
    body.extend_from_slice(&rtattr(IFA_LOCAL, &octets));
    body.extend_from_slice(&rtattr(IFA_ADDRESS, &octets));
    let flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK;
    txn(|pid| nlmsghdr(RTM_NEWADDR, flags, 1, pid, &body), "add_addr")?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Route operations
// ═══════════════════════════════════════════════════════════════════════════

/// Build an `rtmsg` route body (shared by add/del).
fn route_body(route: &RouteSpec) -> Vec<u8> {
    let mut attrs: Vec<u8> = Vec::new();
    if route.prefix > 0 {
        attrs.extend_from_slice(&rtattr(RTA_DST, &route.dest.octets()));
    }
    if let Some(gw) = route.gateway {
        attrs.extend_from_slice(&rtattr(RTA_GATEWAY, &gw.octets()));
    }
    if let Some(oif) = route.oif {
        attrs.extend_from_slice(&rtattr_u32(RTA_OIF, oif));
    }
    let scope = if route.gateway.is_some() {
        RT_SCOPE_UNIVERSE
    } else {
        RT_SCOPE_LINK
    };
    // rtmsg: family, dst_len, src_len, tos, table, protocol, scope, type, flags(u32)
    let mut body = vec![
        AF_INET,
        route.prefix,
        0,
        0,
        RT_TABLE_MAIN,
        RTPROT_BOOT,
        scope,
        RTN_UNICAST,
    ];
    let flags = if route.gateway.is_some() {
        RTM_F_GATEWAY_FLAG
    } else {
        0
    };
    body.extend_from_slice(&flags.to_ne_bytes());
    body.extend_from_slice(&attrs);
    body
}

/// Install a route.
fn add_route(route: &RouteSpec) -> Result<()> {
    let body = route_body(route);
    let flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK;
    txn(|pid| nlmsghdr(RTM_NEWROUTE, flags, 1, pid, &body), "add_route")?;
    Ok(())
}

/// Remove a route.
fn del_route(route: &RouteSpec) -> Result<()> {
    let body = route_body(route);
    let flags = NLM_F_REQUEST | NLM_F_ACK;
    txn(|pid| nlmsghdr(RTM_DELROUTE, flags, 1, pid, &body), "del_route")?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Network namespace switching
// ═══════════════════════════════════════════════════════════════════════════

/// RAII guard that restores the host network namespace on drop.
struct NetnsGuard {
    host_fd: OwnedFd,
}

impl NetnsGuard {
    /// Save the current (host) netns and enter the netns of `pid`.
    fn enter(pid: u32) -> Result<Self> {
        let host_fd = rustix::fs::open(
            "/proc/self/ns/net",
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .context("open host netns")?;
        let pod_fd = rustix::fs::open(
            format!("/proc/{pid}/ns/net"),
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .context("open pod netns")?;
        rustix::thread::move_into_link_name_space(pod_fd.as_fd(), Some(LinkNameSpaceType::Network))
            .context("setns into pod netns")?;
        Ok(Self { host_fd })
    }
}

impl Drop for NetnsGuard {
    fn drop(&mut self) {
        if let Err(e) = rustix::thread::move_into_link_name_space(
            self.host_fd.as_fd(),
            Some(LinkNameSpaceType::Network),
        ) {
            warn!("failed to restore host netns: {e}");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Naming
// ═══════════════════════════════════════════════════════════════════════════

/// The last 8 hex digits of a pod UID — stable, and unique across the replicas
/// of a deployment (whose UIDs share a prefix but differ in the suffix).
fn uid_suffix(pod_uid: &str) -> String {
    let hex: Vec<char> = pod_uid.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    let start = hex.len().saturating_sub(8);
    hex[start..].iter().collect()
}

/// Host-side veth name for a pod.
pub fn host_veth_name(pod_uid: &str) -> String {
    format!("veth-{}", uid_suffix(pod_uid))
}

/// Pod-side veth name for a pod.
pub fn peer_veth_name(pod_uid: &str) -> String {
    format!("zeth-{}", uid_suffix(pod_uid))
}

// ═══════════════════════════════════════════════════════════════════════════
// RouteSocket — the RTNETLINK facade used by the engine
// ═══════════════════════════════════════════════════════════════════════════

/// Stateless handle for RTNETLINK operations. Construction verifies that the
/// process can open a route socket (i.e. has `CAP_NET_ADMIN`); each method
/// opens its own short-lived socket so namespace switches are honored.
#[derive(Debug, Clone, Copy, Default)]
pub struct RouteSocket;

impl RouteSocket {
    /// Verify we can open a `NETLINK_ROUTE` socket.
    pub fn open() -> Result<Self> {
        let sock = socket::socket(
            AddressFamily::Netlink,
            SockType::Raw,
            SockFlag::empty(),
            SockProtocol::NetlinkRoute,
        )
        .map_err(|e| anyhow!("open NETLINK_ROUTE socket: errno {}", e as i32))?;
        let _ = socket::bind(sock.as_raw_fd(), &NetlinkAddr::new(0, 0));
        Ok(Self)
    }

    /// Apply a reconcile-managed route op (host netns).
    pub fn apply(&self, op: &NetlinkOp) -> Result<()> {
        match op {
            NetlinkOp::AddRoute { route } => add_route(route),
            NetlinkOp::DelRoute { route } => del_route(route),
            other => Err(anyhow!("RouteSocket cannot apply non-route op: {other:?}")),
        }
    }

    /// Attach a pod: host-side veth + gateway IP + `/32` route, then configure
    /// the pod-side interface (address, up, default route) inside its netns.
    pub fn attach_pod(
        &self,
        pod_uid: &str,
        pod_ip: Ipv4Addr,
        container_pid: u32,
        gateway: Ipv4Addr,
    ) -> Result<VethPair> {
        let host = host_veth_name(pod_uid);
        let peer = peer_veth_name(pod_uid);

        // Host side: create the pair (peer is born in the pod netns).
        let (host_idx, _) = create_veth_pair(&host, &peer, Some(container_pid))?;
        set_link_up(host_idx).context("host veth up")?;
        // Gateway as /32 on the host veth avoids conflicts between veths.
        if let Err(e) = add_addr(host_idx, gateway, 32) {
            debug!("assign gateway to host veth: {e} (continuing)");
        }
        add_route(&RouteSpec {
            dest: pod_ip,
            prefix: 32,
            gateway: None,
            oif: Some(host_idx),
        })
        .context("host /32 pod route")?;

        // Pod side: enter the pod netns and configure the peer.
        let peer_idx = {
            let _guard = NetnsGuard::enter(container_pid)?;
            let idx = get_ifindex(&peer).context("resolve peer in pod netns")?;
            add_addr(idx, pod_ip, 32).context("assign pod IP")?;
            set_link_up(idx).context("pod veth up")?;
            add_route(&RouteSpec {
                dest: Ipv4Addr::UNSPECIFIED,
                prefix: 0,
                gateway: Some(gateway),
                oif: Some(idx),
            })
            .context("pod default route")?;
            idx
            // guard drops here → back to host netns
        };

        info!(pod = pod_uid, ip = %pod_ip, host_idx, peer_idx, "pod attached");
        Ok(VethPair {
            host_name: host,
            peer_name: peer,
            host_ifindex: host_idx,
            peer_ifindex: peer_idx,
        })
    }

    /// Detach a pod: remove the host `/32` route and delete the veth pair.
    pub fn detach_pod(&self, host_veth: &str, pod_ip: Ipv4Addr) -> Result<()> {
        if let Ok(idx) = get_ifindex(host_veth) {
            let _ = del_route(&RouteSpec {
                dest: pod_ip,
                prefix: 32,
                gateway: None,
                oif: Some(idx),
            });
            del_link(idx).context("delete host veth")?;
        }
        Ok(())
    }

    /// Remove veth pairs that no longer correspond to an active pod.
    pub fn clean_orphan_veths(&self, active_uids: &[String]) -> Result<usize> {
        let active: std::collections::HashSet<String> =
            active_uids.iter().map(|u| host_veth_name(u)).collect();
        let mut removed = 0;
        let dir = match std::fs::read_dir("/sys/class/net") {
            Ok(d) => d,
            Err(_) => return Ok(0),
        };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("veth-") && !active.contains(&name)
                && let Ok(idx) = get_ifindex(&name)
                    && del_link(idx).is_ok() {
                        removed += 1;
                    }
        }
        Ok(removed)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests (pure encoding only — kernel ops need CAP_NET_ADMIN)
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtattr_is_padded() {
        let a = rtattr(IFLA_IFNAME, b"abc");
        // 4 header + 3 data = 7, padded to 8.
        assert_eq!(a.len(), 8);
        assert_eq!(u16::from_ne_bytes([a[0], a[1]]), 7);
    }

    #[test]
    fn rtattr_u32_len() {
        let a = rtattr_u32(RTA_OIF, 42);
        assert_eq!(a.len(), 8);
        assert_eq!(u32::from_ne_bytes([a[4], a[5], a[6], a[7]]), 42);
    }

    #[test]
    fn nlmsghdr_sets_len() {
        let m = nlmsghdr(RTM_NEWLINK, NLM_F_REQUEST, 1, 0, &[1, 2, 3, 4]);
        assert_eq!(m.len(), 20);
        assert_eq!(u32::from_ne_bytes([m[0], m[1], m[2], m[3]]), 20);
    }

    #[test]
    fn uid_suffix_takes_last_8_hex() {
        // Non-hex chars are stripped, then the last 8 hex digits are kept.
        assert_eq!(uid_suffix("zzzz0123456789ab"), "23456789ab"[2..].to_string());
        assert_eq!(uid_suffix("zzzz0123456789ab"), "456789ab");
        assert_eq!(host_veth_name("xxxx12345678"), "veth-12345678");
        assert_eq!(peer_veth_name("xxxx12345678"), "zeth-12345678");
    }

    #[test]
    fn route_body_layout() {
        let r = RouteSpec::host_via(Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(10, 0, 0, 1));
        let body = route_body(&r);
        assert_eq!(body[0], AF_INET);
        assert_eq!(body[1], 32); // prefix
        // gateway present → universe scope + gateway flag
        assert_eq!(body[6], RT_SCOPE_UNIVERSE);
    }

    #[test]
    fn route_body_link_scope_without_gateway() {
        let r = RouteSpec {
            dest: Ipv4Addr::new(10, 0, 0, 5),
            prefix: 32,
            gateway: None,
            oif: Some(3),
        };
        let body = route_body(&r);
        assert_eq!(body[6], RT_SCOPE_LINK);
    }
}
