/// SAFETY: All netlink operations use raw libc FFI because rtnetlink
/// functions (veth creation, routes, addresses) are not available via
/// the `nix` crate. Each unsafe block is justified inline.

use std::mem;
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use anyhow::{Context, Result};

pub const RTM_NEWLINK: u16 = 16;
pub const RTM_DELLINK: u16 = 17;
pub const RTM_GETLINK: u16 = 18;
pub const RTM_NEWADDR: u16 = 20;
pub const RTM_NEWROUTE: u16 = 24;
pub const RTM_DELROUTE: u16 = 25;
pub const RTM_GETROUTE: u16 = 26;

pub const NLM_F_REQUEST: u16 = 1;
pub const NLM_F_CREATE: u16 = 0x400;
pub const NLM_F_EXCL: u16 = 0x200;
pub const NLM_F_ACK: u16 = 4;

pub const NLMSG_ERROR: u16 = 2;

pub const RTN_UNICAST: u8 = 1;
pub const RT_TABLE_MAIN: u32 = 254;
pub const RT_SCOPE_UNIVERSE: u8 = 0;
pub const RT_SCOPE_LINK: u8 = 253;
pub const RTPROT_BOOT: u8 = 3;

pub const IFLA_IFNAME: u16 = 3;
pub const IFLA_LINKINFO: u16 = 18;
pub const IFLA_INFO_KIND: u16 = 1;
pub const IFLA_INFO_DATA: u16 = 2;
pub const IFLA_VETH_PEER: u16 = 1;
pub const IFLA_MTU: u16 = 4;
pub const IFLA_NET_NS_PID: u16 = 19;

pub const RTA_GATEWAY: u16 = 5;
pub const RTA_OIF: u16 = 4;
pub const RTA_DST: u16 = 1;

pub const IFA_LOCAL: u16 = 2;
pub const IFA_ADDRESS: u16 = 1;

pub const AF_INET: i32 = 2;
pub const AF_NETLINK: i32 = 16;
pub const NETLINK_ROUTE: i32 = 0;

pub const IFF_UP: i32 = 1;
pub const IFF_RUNNING: i32 = 0x40;

// ── Netlink helpers ─────────────────────────────────────────────────────────

/// Open a netlink socket with explicit bind.
pub fn netlink_socket() -> Result<std::os::fd::OwnedFd> {
    let fd = nix::sys::socket::socket(
        nix::sys::socket::AddressFamily::Netlink,
        nix::sys::socket::SockType::Raw,
        nix::sys::socket::SockFlag::SOCK_CLOEXEC,
        nix::sys::socket::SockProtocol::NetlinkRoute,
    )
    .context("netlink socket")?;
    let addr = nix::sys::socket::NetlinkAddr::new(0, 0);
    nix::sys::socket::bind(fd.as_raw_fd(), &addr).context("netlink bind")?;
    Ok(fd)
}

pub fn send_nlmsg(fd: &std::os::fd::OwnedFd, buf: &[u8]) -> Result<()> {
    let raw = fd.as_raw_fd();
    // SAFETY: libc sendmsg FFI on valid fd. iovec points to valid buffer. Return checked.
    unsafe {
        let iov = nix::libc::iovec {
            iov_base: buf.as_ptr() as *mut nix::libc::c_void,
            iov_len: buf.len(),
        };
        let mut msg: nix::libc::msghdr = mem::zeroed();
        msg.msg_iov = &iov as *const _ as *mut _;
        msg.msg_iovlen = 1;
        let sent = nix::libc::sendmsg(raw, &msg, 0);
        if sent < 0 {
            return Err(std::io::Error::last_os_error()).context("netlink sendmsg");
        }
    }
    Ok(())
}

