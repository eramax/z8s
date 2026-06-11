use anyhow::Result;
use nix::sys::prctl;
use nix::sys::signal::Signal;
use nix::sys::signalfd::SignalFd;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;
use std::os::fd::AsRawFd;
use tokio::io::unix::AsyncFd;
use tracing::{error, info, warn};

pub struct InitHandler {
    signalfd: SignalFd,
}

impl InitHandler {
    pub fn new() -> Result<Self> {
        // Become a child subreaper so orphaned descendants (e.g. grandchild from
        // double-fork for PID namespaces) are reparented to us instead of init(1).
        prctl::set_child_subreaper(true).ok();

        let mut mask = nix::sys::signal::SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        mask.add(Signal::SIGHUP);
        mask.add(Signal::SIGUSR1);
        mask.add(Signal::SIGUSR2);
        mask.add(Signal::SIGCHLD); // Reap zombies immediately on signal
        mask.thread_block()?;

        let sigfd = SignalFd::new(&mask)?;
        Ok(Self { signalfd: sigfd })
    }

    /// Reap all available zombie children. Returns list of (pid, exit_code).
    /// Called on SIGCHLD and during graceful shutdown.
    pub fn reap_all() -> Vec<(u32, i32)> {
        let mut reaped = Vec::new();
        loop {
            match waitpid(Pid::from_raw(-1), Some(WaitPidFlag::WNOHANG)) {
                Ok(WaitStatus::Exited(pid, status)) => {
                    info!("Reaped zombie PID {} (exit {})", pid, status);
                    reaped.push((pid.as_raw() as u32, status));
                }
                Ok(WaitStatus::Signaled(pid, sig, _)) => {
                    info!("Reaped zombie PID {} (signal {:?})", pid, sig);
                    reaped.push((pid.as_raw() as u32, -(sig as i32)));
                }
                Ok(WaitStatus::StillAlive) => break, // No more zombies
                Ok(_) => continue, // Stopped/continued — ignore
                Err(nix::errno::Errno::ECHILD) => break, // No children
                Err(e) => {
                    warn!("waitpid error: {}", e);
                    break;
                }
            }
        }
        reaped
    }

    pub async fn run(&self, shutdown: &tokio::sync::watch::Sender<bool>) -> Result<()> {
        let async_fd = AsyncFd::new(self.signalfd.as_raw_fd())?;
        loop {
            let _ = async_fd.readable().await?;
            loop {
                match self.signalfd.read_signal() {
                    Ok(Some(siginfo)) => {
                        let signo = siginfo.ssi_signo as i32;
                        if signo == Signal::SIGCHLD as i32 {
                            // Reap zombies immediately — don't wait for reconciler tick
                            let reaped = Self::reap_all();
                            if !reaped.is_empty() {
                                info!("SIGCHLD: reaped {} zombies", reaped.len());
                            }
                        } else if signo == Signal::SIGTERM as i32 || signo == Signal::SIGINT as i32 {
                            info!("Received shutdown signal, initiating graceful shutdown");
                            // Reap any remaining zombies before shutdown
                            let remaining = Self::reap_all();
                            if !remaining.is_empty() {
                                info!("Shutdown: reaped {} remaining zombies", remaining.len());
                            }
                            let _ = shutdown.send(true);
                            let _ = nix::sys::signal::kill(Pid::from_raw(-1), Signal::SIGTERM);
                            return Ok(());
                        } else if signo == Signal::SIGHUP as i32 {
                            info!("Received SIGHUP, triggering reload");
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        error!("Error reading signal fd: {}", e);
                        break;
                    }
                }
            }
        }
    }
}
