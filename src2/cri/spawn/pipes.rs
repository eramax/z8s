//! Stdio and parent/child sync pipes for container fork.

use anyhow::Context;

pub struct StdPipes {
    pub stdout_r: std::os::fd::OwnedFd,
    pub stdout_w: std::os::fd::OwnedFd,
    pub stderr_r: std::os::fd::OwnedFd,
    pub stderr_w: std::os::fd::OwnedFd,
    pub sync_r: std::os::fd::OwnedFd,
    pub sync_w: std::os::fd::OwnedFd,
    pub ack_r: std::os::fd::OwnedFd,
    pub ack_w: std::os::fd::OwnedFd,
}

pub fn create_std_pipes() -> anyhow::Result<StdPipes> {
    let p1 = nix::unistd::pipe().context("stdout pipe")?;
    let p2 = nix::unistd::pipe().context("stderr pipe")?;
    let p3 = nix::unistd::pipe().context("sync pipe")?;
    let p4 = nix::unistd::pipe().context("ack pipe")?;
    Ok(StdPipes {
        stdout_r: p1.0,
        stdout_w: p1.1,
        stderr_r: p2.0,
        stderr_w: p2.1,
        sync_r: p3.0,
        sync_w: p3.1,
        ack_r: p4.0,
        ack_w: p4.1,
    })
}
