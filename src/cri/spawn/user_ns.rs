//! User-namespace container spawn (non-root z8s).

use anyhow::{Context, Result};
use tracing::{error, warn};

use super::context::ContainerSpawnCtx;
use super::{StdPipes, create_std_pipes};
use crate::cri::rootfs;
use crate::cri::runtime::{ProcessSupervisor, RunningContainer};

impl ProcessSupervisor {
    pub(crate) async fn spawn_userns_container(
        &self,
        ctx: ContainerSpawnCtx<'_>,
    ) -> Result<RunningContainer> {
        let ContainerSpawnCtx {
            entrypoint,
            cmd_args,
            env_vars,
            rootfs_path,
            container_id,
            pod_uid,
            image,
            container_name,
            volumes,
            run_as_user,
            run_as_group,
            isolate_net,
            privileged,
            extra_caps,
            working_dir,
            probes,
            subnet,
        } = ctx;
        let pipes = create_std_pipes()?;
        let StdPipes {
            stdout_r,
            stdout_w,
            stderr_r,
            stderr_w,
            sync_r,
            sync_w,
            ack_r,
            ack_w,
        } = pipes;

        let rootfs_owned = rootfs_path.to_string();
        let entrypoint_owned = entrypoint.to_string();
        let args_owned: Vec<String> = cmd_args.to_vec();
        let env_owned = Self::merge_env(env_vars, &rootfs_owned);

        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent { child }) => {
                drop(stdout_w);
                drop(stderr_w);
                drop(sync_w);
                drop(ack_r);

                let child_pid = child.as_raw();

                let mut sync_buf = [0u8; 1];
                let n = nix::unistd::read(&sync_r, &mut sync_buf)
                    .context("Failed to read sync from child")?;
                if n == 0 || sync_buf[0] != b'S' {
                    anyhow::bail!("Child process died before completing namespace setup");
                }
                drop(sync_r);

                rootfs::write_userns_maps(child_pid, run_as_user, run_as_group)?;

                let mut pod_ip: Option<std::net::Ipv4Addr> = None;
                let mut host_veth_ifindex: Option<u32> = None;

                if isolate_net {
                    let pid = child_pid as u32;
                    if let Ok((ip, host_idx, peer_idx)) = self.netmux.attach_pod(pod_uid, Some(pid), subnet.as_deref()) {
                        if let Err(e) = self.netmux.configure_pod_netns(pod_uid, &ip, pid, peer_idx) {
                            warn!("NetMux configure_pod_netns failed: {:#}", e);
                        }
                        pod_ip = Some(ip);
                        host_veth_ifindex = Some(host_idx);
                    }
                }

                nix::unistd::write(&ack_w, b"A").ok();
                drop(ack_w);

                return self.parent_post_fork(
                    child_pid as u32,
                    &container_id,
                    &container_name,
                    image,
                    rootfs_path,
                    env_owned,
                    pod_uid,
                    isolate_net,
                    pod_ip,
                    host_veth_ifindex,
                    run_as_user,
                    run_as_group,
                    stdout_r,
                    stderr_r,
                    &probes,
                );
            }
            Ok(nix::unistd::ForkResult::Child) => {
                drop(stdout_r);
                drop(stderr_r);
                drop(sync_r);
                drop(ack_w);

                let _ = nix::unistd::setsid();

                let pod_hostname = container_id
                    .rsplit_once('-')
                    .map_or(container_id, |(pod, _)| pod);
                let isolation = match rootfs::child_enter_ns_fork(
                    &rootfs_owned,
                    sync_w,
                    ack_r,
                    &volumes,
                    isolate_net,
                    pod_hostname,
                ) {
                    Ok(i) => i,
                    Err(e) => {
                        error!("z8s: namespace setup failed: {:#}", e);
                        unsafe {
                            nix::libc::_exit(1);
                        }
                    }
                };

                nix::unistd::dup2_stdout(&stdout_w).ok();
                nix::unistd::dup2_stderr(&stderr_w).ok();
                drop(stdout_w);
                drop(stderr_w);

                // Open /dev/null for stdin instead of close(0)
                // close(0) after pivot_root causes SIGABRT in userns because when a forked child
                // starts, glibc tries to open /dev/null for fd 0, but device bind-mounts only
                // existed if we pre-created the destination files
                if let Ok(fd) = nix::fcntl::open(
                    "/dev/null",
                    nix::fcntl::OFlag::O_RDONLY,
                    nix::sys::stat::Mode::empty(),
                ) {
                    let _ = nix::unistd::dup2_stdin(fd);
                }

                crate::cri::spawn::child::child_setup_privileges(
                    run_as_group,
                    run_as_user,
                    &working_dir,
                    privileged,
                    &extra_caps,
                    isolation,
                );

                let (exec_path, prog_args) = crate::cri::spawn::child::argv_for_isolation(
                    &entrypoint_owned,
                    &args_owned,
                    &rootfs_owned,
                    isolation,
                );
                crate::cri::spawn::child::execvpe_container(
                    &exec_path,
                    &prog_args,
                    &env_owned,
                    &rootfs_owned,
                    isolation,
                );
            }
            Err(e) => {
                drop(stdout_r);
                drop(stdout_w);
                drop(stderr_r);
                drop(stderr_w);
                drop(sync_r);
                drop(sync_w);
                drop(ack_r);
                drop(ack_w);
                anyhow::bail!("Failed to fork: {}", e);
            }
        }
    }
}
