//! # Container Exec — kubectl exec Support
//!
//! Provides exec functionality for running commands inside running containers.
//! Supports both PTY (interactive) and pipe (non-interactive) modes.
//!
//! ## Exec Flow
//!
//! 1. Resolve target container by name/pod
//! 2. Open namespace FDs (/proc/<pid>/ns/{user,mnt,net,pid})
//! 3. Build command with container environment and resolved paths
//! 4. Spawn child with `pre_exec` to enter container namespaces
//! 5. Stream stdin/stdout/stderr over WebSocket (k8s exec protocol)

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::fd::FromRawFd;
use tokio::process::Command;

use z8s_core::sys;

use super::rootfs;

// ── Types ──────────────────────────────────────────────────────────────────

/// Namespace file descriptors for a container process.
pub struct ContainerNamespaces {
    pub user: Option<OwnedFd>,
    pub pid: Option<OwnedFd>,
    pub mnt: Option<OwnedFd>,
    pub net: Option<OwnedFd>,
}

/// Parameters for an exec session.
pub struct ExecParams {
    pub command: Vec<String>,
    pub container: Option<String>,
    pub tty: bool,
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
}

// ── Namespace Helpers ─────────────────────────────────────────────────────

/// Open namespace FDs for a container process.
pub fn try_open_namespace_fds(container_pid: u32) -> ContainerNamespaces {
    let open_one = |path: &str| {
        sys::open(
            path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .ok()
    };
    ContainerNamespaces {
        user: open_one(&format!("/proc/{}/ns/user", container_pid)),
        pid: open_one(&format!("/proc/{}/ns/pid", container_pid)),
        mnt: open_one(&format!("/proc/{}/ns/mnt", container_pid)),
        net: open_one(&format!("/proc/{}/ns/net", container_pid)),
    }
}

/// Enter container namespaces.
fn enter_container_namespaces(
    ns: &ContainerNamespaces,
    isolated_net: bool,
    use_mnt_ns: bool,
) -> Result<(), std::io::Error> {
    if let Some(ref user) = ns.user {
        let _ = sys::setns(user, rustix::thread::LinkNameSpaceType::User);
    }
    if use_mnt_ns && let Some(ref mnt) = ns.mnt {
        sys::setns(mnt, rustix::thread::LinkNameSpaceType::Mount)
            .map_err(|e| std::io::Error::other(format!("setns(MOUNT): {e}")))?;
    }
    if isolated_net && let Some(ref net) = ns.net {
        sys::setns(net, rustix::thread::LinkNameSpaceType::Network)
            .map_err(|e| std::io::Error::other(format!("setns(NET): {e}")))?;
    }
    Ok(())
}

// ── Environment ────────────────────────────────────────────────────────────

/// Read environment variables from /proc/<pid>/environ.
fn read_container_environ(pid: u32) -> Vec<(String, String)> {
    let Ok(data) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return Vec::new();
    };
    data.split(|&b| b == 0)
        .filter(|var| !var.is_empty())
        .filter_map(|var| {
            let eq = var.iter().position(|&b| b == b'=')?;
            let key = std::str::from_utf8(&var[..eq]).ok()?;
            let val = std::str::from_utf8(&var[eq + 1..]).ok()?;
            Some((key.to_string(), val.to_string()))
        })
        .collect()
}

/// Merge stored env vars with live container environment.
#[allow(dead_code)]
fn merge_exec_env(stored: &[(String, String)], pid: u32) -> Vec<(String, String)> {
    // Start with stored env, then add container env (container wins)
    let mut result: Vec<(String, String)> = stored.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut seen: HashMap<String, ()> = result.iter().map(|(k, _)| (k.clone(), ())).collect();
    for (k, v) in read_container_environ(pid) {
        if seen.insert(k.clone(), ()).is_none() {
            result.push((k, v));
        }
    }
    result
}

// ── Command Building ──────────────────────────────────────────────────────

/// Build a Command configured for the target container.
pub fn build_command(
    cmd: &str,
    args: &[&str],
    rootfs_pid: Option<(&str, u32)>,
    env_vars: &[(String, String)],
    isolated_net: bool,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) -> Command {
    let apply_env = |c: &mut Command| {
        c.env_clear();
        let mut has_path = false;
        for (k, v) in env_vars {
            c.env(k, v);
            if k == "PATH" {
                has_path = true;
            }
        }
        if !has_path {
            if let Some((_, pid)) = rootfs_pid {
                let path = read_container_environ(pid)
                    .into_iter()
                    .find(|(k, _)| k == "PATH")
                    .map(|(_, v)| v)
                    .unwrap_or_else(|| DEFAULT_PATH.to_string());
                c.env("PATH", path);
            } else {
                c.env("PATH", DEFAULT_PATH);
            }
        }
    };

    if let Some((root, container_pid)) = rootfs_pid {
        let ns_fds = try_open_namespace_fds(container_pid);
        let can_enter_mnt = ns_fds.mnt.is_some();
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (exec_path, prog_args) = if can_enter_mnt {
            rootfs::build_container_argv_in_mount_ns(cmd, &args_owned, root)
        } else {
            rootfs::build_container_argv(cmd, &args_owned, root)
        };
        let binary_in_rootfs = exec_path.contains('/');
        let use_mnt_ns = can_enter_mnt && binary_in_rootfs;
        let (program, prog_args) = if use_mnt_ns {
            (exec_path, prog_args)
        } else {
            rootfs::wrap_dynamic_linker(&exec_path, prog_args, root)
        };

        let mut c = Command::new(&program);
        for a in &prog_args {
            c.arg(a);
        }
        apply_env(&mut c);

        let iso_net = isolated_net;
        let exec_gid = run_as_group;
        let exec_uid = run_as_user;
        let has_pid_ns = ns_fds.pid.is_some();
        unsafe {
            c.pre_exec(move || {
                let enter_mnt = use_mnt_ns || has_pid_ns;
                let _ = enter_container_namespaces(&ns_fds, iso_net, enter_mnt);
                let _ = sys::chdir("/");

                if let Some(ref pid) = ns_fds.pid {
                    let _ = sys::setns(pid, rustix::thread::LinkNameSpaceType::ProcessID);
                    match sys::fork() {
                        Ok(sys::ForkResult::Parent(_)) => {
                            std::process::exit(0);
                        }
                        Ok(sys::ForkResult::Child) => {
                            sys::mount(
                                Some("proc"),
                                "/proc",
                                Some("proc"),
                                rustix::mount::MountFlags::NOSUID
                                    | rustix::mount::MountFlags::NOEXEC
                                    | rustix::mount::MountFlags::NODEV,
                                None,
                            )
                            .ok();
                        }
                        Err(_) => {}
                    }
                }
                if let Some(gid) = exec_gid {
                    sys::setgid(gid).ok();
                }
                if let Some(uid) = exec_uid {
                    sys::setuid(uid).ok();
                }
                Ok(())
            });
        }
        c
    } else {
        let mut c = Command::new(cmd);
        for a in args {
            c.arg(a);
        }
        apply_env(&mut c);
        c
    }
}

/// Set PTY window size via rustix::termios::tcsetwinsize.
pub fn set_winsize(fd: std::os::fd::RawFd, cols: u16, rows: u16) {
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    sys::tcsetwinsize(&owned, rows, cols).ok();
    std::mem::forget(owned); // don't close the fd
}

const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
