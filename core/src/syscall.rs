//! # Direct Linux Kernel Syscalls via rustix
//!
//! Thin wrappers around raw Linux syscalls using the `rustix` crate.
//!
//! ## Available Syscalls
//!
//! - `mount` / `umount2` — filesystem operations
//! - `chroot` / `chdir` — filesystem isolation
//! - `unshare` / `setns` — namespace operations
//! - `sethostname` — hostname
//! - `fork` — process creation (via inline asm)
//! - `kill` — signal sending
//! - `pipe2` / `dup2` — file descriptor operations
//! - `open` / `write` / `read` — file I/O
//! - `flock` — file locking
//! - `write_uid_map` / `write_gid_map` — user namespace mapping

use rustix::fd::{AsFd, OwnedFd};
use rustix::io::Errno;

// ── Filesystem ────────────────────────────────────────────────────────────

/// Mount a filesystem.
pub fn mount(
    source: Option<&str>,
    target: &str,
    fstype: Option<&str>,
    flags: rustix::mount::MountFlags,
    data: Option<&str>,
) -> Result<(), Errno> {
    let tgt = std::ffi::CString::new(target).unwrap();
    let src = source.map(|s| std::ffi::CString::new(s).unwrap());
    let fs = fstype.map(|s| std::ffi::CString::new(s).unwrap());
    let dat = data.map(|s| std::ffi::CString::new(s).unwrap());

    let empty = std::ffi::CStr::from_bytes_with_nul(b"\0").unwrap();
    let src_ref = src.as_ref().map(|s| s.as_c_str()).unwrap_or(empty);
    let fs_ref = fs.as_ref().map(|s| s.as_c_str()).unwrap_or(empty);
    let dat_ref = dat.as_ref().map(|s| s.as_c_str());

    rustix::mount::mount(src_ref, &tgt, fs_ref, flags, dat_ref)
}

/// Unmount a filesystem.
pub fn umount2(target: &str, flags: rustix::mount::UnmountFlags) -> Result<(), Errno> {
    let tgt = std::ffi::CString::new(target).unwrap();
    rustix::mount::unmount(&tgt, flags)
}

/// Change root directory.
pub fn chroot(path: &str) -> Result<(), Errno> {
    rustix::process::chroot(path)
}

/// Change working directory.
pub fn chdir(path: &str) -> Result<(), Errno> {
    rustix::process::chdir(path)
}

// ── Namespaces ────────────────────────────────────────────────────────────

/// Unshare namespaces.
pub fn unshare(flags: rustix::thread::UnshareFlags) -> Result<(), Errno> {
    // SAFETY: unshare is safe to call, but rustix wraps it as unsafe
    unsafe { rustix::thread::unshare_unsafe(flags) }
}

/// Set hostname.
pub fn sethostname(name: &str) -> Result<(), Errno> {
    rustix::system::sethostname(name.as_bytes())
}

/// Join a namespace.
pub fn setns(fd: &OwnedFd, nstype: rustix::thread::LinkNameSpaceType) -> Result<(), Errno> {
    rustix::thread::move_into_link_name_space(fd.as_fd(), Some(nstype))
}

// ── Process Management ────────────────────────────────────────────────────

/// Fork a process via inline assembly.
/// Returns child PID to parent, 0 to child.
#[cfg(target_arch = "x86_64")]
pub fn fork() -> Result<u32, Errno> {
    let pid: i64;
    unsafe {
        std::arch::asm!(
            "syscall",
            inlateout("rax") 57_i64 => pid,
            lateout("rcx") _,
            lateout("r11") _,
        );
    }
    if pid >= 0 { Ok(pid as u32) }
    else { Err(Errno::from_raw_os_error(-pid as i32)) }
}

#[cfg(target_arch = "aarch64")]
pub fn fork() -> Result<u32, Errno> {
    let pid: i64;
    unsafe {
        std::arch::asm!(
            "svc #0",
            inlateout("x8") 220_i64 => pid,
            lateout("x1") _, lateout("x2") _, lateout("x3") _,
            lateout("x4") _, lateout("x5") _,
        );
    }
    if pid >= 0 { Ok(pid as u32) }
    else { Err(Errno::from_raw_os_error(-pid as i32)) }
}

/// Send a signal to a process.
pub fn kill(pid: i32, sig: rustix::process::Signal) -> Result<(), Errno> {
    let raw_pid = rustix::process::Pid::from_raw(pid)
        .ok_or(Errno::SRCH)?;
    rustix::process::kill_process(raw_pid, sig)
}

// ── File Descriptors ──────────────────────────────────────────────────────

/// Create a pipe.
pub fn pipe2(flags: rustix::pipe::PipeFlags) -> Result<(OwnedFd, OwnedFd), Errno> {
    rustix::pipe::pipe_with(flags)
}

/// Duplicate file descriptor.
pub fn dup2(old: &OwnedFd, new: &mut OwnedFd) -> Result<(), Errno> {
    rustix::io::dup2(old, new)
}

/// Write data to a file descriptor.
pub fn write(fd: &OwnedFd, data: &[u8]) -> Result<usize, Errno> {
    rustix::io::write(fd, data)
}

/// Read data from a file descriptor.
pub fn read(fd: &OwnedFd, buf: &mut [u8]) -> Result<usize, Errno> {
    rustix::io::read(fd, buf)
}

/// Set close-on-exec flag.
pub fn set_cloexec<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)
}

/// Open a file.
pub fn open(path: &str, flags: rustix::fs::OFlags, mode: rustix::fs::Mode) -> Result<OwnedFd, Errno> {
    let c = std::ffi::CString::new(path).unwrap();
    rustix::fs::open(&c, flags, mode)
}

// ── File Locking ──────────────────────────────────────────────────────────

/// Acquire an exclusive file lock (non-blocking).
pub fn flock_exclusive<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::fs::flock(fd, rustix::fs::FlockOperation::NonBlockingLockExclusive)
}

/// Release a file lock.
pub fn flock_unlock<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::fs::flock(fd, rustix::fs::FlockOperation::Unlock)
}

// ── User Namespace Mapping ────────────────────────────────────────────────

/// Write UID map for a child process.
pub fn write_uid_map(pid: i32, map: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/uid_map", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    write(&fd, map.as_bytes())?;
    drop(fd);
    Ok(())
}

/// Write GID map for a child process.
pub fn write_gid_map(pid: i32, map: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/gid_map", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    write(&fd, map.as_bytes())?;
    drop(fd);
    Ok(())
}

/// Write to /proc/<pid>/setgroups.
pub fn write_setgroups(pid: i32, value: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/setgroups", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    write(&fd, value.as_bytes())?;
    drop(fd);
    Ok(())
}
