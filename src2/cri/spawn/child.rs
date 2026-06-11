//! Child-side setup and exec helpers (fork path).

use tracing::{error, warn};

use crate::cri::rootfs;

pub fn raise_nproc_limit() {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    if let Ok((soft, hard)) = getrlimit(Resource::RLIMIT_NPROC) {
        let target: u64 = 65535;
        if soft < target {
            let new_hard = hard.max(target);
            let _ = setrlimit(Resource::RLIMIT_NPROC, target, new_hard);
        }
    }
}

pub fn setup_child_pipes(
        stdout_r: std::os::fd::OwnedFd,
        stderr_r: std::os::fd::OwnedFd,
        stdout_w: std::os::fd::OwnedFd,
        stderr_w: std::os::fd::OwnedFd,
    ) {
        drop(stdout_r);
        drop(stderr_r);
        nix::unistd::dup2_stdout(&stdout_w).ok();
        nix::unistd::dup2_stderr(&stderr_w).ok();
        drop(stdout_w);
        drop(stderr_w);
        if let Ok(fd) = nix::fcntl::open(
            "/dev/null",
            nix::fcntl::OFlag::O_RDONLY,
            nix::sys::stat::Mode::empty(),
        ) {
            let _ = nix::unistd::dup2_stdin(fd);
        }
    }

pub fn child_setup_privileges(
        run_as_group: Option<u32>,
        run_as_user: Option<u32>,
        working_dir: &Option<String>,
        privileged: bool,
        extra_caps: &[String],
        isolation: rootfs::RootfsIsolation,
        skip_landlock: bool,
    ) {
        if let Some(gid) = run_as_group {
            if let Err(e) = nix::unistd::setgid(nix::unistd::Gid::from_raw(gid)) {
                warn!("setgid({}) failed: {}", gid, e);
            }
        }
        if let Some(uid) = run_as_user {
            if let Err(e) = nix::unistd::setuid(nix::unistd::Uid::from_raw(uid)) {
                warn!("setuid({}) failed: {}", uid, e);
            }
        }
        if let Some(wd) = working_dir {
            if let Err(e) = nix::unistd::chdir(std::path::Path::new(wd)) {
                warn!("chdir({}) failed: {}", wd, e);
            }
        }
        raise_nproc_limit();
        rootfs::drop_capabilities(privileged, extra_caps);
        if isolation != rootfs::RootfsIsolation::Degraded && !skip_landlock {
            rootfs::apply_landlock();
        }
    }
pub fn argv_for_isolation(
        entrypoint: &str,
        args: &[String],
        rootfs_host_path: &str,
        isolation: rootfs::RootfsIsolation,
    ) -> (String, Vec<String>) {
        if isolation == rootfs::RootfsIsolation::Degraded {
            rootfs::build_container_argv(entrypoint, args, rootfs_host_path)
        } else {
            rootfs::build_container_argv_in_mount_ns(entrypoint, args, rootfs_host_path)
        }
    }

pub fn execvpe_container(
        exec_path: &str,
        prog_args: &[String],
        env_owned: &[(String, String)],
        rootfs_host_path: &str,
        isolation: rootfs::RootfsIsolation,
    ) -> ! {
        let envp: Vec<std::ffi::CString> = env_owned
            .iter()
            .map(|(k, v)| {
                std::ffi::CString::new(format!("{}={}", k, v))
                    .expect("env keys/values cannot contain null bytes")
            })
            .collect();

        let mut argv: Vec<std::ffi::CString> =
            vec![std::ffi::CString::new(exec_path).expect("exec path cannot contain null bytes")];
        for a in prog_args {
            argv.push(
                std::ffi::CString::new(a.as_str()).expect("arg strings cannot contain null bytes"),
            );
        }

        if isolation == rootfs::RootfsIsolation::Degraded {
            let (loader, args) =
                rootfs::wrap_dynamic_linker(exec_path, prog_args.to_vec(), rootfs_host_path);
            argv = vec![
                std::ffi::CString::new(loader).expect("loader path cannot contain null bytes"),
            ];
            for a in args {
                argv.push(
                    std::ffi::CString::new(a).expect("arg strings cannot contain null bytes"),
                );
            }
        }

        let e = nix::unistd::execvpe(&argv[0], &argv, &envp)
            .expect_err("execvpe returned unexpectedly");
        error!(
            "z8s: execvpe({}) failed: {}",
            argv[0].to_str().unwrap_or("?"),
            e
        );
        unsafe {
            nix::libc::_exit(1);
        }
    }
