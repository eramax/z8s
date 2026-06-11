use anyhow::{Context, Result};
use std::mem;

use super::socket::{IFF_RUNNING, IFF_UP};

pub fn enable_ip_forward() -> Result<()> {
    let val = "1\n".as_bytes().to_vec();
    tokio::task::block_in_place(|| {
        std::fs::write("/proc/sys/net/ipv4/ip_forward", &val).context("Failed to enable ip_forward")
    })
}

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
    // SAFETY: SIOCGIFFLAGS/SIOCSIFFLAGS on loopback.
    unsafe {
        let fd = nix::libc::socket(
            nix::libc::AF_INET,
            nix::libc::SOCK_DGRAM | nix::libc::SOCK_CLOEXEC,
            0,
        );
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
