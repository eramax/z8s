//! # Syscall Wrappers via rustix
//!
//! Thin wrappers around rustix 1.1.4 — **zero inline asm, zero CString**.
//!
//! All path operations use rustix's `path::Arg` trait which accepts `&str`
//! directly. CString is only used in `execve` (POSIX requirement).

use rustix::fd::{AsFd, OwnedFd};
use rustix::io::Errno;

/// Result of a fork operation.
pub enum ForkResult {
    /// Parent process — contains child PID.
    Parent(u32),
    /// Child process.
    Child,
}

// ── Filesystem ────────────────────────────────────────────────────────────

/// Mount a filesystem. Data is only used for special filesystems (overlay, etc).
pub fn mount(
    source: Option<&str>,
    target: &str,
    fstype: Option<&str>,
    flags: rustix::mount::MountFlags,
    data: Option<&str>,
) -> Result<(), Errno> {
    let source_str = source.unwrap_or("");
    let fstype_str = fstype.unwrap_or("");
    match data {
        None => rustix::mount::mount(source_str, target, fstype_str, flags, None::<&std::ffi::CStr>),
        Some(d) => {
            let cdata = std::ffi::CString::new(d).expect("mount data contains null byte");
            rustix::mount::mount(source_str, target, fstype_str, flags, Some(cdata.as_c_str()))
        }
    }
}

pub fn umount2(target: &str, flags: rustix::mount::UnmountFlags) -> Result<(), Errno> {
    rustix::mount::unmount(target, flags)
}

pub fn chroot(path: &str) -> Result<(), Errno> {
    rustix::process::chroot(path)
}

pub fn chdir(path: &str) -> Result<(), Errno> {
    rustix::process::chdir(path)
}

pub fn pivot_root(new_root: &str, put_old: &str) -> Result<(), Errno> {
    rustix::process::pivot_root(new_root, put_old)
}

pub fn loopback_up() -> Result<(), Errno> {
    let _ = std::process::Command::new("ip").args(["link", "set", "lo", "up"]).output();
    Ok(())
}

pub fn mknod(path: &std::path::Path, major: u64, minor: u64) -> Result<(), Errno> {
    let dev = rustix::fs::makedev(major as u32, minor as u32);
    let mode = rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR
        | rustix::fs::Mode::RGRP | rustix::fs::Mode::WGRP
        | rustix::fs::Mode::ROTH | rustix::fs::Mode::WOTH;
    rustix::fs::mknodat(
        rustix::fs::CWD,
        path,
        rustix::fs::FileType::CharacterDevice,
        mode,
        dev,
    )
}

// ── Namespaces ────────────────────────────────────────────────────────────

pub fn unshare(flags: rustix::thread::UnshareFlags) -> Result<(), Errno> {
    // SAFETY: unshare is safe to call
    unsafe { rustix::thread::unshare_unsafe(flags) }
}

pub fn sethostname(name: &str) -> Result<(), Errno> {
    rustix::system::sethostname(name.as_bytes())
}

pub fn setns(fd: &OwnedFd, nstype: rustix::thread::LinkNameSpaceType) -> Result<(), Errno> {
    rustix::thread::move_into_link_name_space(fd.as_fd(), Some(nstype))
}

// ── Process Management ────────────────────────────────────────────────────

pub fn fork() -> Result<ForkResult, Errno> {
    let fork_result = unsafe { rustix::runtime::kernel_fork() }?;
    match fork_result {
        rustix::runtime::Fork::Child(_) => Ok(ForkResult::Child),
        rustix::runtime::Fork::ParentOf(pid) => Ok(ForkResult::Parent(pid.as_raw_pid() as u32)),
    }
}

pub fn kill(pid: i32, sig: rustix::process::Signal) -> Result<(), Errno> {
    let raw_pid = rustix::process::Pid::from_raw(pid)
        .ok_or(Errno::SRCH)?;
    rustix::process::kill_process(raw_pid, sig)
}

