//! Root-namespace (double-fork + PID ns) container spawn.

use anyhow::{Context, Result};
use tracing::{error, warn};

use super::context::ContainerSpawnCtx;
use super::pipeline::SpawnState;
use super::{StdPipes, create_std_pipes};
use crate::cri::rootfs;
use crate::cri::runtime::{ProcessSupervisor, RunningContainer};

impl ProcessSupervisor {
    pub(crate) async fn spawn_root_ns_container(
        &self,
        state: SpawnState<'_>,
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
            is_native,
            extra_caps,
            working_dir,
            probes,
            subnet,
        } = state.ctx;
        let env_owned = state.merged_env.unwrap_or_else(|| Self::merge_env(env_vars, rootfs_path));
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
        let (gc_pid_r, gc_pid_w) =
            nix::unistd::pipe().context("Failed to create grandchild PID pipe")?;
        let rootfs_owned = rootfs_path.to_string();
        let entrypoint_owned = entrypoint.to_string();
        let args_owned = cmd_args.to_vec();

        // Fork #1: create intermediate child that will unshare namespaces (including PID)
        // and then fork #2 to place the grandchild (actual container) in the new PID ns.
        match unsafe { nix::unistd::fork() } {
            Ok(nix::unistd::ForkResult::Parent {
                child: _intermediate,
            }) => {
                // Parent: close write ends and unused fds
                drop(stdout_w);
                drop(stderr_w);
                drop(sync_w);
                drop(ack_r);
                drop(gc_pid_w);

                // Read grandchild PID from pipe (written by intermediate child)
                let mut buf = [0u8; 4];
                let n = nix::unistd::read(&gc_pid_r, &mut buf)
                    .context("Failed to read grandchild PID")?;
                if n != 4 {
                    anyhow::bail!("Incomplete grandchild PID: got {} bytes", n);
                }
                let pid = u32::from_ne_bytes(buf);
                drop(gc_pid_r);

                let (pod_ip, host_veth_ifindex) = self.handle_veth_netns(
                    pod_uid, pid, isolate_net, &sync_r, &ack_w, subnet.as_deref(),
                );
                drop(sync_r);
                drop(ack_w);

                return self.parent_post_fork(
                    pid,
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
                // ── Intermediate child ──────────────────────────────────────────
                drop(gc_pid_r);

                let _ = nix::unistd::setsid();
                let pod_hostname = container_id
                    .rsplit_once('-')
                    .map_or(container_id, |(pod, _)| pod);

                // Phase 1: unshare namespaces including CLONE_NEWPID.
                // This puts future children in a new PID namespace.
                if let Err(e) = rootfs::unshare_container_ns(isolate_net, pod_hostname, true) {
                    error!("z8s: namespace setup failed: {}", e);
                    unsafe {
                        nix::libc::_exit(1);
                    }
                }

                // Fork #2 BEFORE closing fds — grandchild inherits all open fds.
                match unsafe { nix::unistd::fork() } {
                    Ok(nix::unistd::ForkResult::Parent { child: gc }) => {
                        // Intermediate child (after fork #2):
                        // Grandchild inherited all fds — close everything except gc_pid_w.
                        drop(stdout_r);
                        drop(stderr_r);
                        drop(sync_r);
                        drop(ack_w);
                        drop(stdout_w);
                        drop(stderr_w);
                        drop(sync_w);
                        drop(ack_r);

                        // Write grandchild PID to parent, then exit immediately.
                        // The grandchild is reparented to z8s (PR_SET_CHILD_SUBREAPER)
                        // and ProcessTracker::reap_zombies catches it directly.
                        let gc_pid = gc.as_raw() as u32;
                        let _ = nix::unistd::write(&gc_pid_w, &gc_pid.to_ne_bytes());
                        drop(gc_pid_w);

                        unsafe {
                            nix::libc::_exit(0);
                        }
                    }
                    Ok(nix::unistd::ForkResult::Child) => {
                        // ── Grandchild (PID 1 in new PID ns) ───────────────────
                        drop(gc_pid_w);
                        drop(sync_r);
                        drop(ack_w);

                        // Redirect container stdout/stderr to pipe
                        crate::cri::spawn::child::setup_child_pipes(stdout_r, stderr_r, stdout_w, stderr_w);

                        // Phase 2: set up rootfs isolation (pivot_root/chroot + mount)
                        let isolation =
                            match rootfs::setup_container_rootfs(&rootfs_owned, &volumes) {
                                Ok(i) => i,
                                Err(e) => {
                                    error!("z8s: rootfs setup failed: {}", e);
                                    unsafe {
                                        nix::libc::_exit(1);
                                    }
                                }
                            };

                        // Sync with parent for veth setup (if isolate_net)
                        if isolate_net {
                            nix::unistd::write(&sync_w, b"S").ok();
                            let mut ack = [0u8; 1];
                            let _ = nix::unistd::read(&ack_r, &mut ack);
                        }
                        drop(sync_w);
                        drop(ack_r);

                        crate::cri::spawn::child::child_setup_privileges(
                            run_as_group,
                            run_as_user,
                            &working_dir,
                            privileged,
                            &extra_caps,
                            isolation,
                            is_native,
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
                        error!("z8s: second fork failed: {}", e);
                        unsafe {
                            nix::libc::_exit(1);
                        }
                    }
                }
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
                drop(gc_pid_r);
                drop(gc_pid_w);
                anyhow::bail!("Failed to fork: {}", e);
            }
        }
    }
}
