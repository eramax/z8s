//! Publish container ports on unique 127.0.0.1 addresses on the host.
//!
//! Each pod gets its own network namespace (CLONE_NEWNET). Processes listen on
//! container ports inside that namespace; we forward `127.0.0.1:host_port` →
//! `127.0.0.1:container_port` inside the pod netns so multiple pods can all use
//! port 80 without colliding on the host.

use std::collections::HashMap;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use nix::fcntl::OFlag;
use nix::sched::CloneFlags;
use nix::sys::stat::Mode;
use tracing::{info, warn};

static HOST_PORT_COUNTER: AtomicU16 = AtomicU16::new(20000);

#[derive(Debug)]
pub struct PortPublish {
    /// container_port → host_port on 127.0.0.1
    pub map: HashMap<u16, u16>,
    cancel: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl PortPublish {
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }

    /// Publish additional container ports on the same process (e.g. after a Service is applied).
    pub fn append_ports(&mut self, container_pid: u32, container_ports: &[u16]) {
        for &cp in container_ports {
            if self.map.contains_key(&cp) {
                continue;
            }
            let host_port = HOST_PORT_COUNTER.fetch_add(1, Ordering::Relaxed);
            self.map.insert(cp, host_port);
            let c = self.cancel.clone();
            let task = tokio::spawn(async move {
                if let Err(e) = run_forwarder(host_port, container_pid, cp, c).await {
                    warn!(
                        "port forward 127.0.0.1:{} → pid {}:{} stopped: {}",
                        host_port, container_pid, cp, e
                    );
                }
            });
            self.tasks.push(task);
            info!(
                "Published (late) container port {} → 127.0.0.1:{} (pid {})",
                cp, host_port, container_pid
            );
        }
    }
}

impl Drop for PortPublish {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn publish_ports(container_pid: u32, container_ports: &[u16]) -> PortPublish {
    let mut map = HashMap::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::new();

    for &cp in container_ports {
        let host_port = HOST_PORT_COUNTER.fetch_add(1, Ordering::Relaxed);
        map.insert(cp, host_port);
        let c = cancel.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = run_forwarder(host_port, container_pid, cp, c).await {
                warn!(
                    "port forward 127.0.0.1:{} → pid {}:{} stopped: {}",
                    host_port, container_pid, cp, e
                );
            }
        });
        tasks.push(task);
        info!(
            "Published container port {} → 127.0.0.1:{} (pid {})",
            cp, host_port, container_pid
        );
    }

    PortPublish { map, cancel, tasks }
}

async fn run_forwarder(
    host_port: u16,
    container_pid: u32,
    container_port: u16,
    cancel: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", host_port)).await?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let accept = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
        let Ok(Ok((mut client, _))) = accept else {
            continue;
        };
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let backend = match tokio::task::spawn_blocking(move || {
                connect_tcp_in_netns(container_pid, container_port)
            })
            .await
            {
                Ok(Ok(s)) => s,
                _ => return,
            };
            let mut backend = match tokio::net::TcpStream::from_std(backend) {
                Ok(s) => s,
                Err(_) => return,
            };
            tokio::io::copy_bidirectional(&mut client, &mut backend)
                .await
                .ok();
        });
    }
    Ok(())
}

fn connect_tcp_in_netns(container_pid: u32, port: u16) -> std::io::Result<TcpStream> {
    let net_path = format!("/proc/{}/ns/net", container_pid);
    let net_fd = nix::fcntl::open(net_path.as_str(), OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty())
        .map_err(|e| std::io::Error::other(format!("open netns: {e}")))?;

    // setns is per-thread; run connect on a short-lived thread.
    std::thread::spawn(move || -> std::io::Result<TcpStream> {
        nix::sched::setns(&net_fd, CloneFlags::CLONE_NEWNET)
            .map_err(|e| std::io::Error::other(format!("setns net: {e}")))?;
        drop(net_fd);
        // Connect with a short timeout; keep the stream non-blocking for tokio.
        // Do NOT set read/write timeouts — that would break long-lived connections
        // (WebSockets, streaming logs, kubectl exec).
        let stream = TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", port).parse().map_err(std::io::Error::other)?,
            Duration::from_secs(5),
        )?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    })
    .join()
    .map_err(|_| std::io::Error::other("netns connect thread panicked"))?
}

/// Bring up loopback inside a fresh network namespace.
pub fn setup_loopback() {
    unsafe {
        let fd = nix::libc::socket(nix::libc::AF_INET, nix::libc::SOCK_DGRAM | nix::libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return;
        }
        let mut ifr: nix::libc::ifreq = std::mem::zeroed();
        std::ptr::copy_nonoverlapping(b"lo\0".as_ptr(), ifr.ifr_name.as_mut_ptr() as *mut u8, 3);
        if nix::libc::ioctl(fd, nix::libc::SIOCGIFFLAGS, &mut ifr) == 0 {
            let flags = ifr.ifr_ifru.ifru_flags as i16;
            ifr.ifr_ifru.ifru_flags =
                flags | (nix::libc::IFF_UP as i16) | (nix::libc::IFF_RUNNING as i16);
            let _ = nix::libc::ioctl(fd, nix::libc::SIOCSIFFLAGS, &mut ifr);
        }
        nix::libc::close(fd);
    }
}