pub fn recv_nlmsg(fd: &std::os::fd::OwnedFd) -> Result<Vec<u8>> {
    let raw = fd.as_raw_fd();
    let mut buf = vec![0u8; 8192]; // 8KB buffer — sufficient for netlink responses
    // SAFETY: libc recvmsg FFI. iovec points to valid buffer. Return checked.
    unsafe {
        let iov = nix::libc::iovec {
            iov_base: buf.as_mut_ptr() as *mut nix::libc::c_void,
            iov_len: buf.len(),
        };
        let mut msg: nix::libc::msghdr = mem::zeroed();
        msg.msg_iov = &iov as *const _ as *mut _;
        msg.msg_iovlen = 1;
        let n = nix::libc::recvmsg(raw, &mut msg, 0);
        if n < 0 {
            return Err(std::io::Error::last_os_error()).context("netlink recvmsg");
        }
        buf.truncate(n as usize);
    }
    Ok(buf)
}

pub fn nlattr<T: Copy>(nla_type: u16, data: &T) -> Vec<u8> {
    let size = mem::size_of::<T>() + 4;
    let mut buf = vec![0u8; size];
    buf[0..2].copy_from_slice(&(size as u16).to_ne_bytes());
    buf[2..4].copy_from_slice(&nla_type.to_ne_bytes());
    // SAFETY: T is Copy, data points to valid memory of known size (mem::size_of::<T>())
    let data_bytes = unsafe { std::slice::from_raw_parts(data as *const T as *const u8, mem::size_of::<T>()) };
    buf[4..].copy_from_slice(data_bytes);
    buf
}

pub fn nlattr_bytes(nla_type: u16, data: &[u8]) -> Vec<u8> {
    let padded = (data.len() + 3) & !3;
    let size = padded + 4;
    let mut buf = vec![0u8; size];
    buf[0..2].copy_from_slice(&(size as u16).to_ne_bytes());
    buf[2..4].copy_from_slice(&nla_type.to_ne_bytes());
    buf[4..4+data.len()].copy_from_slice(data);
    buf
}

pub fn nlattr_nested(nla_type: u16, attrs: &[u8]) -> Vec<u8> {
    let size = attrs.len() + 4;
    let mut buf = vec![0u8; size];
    buf[0..2].copy_from_slice(&(size as u16).to_ne_bytes());
    buf[2..4].copy_from_slice(&nla_type.to_ne_bytes());
    buf[4..].copy_from_slice(attrs);
    buf
}

/// Check netlink response for error. Returns Ok(()) if response is not an error.
fn check_nl_response(resp: &[u8], context: &str) -> Result<()> {
    if resp.len() >= 16 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type == NLMSG_ERROR && resp.len() >= 20 {
            let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
            if err_code != 0 {
                return Err(anyhow::anyhow!("{}: netlink error {}", context, err_code));
            }
        }
    }
    Ok(())
}

// ── Veth creation ───────────────────────────────────────────────────────────

