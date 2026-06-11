//! # RTNETLINK — Raw Netlink Operations
//!
//! This module provides low-level netlink operations for:
//! - Link management (create, delete, set up/down, get ifindex)
//! - Address management (add, delete IPv4 addresses)
//! - Route management (add, delete IPv4 routes)
//! - Sysctl (IP forwarding, etc.)
//!
//! ## Why a Custom Implementation?
//!
//! The plan (§2: `PERFORMANCE-PLAN.md`) calls for **batched netlink** so the
//! veth + addr + route setup is one round-trip instead of 4. Since no
//! off-the-shelf crate offers this with sufficient control, we use raw
//! netlink via `rustix` for socket I/O and a small hand-rolled message
//! encoder for the few operations we need.
//!
//! ## Linux RTNETLINK message format
//!
//! ```text
//! struct nlmsghdr {
//!     __u32 nlmsg_len;    // length incl. header
//!     __u16 nlmsg_type;    // RTM_NEWLINK / RTM_DELLINK / RTM_NEWADDR / ...
//!     __u16 nlmsg_flags;  // NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | ...
//!     __u32 nlmsg_seq;    // sequence number
//!     __u32 nlmsg_pid;    // port ID (0 for kernel)
//! };
//! ```

use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use anyhow::{anyhow, Context, Result};
use rustix::net::{AddressFamily, SocketType, SocketFlags};

/// RTM message types we use.
pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_GETLINK: u16 = 18;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_DELADDR: u16 = 21;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;
pub const RTM_GETROUTE: u16 = 26;

/// NLM flags.
pub const NLM_F_REQUEST: u16 = 0x01;
pub const NLM_F_ACK: u16 = 0x04;
pub const NLM_F_CREATE: u16 = 0x400;
pub const NLM_F_EXCL: u16 = 0x200;

/// Address families.
pub const AF_INET: u16 = 2;
pub const AF_NETLINK: u16 = 16;

/// Link-level attributes.
pub const IFLA_IFNAME: u16 = 3;
pub const IFLA_MTU: u16 = 4;
pub const IFLA_LINKINFO: u16 = 18;
pub const IFLA_NET_NS_PID: u16 = 19;
pub const IFLA_ADDRESS: u16 = 1;
pub const IFLA_MASTER: u16 = 10;

/// Link info sub-attributes.
pub const IFLA_INFO_KIND: u16 = 1;
pub const IFLA_INFO_DATA: u16 = 2;

/// Netlink response types.
pub const NLMSG_ERROR: u16 = 2;
pub const NLMSG_DONE: u16 = 3;

/// Netlink error codes (positive = errno).
pub const NLMSGERR_NO_ERROR: i32 = 0;

/// Create a netlink socket for RTNETLINK.
pub fn netlink_socket() -> Result<OwnedFd> {
    let fd = rustix::net::socket_with(
        AddressFamily::NETLINK,
        SocketType::RAW,
        SocketFlags::CLOEXEC,
        None,
    )
    .context("netlink socket()")?;
    Ok(fd)
}

/// Send a netlink message and read the response.
pub fn send_nlmsg(fd: &OwnedFd, buf: &[u8]) -> Result<()> {
    let n = unsafe {
        libc_send(fd.as_raw_fd(), buf.as_ptr() as *const _, buf.len(), 0)
    };
    if n < 0 {
        return Err(anyhow!("netlink send returned {}", n));
    }
    if (n as usize) != buf.len() {
        return Err(anyhow!("netlink short send: {} of {}", n, buf.len()));
    }
    Ok(())
}

pub fn recv_nlmsg(fd: &OwnedFd) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; 8192];
    let n = unsafe { libc_recv(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), 0) };
    if n < 0 {
        return Err(anyhow!("netlink recv returned {}", n));
    }
    buf.truncate(n as usize);
    Ok(buf)
}

/// Send a message and parse the response for errors.
pub fn send_recv(fd: &OwnedFd, buf: &[u8]) -> Result<()> {
    send_nlmsg(fd, buf)?;
    let resp = recv_nlmsg(fd)?;
    if resp.len() < 16 {
        return Ok(()); // too short to be an error
    }
    let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
    if msg_type == NLMSG_ERROR && resp.len() >= 20 {
        let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
        if err_code != 0 {
            return Err(anyhow!("netlink error {}", -err_code));
        }
    }
    Ok(())
}