pub fn setsid() -> Result<rustix::process::Pid, Errno> {
    rustix::process::setsid()
}

// ── Identity ──────────────────────────────────────────────────────────────

pub fn setuid(uid: u32) -> Result<(), Errno> {
    rustix::thread::set_thread_uid(rustix::process::Uid::from_raw(uid))
}

pub fn setgid(gid: u32) -> Result<(), Errno> {
    rustix::thread::set_thread_gid(rustix::process::Gid::from_raw(gid))
}

// ── File Descriptors ──────────────────────────────────────────────────────

pub fn pipe2(flags: rustix::pipe::PipeFlags) -> Result<(OwnedFd, OwnedFd), Errno> {
    rustix::pipe::pipe_with(flags)
}

pub fn dup2_stdout<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::stdio::dup2_stdout(fd)
}

pub fn dup2_stderr<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::stdio::dup2_stderr(fd)
}

pub fn dup2_stdin<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::stdio::dup2_stdin(fd)
}

pub fn write_fd(fd: &OwnedFd, data: &[u8]) -> Result<usize, Errno> {
    rustix::io::write(fd, data)
}

pub fn read_fd(fd: &OwnedFd, buf: &mut [u8]) -> Result<usize, Errno> {
    rustix::io::read(fd, buf)
}

pub fn set_cloexec<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC)
}

pub fn open(path: &str, flags: rustix::fs::OFlags, mode: rustix::fs::Mode) -> Result<OwnedFd, Errno> {
    rustix::fs::open(path, flags, mode)
}

/// Execute a program (replaces current process). Does not return on success.
/// CStr is a POSIX requirement — no way around null-terminated strings for execve.
pub fn execve(
    program: &std::ffi::CStr,
    argv: &[*const std::ffi::c_char],
    envp: &[*const std::ffi::c_char],
) -> Errno {
    unsafe {
        rustix::runtime::execve(program, argv.as_ptr().cast(), envp.as_ptr().cast())
    }
}

// ── Terminal ──────────────────────────────────────────────────────────────

pub fn tcsetwinsize<Fd: AsFd>(fd: Fd, rows: u16, cols: u16) -> Result<(), Errno> {
    let ws = rustix::termios::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    rustix::termios::tcsetwinsize(fd, ws)
}

// ── File Locking ──────────────────────────────────────────────────────────

pub fn flock_exclusive<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::fs::flock(fd, rustix::fs::FlockOperation::NonBlockingLockExclusive)
}

pub fn flock_unlock<Fd: AsFd>(fd: Fd) -> Result<(), Errno> {
    rustix::fs::flock(fd, rustix::fs::FlockOperation::Unlock)
}

// ── User Namespace Mapping ────────────────────────────────────────────────

pub fn write_uid_map(pid: i32, map: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/uid_map", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    rustix::io::write(&fd, map.as_bytes())?;
    drop(fd);
    Ok(())
}

pub fn write_gid_map(pid: i32, map: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/gid_map", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    rustix::io::write(&fd, map.as_bytes())?;
    drop(fd);
    Ok(())
}

pub fn write_setgroups(pid: i32, value: &str) -> Result<(), Errno> {
    let path = format!("/proc/{}/setgroups", pid);
    let fd = open(&path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
    rustix::io::write(&fd, value.as_bytes())?;
    drop(fd);
    Ok(())
}

// ── Process Reaping ───────────────────────────────────────────────────────

/// Wait for a child process. Returns (pid, exit_code).
pub fn waitpid(pid: i32) -> Option<(u32, i32)> {
    let target = if pid == -1 { None } else { rustix::process::Pid::from_raw(pid) };
    let opts = rustix::process::WaitOptions::UNTRACED;
    match rustix::process::waitpid(target, opts) {
        Ok(Some((p, status))) => {
            let code = status.exit_status().unwrap_or(-1);
            Some((p.as_raw_pid() as u32, code))
        }
        _ => None,
    }
}