pub fn create_veth_pair(host_name: &str, peer_name: &str, peer_pid: Option<u32>) -> Result<(u32, u32)> {
    let fd = netlink_socket()?;

    let mut peer_data = Vec::new();
    // IFLA_VETH_PEER must start with a full ifinfomsg struct (all zeros, matching ip command)
    let mut peer_infomsg = vec![0u8; 16];
    peer_data.extend_from_slice(&peer_infomsg);
    peer_data.extend_from_slice(&nlattr_bytes(IFLA_IFNAME, peer_name.as_bytes()));
    if let Some(pid) = peer_pid {
        peer_data.extend_from_slice(&nlattr(IFLA_NET_NS_PID, &pid));
    }
    let peer_nested = nlattr_nested(IFLA_VETH_PEER, &peer_data);

    let mut info_data = Vec::new();
    info_data.extend_from_slice(&nlattr_bytes(IFLA_INFO_KIND, b"veth\0"));
    // IFLA_INFO_DATA wraps the peer info
    info_data.extend_from_slice(&nlattr_nested(IFLA_INFO_DATA, &peer_nested));
    let info_nested = nlattr_nested(IFLA_LINKINFO, &info_data);

    let ifname_attr = nlattr_bytes(IFLA_IFNAME, host_name.as_bytes());
    let mtu_attr = nlattr(IFLA_MTU, &1500u32);

    let payload_len = 16 + ifname_attr.len() + mtu_attr.len() + info_nested.len();
    let total_len = 16 + payload_len;
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    // AF_UNSPEC for veth creation (virtual ethernet device, not L3-specific)
    buf[16] = 0;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&0i32.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    let mut offset = 32;
    for attr in [&ifname_attr, &mtu_attr, &info_nested] {
        buf[offset..offset+attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "create_veth")?;

    let host_idx = get_ifindex(host_name)?;
    let peer_idx = get_ifindex(peer_name)?;

    Ok((host_idx, peer_idx))
}

pub fn get_ifindex(name: &str) -> Result<u32> {
    let fd = netlink_socket()?;

    let name_attr = nlattr_bytes(IFLA_IFNAME, name.as_bytes());
    let total_len = 16 + 16 + name_attr.len();
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_GETLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&0i32.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    let offset = 32;
    buf[offset..offset+name_attr.len()].copy_from_slice(&name_attr);

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop

    if resp.len() >= 20 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type != NLMSG_ERROR {
            let idx = u32::from_ne_bytes([resp[20], resp[21], resp[22], resp[23]]);
            if idx != 0 {
                return Ok(idx);
            }
        }
    }

    Err(anyhow::anyhow!("get_ifindex: interface {} not found", name))
}

pub fn set_link_up(ifindex: u32) -> Result<()> {
    let fd = netlink_socket()?;

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());
    buf[24..28].copy_from_slice(&(IFF_UP as u32).to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "set_link_up")
}

pub fn add_route(dest: &Ipv4Addr, prefix: u8, gateway: Option<&Ipv4Addr>, oif: Option<u32>) -> Result<()> {
    let fd = netlink_socket()?;

    let dest_bytes = dest.octets();
    let mut attrs: Vec<Vec<u8>> = Vec::new();
    attrs.push(nlattr(RTA_DST, &u32::from_ne_bytes(dest_bytes)));

    if let Some(gw) = gateway {
        let gw_bytes = gw.octets();
        attrs.push(nlattr(RTA_GATEWAY, &u32::from_ne_bytes(gw_bytes)));
    }
    if let Some(idx) = oif {
        attrs.push(nlattr(RTA_OIF, &idx));
    }

    let scope = if gateway.is_some() { RT_SCOPE_UNIVERSE } else { RT_SCOPE_LINK };
    let attrs_len: usize = attrs.iter().map(|a| a.len()).sum();
    let total_len = 16 + 12 + attrs_len; // nlmsghdr(16) + rtmsg(12) + attrs
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWROUTE.to_ne_bytes());
    // Use CREATE without EXCL so existing routes are replaced (avoids EEXIST on re-run)
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    // rtmsg: struct rtmsg — order: family, dst_len, src_len, tos, table, protocol, scope, type, flags
    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20] = RT_TABLE_MAIN as u8;
    buf[21] = RTPROT_BOOT;
    buf[22] = scope;
    buf[23] = RTN_UNICAST;
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes()); // rtm_flags

    let mut offset = 28;
    for attr in &attrs {
        buf[offset..offset+attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "add_route")
}

pub fn del_route(dest: &Ipv4Addr, prefix: u8, gateway: Option<&Ipv4Addr>, oif: Option<u32>) -> Result<()> {
    let fd = netlink_socket()?;

    let dest_bytes = dest.octets();
    let mut attrs: Vec<Vec<u8>> = Vec::new();
    attrs.push(nlattr(RTA_DST, &u32::from_ne_bytes(dest_bytes)));

    if let Some(gw) = gateway {
        let gw_bytes = gw.octets();
        attrs.push(nlattr(RTA_GATEWAY, &u32::from_ne_bytes(gw_bytes)));
    }
    if let Some(idx) = oif {
        attrs.push(nlattr(RTA_OIF, &idx));
    }

    let attrs_len: usize = attrs.iter().map(|a| a.len()).sum();
    let total_len = 16 + 12 + attrs_len; // nlmsghdr(16) + rtmsg(12) + attrs
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELROUTE.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    // rtmsg: order: family, dst_len, src_len, tos, table, protocol, scope, type, flags
    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20] = RT_TABLE_MAIN as u8;
    buf[21] = RTPROT_BOOT;
    buf[22] = RT_SCOPE_UNIVERSE;
    buf[23] = RTN_UNICAST;
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());

    let mut offset = 28;
    for attr in &attrs {
        buf[offset..offset+attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "del_route")
}

