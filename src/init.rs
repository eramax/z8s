use anyhow::Result;
use nix::sys::prctl;
use nix::sys::signal::Signal;
use nix::sys::signalfd::SignalFd;
use nix::unistd::Pid;
use std::os::fd::AsRawFd;
use tokio::io::unix::AsyncFd;
use tracing::{error, info};

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
        mask.thread_block()?;

        let sigfd = SignalFd::new(&mask)?;
        Ok(Self { signalfd: sigfd })
    }

    pub async fn run(&self, shutdown: &tokio::sync::watch::Sender<bool>) -> Result<()> {
        let async_fd = AsyncFd::new(self.signalfd.as_raw_fd())?;
        loop {
            let _ = async_fd.readable().await?;
            loop {
                match self.signalfd.read_signal() {
                    Ok(Some(siginfo)) => {
                        let signo = siginfo.ssi_signo as i32;
                        if signo == Signal::SIGTERM as i32
                            || signo == Signal::SIGINT as i32
                        {
                            info!("Received shutdown signal, initiating graceful shutdown");
                            let _ = shutdown.send(true);
                            let _ = nix::sys::signal::kill(
                                Pid::from_raw(-1),
                                Signal::SIGTERM,
                            );
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
