use anyhow::{Context, Result};
use std::mem;
use std::os::fd::AsRawFd;

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
pub const RTN_LOCAL: u8 = 2;
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
    // SAFETY: libc sendmsg FFI on valid fd.
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
    let mut buf = vec![0u8; 8192];
    // SAFETY: libc recvmsg FFI on valid fd.
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
    // SAFETY: T is Copy with known size.
    let data_bytes =
        unsafe { std::slice::from_raw_parts(data as *const T as *const u8, mem::size_of::<T>()) };
    buf[4..].copy_from_slice(data_bytes);
    buf
}

pub fn nlattr_bytes(nla_type: u16, data: &[u8]) -> Vec<u8> {
    let padded = (data.len() + 3) & !3;
    let size = padded + 4;
    let mut buf = vec![0u8; size];
    buf[0..2].copy_from_slice(&(size as u16).to_ne_bytes());
    buf[2..4].copy_from_slice(&nla_type.to_ne_bytes());
    buf[4..4 + data.len()].copy_from_slice(data);
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

pub(crate) fn check_nl_response(resp: &[u8], context: &str) -> Result<()> {
    if resp.len() >= 16 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type == NLMSG_ERROR && resp.len() >= 20 {
            let err_code = i32::from_ne_bytes([resp[16], resp[17], resp[18], resp[19]]);
            if err_code != 0 {
                return Err(anyhow::anyhow!(
                    "{}: netlink error {} ({})",
                    context,
                    err_code,
                    nix::errno::Errno::from_raw(err_code as i32)
                ));
            }
        }
    }
    Ok(())
}