pub fn add_addr(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    let fd = netlink_socket()?;

    let ip_bytes = ip.octets();
    let local_attr = nlattr(IFA_LOCAL, &u32::from_ne_bytes(ip_bytes));
    let addr_attr = nlattr(IFA_ADDRESS, &u32::from_ne_bytes(ip_bytes));

    let total_len = 16 + 8 + local_attr.len() + addr_attr.len();
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWADDR.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());

    let mut offset = 24;
    for attr in [&local_attr, &addr_attr] {
        buf[offset..offset+attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "add_addr")
}

pub fn del_link(ifindex: u32) -> Result<()> {
    let fd = netlink_socket()?;

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    // fd auto-closed on drop
    check_nl_response(&resp, "del_link")
}

// ── Utility ────────────────────────────────────────────────────────────────

pub fn enable_ip_forward() -> Result<()> {
    let val = "1\n".as_bytes().to_vec();
    tokio::task::block_in_place(|| {
        std::fs::write("/proc/sys/net/ipv4/ip_forward", &val)
            .context("Failed to enable ip_forward")
    })
}

/// Apply system hardening recommended by the plan:
/// - rp_filter = 1 (strict reverse-path filtering, prevents IP spoofing between VNets)
/// - arp_announce = 2 (always use best local address for ARP, prevents cross-VNet ARP leaks)
pub fn harden_sysctl() -> Result<()> {
    for param in &[
        "net/ipv4/conf/all/rp_filter",
        "net/ipv4/conf/default/rp_filter",
    ] {
        let path = format!("/proc/sys/{}", param);
        if let Err(e) = std::fs::write(&path, "1\n") {
            tracing::warn!("Failed to set {}: {}", path, e);
        }
    }
    for param in &[
        "net/ipv4/conf/all/arp_announce",
        "net/ipv4/conf/default/arp_announce",
    ] {
        let path = format!("/proc/sys/{}", param);
        if let Err(e) = std::fs::write(&path, "2\n") {
            tracing::warn!("Failed to set {}: {}", path, e);
        }
    }
    Ok(())
}

pub fn ensure_loopback_up() -> Result<()> {
    // SAFETY: SIOCGIFFLAGS/SIOCSIFFLAGS are standard safe ioctls on loopback.
    // Uses raw libc socket because nix::sys::socket returns OwnedFd which may
    // conflict with Rust 2024 IO safety when the fd is used for ioctl.
    unsafe {
        let fd = nix::libc::socket(nix::libc::AF_INET, nix::libc::SOCK_DGRAM | nix::libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("loopback socket");
        }
        let mut ifr: nix::libc::ifreq = mem::zeroed();
        std::ptr::copy_nonoverlapping(b"lo\0".as_ptr(), ifr.ifr_name.as_mut_ptr() as *mut u8, 3);
        if nix::libc::ioctl(fd, nix::libc::SIOCGIFFLAGS, &mut ifr) == 0 {
            let flags = ifr.ifr_ifru.ifru_flags as i16;
            ifr.ifr_ifru.ifru_flags = flags | (IFF_UP | IFF_RUNNING) as i16;
            if nix::libc::ioctl(fd, nix::libc::SIOCSIFFLAGS, &mut ifr) != 0 {
                nix::libc::close(fd);
                return Err(std::io::Error::last_os_error()).context("loopback set flags");
            }
        }
        nix::libc::close(fd);
    }
    Ok(())
}