/// Build an `nlattr` header + payload.
pub fn nlattr_bytes(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    let total_len = 4 + payload.len();
    out.extend_from_slice(&(total_len as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(payload);
    // Pad to 4-byte alignment
    let pad = (4 - (payload.len() % 4)) % 4;
    out.extend(std::iter::repeat(0u8).take(pad));
    out
}

/// Build a 32-bit nlattr.
pub fn nlattr_u32(kind: u16, value: u32) -> Vec<u8> {
    nlattr_bytes(kind, &value.to_ne_bytes())
}

/// Create a veth pair.
///
/// - `host_name`: name of the host-side veth interface
/// - `peer_name`: name of the peer-side veth interface
/// - `peer_pid`: if Some, peer is moved into that PID's netns at creation
///
/// Returns `(host_ifindex, peer_ifindex)`. If `peer_pid` is set, peer_ifindex
/// is 0 (must be resolved inside the target netns).
pub fn create_veth_pair(
    host_name: &str,
    peer_name: &str,
    peer_pid: Option<u32>,
) -> Result<(u32, u32)> {
    let fd = netlink_socket()?;

    // RTM_NEWLINK with IFLA_IFNAME = host_name
    // IFLA_LINKINFO with IFLA_INFO_KIND = "veth"
    // IFLA_INFO_DATA with IFLA_INFO_KIND peer's name as IFLA_IFNAME
    // Optionally IFLA_NET_NS_PID

    let host_name_attr = nlattr_bytes(IFLA_IFNAME, host_name.as_bytes());

    // Link info: kind=veth, data={IFLA_IFNAME=peer_name}
    let peer_name_attr = nlattr_bytes(IFLA_IFNAME, peer_name.as_bytes());
    let mut info_data = vec![];
    info_data.extend_from_slice(&peer_name_attr);

    let mut linkinfo_inner = vec![];
    linkinfo_inner.extend_from_slice(&nlattr_bytes(IFLA_INFO_KIND, b"veth"));
    linkinfo_inner.extend_from_slice(&nlattr_bytes(IFLA_INFO_DATA, &info_data));
    let linkinfo_attr = nlattr_bytes(IFLA_LINKINFO, &linkinfo_inner);

    let mut netns_attr = Vec::new();
    if let Some(pid) = peer_pid {
        netns_attr = nlattr_u32(IFLA_NET_NS_PID, pid);
    }

    // ifinfomsg header (16 bytes)
    let mut ifi = vec![0u8; 16];
    ifi[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    // The rest is zero

    let total_payload: Vec<u8> = ifi
        .iter()
        .chain(host_name_attr.iter())
        .chain(linkinfo_attr.iter())
        .chain(netns_attr.iter())
        .copied()
        .collect();

    let total_len = 16 + total_payload.len(); // 16 = nlmsghdr
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;

    let host_idx = get_ifindex(host_name)?;
    let peer_idx = if peer_pid.is_none() {
        get_ifindex(peer_name)?
    } else {
        0
    };
    Ok((host_idx, peer_idx))
}

/// Get ifindex of a named interface.
pub fn get_ifindex(name: &str) -> Result<u32> {
    // Try sysfs first — faster and doesn't need a netlink roundtrip
    let path = format!("/sys/class/net/{}/ifindex", name);
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(idx) = content.trim().parse::<u32>() {
            return Ok(idx);
        }
    }
    Err(anyhow!("interface {} not found", name))
}

/// Set link up or down.
pub fn set_link_up(ifindex: u32) -> Result<()> {
    set_link_state(ifindex, true)
}

pub fn set_link_down(ifindex: u32) -> Result<()> {
    set_link_state(ifindex, false)
}

fn set_link_state(ifindex: u32, up: bool) -> Result<()> {
    let fd = netlink_socket()?;
    // ifinfomsg.ifi_flags = IFF_UP (0x1) when up, 0 when down
    let mut ifi = vec![0u8; 16];
    ifi[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    if up {
        ifi[2..4].copy_from_slice(&0x1u16.to_ne_bytes());
    }

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..32].copy_from_slice(&ifi);

    // Target ifindex in ifi_change (offset 12) — actually offset 4 in ifinfomsg
    let mut buf2 = buf.clone();
    buf2[20..24].copy_from_slice(&ifindex.to_ne_bytes());
    send_recv(&fd, &buf2)?;
    Ok(())
}

/// Add an IPv4 address to an interface.
pub fn add_addr(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    let fd = netlink_socket()?;
    // ifaddrmsg: family(2), prefixlen(1), flags(1), scope(1), index(4)
    let mut ifa = vec![0u8; 8];
    ifa[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    ifa[2] = prefix;
    ifa[3] = 0; // flags
    ifa[4] = 0; // scope = global
    ifa[4..8].copy_from_slice(&ifindex.to_ne_bytes());

    // IFA_LOCAL + IFA_ADDRESS (both for IPv4 — same value)
    let ip_bytes = ip.octets();
    let local_attr = nlattr_bytes(1, &ip_bytes); // IFA_LOCAL = 1
    let addr_attr = nlattr_bytes(2, &ip_bytes); // IFA_ADDRESS = 2

    let total_payload: Vec<u8> = ifa
        .iter()
        .chain(local_attr.iter())
        .chain(addr_attr.iter())
        .copied()
        .collect();

    let total_len = 16 + total_payload.len();
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWADDR.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Delete an IPv4 address from an interface.
pub fn del_addr(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    let fd = netlink_socket()?;
    let mut ifa = vec![0u8; 8];
    ifa[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    ifa[2] = prefix;
    ifa[4..8].copy_from_slice(&ifindex.to_ne_bytes());

    let ip_bytes = ip.octets();
    let local_attr = nlattr_bytes(1, &ip_bytes);

    let total_payload: Vec<u8> = ifa.iter().chain(local_attr.iter()).copied().collect();
    let total_len = 16 + total_payload.len();
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELADDR.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Add an IPv4 route.
pub fn add_route(
    dest: &Ipv4Addr,
    prefix: u8,
    via: Option<&Ipv4Addr>,
    dev_ifindex: Option<u32>,
) -> Result<()> {
    let fd = netlink_socket()?;
    // rtmsg: family(1), dst_len(1), src_len(1), tos(1), table(1), protocol(1),
    //         scope(1), type(1), flags(4)
    let mut rt = vec![0u8; 12];
    rt[0] = AF_INET as u8;
    rt[1] = prefix; // dst_len
    rt[2] = 0; // src_len
    rt[3] = 0; // tos
    rt[4] = 254; // table = RT_TABLE_MAIN
    rt[5] = 2; // protocol = RTPROT_BOOT
    rt[6] = 0; // scope = RT_SCOPE_UNIVERSE
    rt[7] = 1; // type = RTN_UNICAST

    // RTA_DST
    let mut attrs: Vec<Vec<u8>> = vec![nlattr_bytes(1, &dest.octets())]; // RTA_DST = 1
    if let Some(gw) = via {
        attrs.push(nlattr_bytes(5, &gw.octets())); // RTA_GATEWAY = 5
    }
    if let Some(idx) = dev_ifindex {
        attrs.push(nlattr_u32(4, idx)); // RTA_OIF = 4
    }

    let mut total_payload: Vec<u8> = rt;
    for a in &attrs {
        total_payload.extend_from_slice(a);
    }

    let total_len = 16 + total_payload.len();
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWROUTE.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Delete an IPv4 route.
pub fn del_route(
    dest: &Ipv4Addr,
    prefix: u8,
    via: Option<&Ipv4Addr>,
    dev_ifindex: Option<u32>,
) -> Result<()> {
    let fd = netlink_socket()?;
    let mut rt = vec![0u8; 12];
    rt[0] = AF_INET as u8;
    rt[1] = prefix;
    rt[4] = 254;
    rt[5] = 2;
    rt[6] = 0;
    rt[7] = 1;

    let mut attrs: Vec<Vec<u8>> = vec![nlattr_bytes(1, &dest.octets())];
    if let Some(gw) = via {
        attrs.push(nlattr_bytes(5, &gw.octets()));
    }
    if let Some(idx) = dev_ifindex {
        attrs.push(nlattr_u32(4, idx));
    }

    let mut total_payload: Vec<u8> = rt;
    for a in &attrs {
        total_payload.extend_from_slice(a);
    }

    let total_len = 16 + total_payload.len();
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELROUTE.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Delete a link by ifindex.
pub fn del_link(ifindex: u32) -> Result<()> {
    let fd = netlink_socket()?;
    let mut ifi = vec![0u8; 16];
    ifi[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    ifi[4..8].copy_from_slice(&ifindex.to_ne_bytes());

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..32].copy_from_slice(&ifi);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Move a peer interface into a PID's network namespace.
pub fn move_peer_to_netns(peer_ifindex: u32, pid: u32) -> Result<()> {
    let fd = netlink_socket()?;
    let ns_pid_attr = nlattr_u32(IFLA_NET_NS_PID, pid);

    let mut ifi = vec![0u8; 16];
    ifi[0..2].copy_from_slice(&AF_INET.to_ne_bytes());
    ifi[4..8].copy_from_slice(&peer_ifindex.to_ne_bytes());

    let total_payload: Vec<u8> = ifi.iter().chain(ns_pid_attr.iter()).copied().collect();
    let total_len = 16 + total_payload.len();
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());
    buf[16..].copy_from_slice(&total_payload);

    send_recv(&fd, &buf)?;
    Ok(())
}

/// Set a sysctl value (e.g. /proc/sys/net/ipv4/ip_forward).
pub fn set_sysctl(path: &str, value: &str) -> Result<()> {
    std::fs::write(path, value).with_context(|| format!("write {}", path))?;
    Ok(())
}

/// Enable IP forwarding.
pub fn enable_ip_forward() -> Result<()> {
    set_sysctl("/proc/sys/net/ipv4/ip_forward", "1")
}

/// Ensure the loopback interface is up.
pub fn ensure_loopback_up() -> Result<()> {
    let _ = std::process::Command::new("ip")
        .args(["link", "set", "lo", "up"])
        .output();
    Ok(())
}

/// Add a local route for a service CIDR (so the kernel keeps DNATed packets local).
pub fn add_local_service_cidr(base: &Ipv4Addr, prefix: u8) -> Result<()> {
    // Local route: 10.96.0.0/12 dev lo — keeps ClusterIP lookups on the host
    add_route(base, prefix, None, Some(get_ifindex("lo")?))
        .or_else(|_| Ok::<(), anyhow::Error>(())) // best-effort
}

/// Open a netns by path.
pub fn open_netns(path: &str) -> Result<OwnedFd> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .with_context(|| format!("open {}", path))?;
    Ok(fd)
}

// ── Raw libc send/recv (rustix doesn't expose them generically) ─────────

unsafe extern "C" {
    fn send(fd: i32, buf: *const libc::c_void, len: usize, flags: i32) -> isize;
    fn recv(fd: i32, buf: *mut libc::c_void, len: usize, flags: i32) -> isize;
}

unsafe fn libc_send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize {
    unsafe { send(fd, buf as *const libc::c_void, len, flags) }
}

unsafe fn libc_recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize {
    unsafe { recv(fd, buf as *mut libc::c_void, len, flags) }
}

// We need a libc type alias to avoid pulling in the `libc` crate.
// Use a small struct as a placeholder; only used in extern decls.
mod libc {
    pub type c_void = core::ffi::c_void;
}

// Re-export for tests
#[allow(unused_imports)]
use OwnedFd as _OwnedFd;

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nlattr_bytes_alignment() {
        let attr = nlattr_bytes(IFLA_IFNAME, b"lo");
        // Header (4) + "lo" (2) = 6 → padded to 8
        assert_eq!(attr.len(), 8);
        assert_eq!(u16::from_ne_bytes([attr[0], attr[1]]), 6);
        assert_eq!(u16::from_ne_bytes([attr[2], attr[3]]), IFLA_IFNAME);
    }

    #[test]
    fn nlattr_bytes_no_pad_when_aligned() {
        let attr = nlattr_bytes(IFLA_IFNAME, b"loopback0");
        // Header (4) + "loopback0" (9) = 13 → padded to 16
        assert_eq!(attr.len(), 16);
    }

    #[test]
    fn nlattr_u32_size() {
        let attr = nlattr_u32(1, 1234);
        // Header (4) + u32 (4) = 8
        assert_eq!(attr.len(), 8);
        assert_eq!(u32::from_ne_bytes([attr[4], attr[5], attr[6], attr[7]]), 1234);
    }
}
