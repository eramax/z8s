use anyhow::{Context, Result};
use nix::mount::{mount, umount, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::sys::stat::{makedev, mknod, Mode, SFlag};
use nix::unistd::{chdir, chroot, getgid, getuid, pivot_root, Uid};
use std::os::fd::OwnedFd;
use std::path::Path;
use tracing::info;

pub fn is_root() -> bool {
    Uid::effective().is_root()
}

pub fn prepare_rootfs(rootfs_path: &str) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    if !rootfs.exists() {
        anyhow::bail!("Rootfs path does not exist: {}", rootfs_path);
    }

    for dir in &["proc", "sys", "dev", "dev/pts", "tmp", "etc", "run", "dev/shm"] {
        std::fs::create_dir_all(rootfs.join(dir))
            .with_context(|| format!("Failed to create /{} in rootfs", dir))?;
    }

    let dev_nodes: &[&str] = &[
        "null", "zero", "full", "random", "urandom", "tty", "console", "ptmx",
    ];
    for name in dev_nodes {
        let path = rootfs.join("dev").join(name);
        let _ = std::fs::write(&path, []);
    }

    let resolv_conf = rootfs.join("etc/resolv.conf");
    if !resolv_conf.exists() {
        let host_resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        let content = if host_resolv.trim().is_empty()
            || host_resolv.contains("127.0.0.53")
            || host_resolv.contains("systemd-resolved")
        {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        } else {
            host_resolv
        };
        std::fs::write(&resolv_conf, content)
            .context("Failed to write /etc/resolv.conf")?;
    }

    let hosts = rootfs.join("etc/hosts");
    if !hosts.exists() {
        std::fs::write(&hosts, "127.0.0.1 localhost\n")
            .context("Failed to write /etc/hosts")?;
    }

    info!("Rootfs prepared at: {}", rootfs_path);
    Ok(())
}

pub fn write_userns_maps(child_pid: i32) -> Result<()> {
    let uid = getuid().as_raw();
    let gid = getgid().as_raw();

    let setgroups = format!("/proc/{}/setgroups", child_pid);
    std::fs::write(&setgroups, "deny")
        .with_context(|| format!("Failed to write setgroups for pid {}", child_pid))?;

    let uid_map = format!("/proc/{}/uid_map", child_pid);
    std::fs::write(&uid_map, format!("0 {} 1\n", uid))
        .with_context(|| format!("Failed to write uid_map for pid {}", child_pid))?;

    let gid_map = format!("/proc/{}/gid_map", child_pid);
    std::fs::write(&gid_map, format!("0 {} 1\n", gid))
        .with_context(|| format!("Failed to write gid_map for pid {}", child_pid))?;

    info!("Wrote userns maps for child pid {} (uid={} gid={})", child_pid, uid, gid);
    Ok(())
}

pub fn child_enter_ns_fork(rootfs_path: &str, sync_w: OwnedFd, ack_r: OwnedFd) -> Result<()> {
    let flags = CloneFlags::CLONE_NEWUSER
        | CloneFlags::CLONE_NEWNS
        | CloneFlags::CLONE_NEWUTS;
    unshare(flags)
        .context("Failed to unshare user/mount/pid/uts namespaces")?;

    nix::unistd::write(&sync_w, b"S")
        .context("child: failed to write sync byte")?;

    let mut ack = [0u8; 1];
    let n = nix::unistd::read(&ack_r, &mut ack)
        .context("child: failed to read ack")?;
    if n == 0 {
        anyhow::bail!("child: ack pipe closed before receiving ack");
    }
    if ack[0] != b'A' {
        anyhow::bail!("child: invalid ack byte: {}", ack[0]);
    }

    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("Failed to set private mount propagation")?;

    enter_rootfs(rootfs_path, false)
}

pub fn child_enter_ns_root(rootfs_path: &str) -> Result<()> {
    unshare(
        CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWUTS,
    )
    .context("Failed to unshare mount/pid/uts")?;

    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("Failed to set private mount propagation")?;

    chroot(rootfs_path).context("Failed to chroot")?;
    chdir("/").context("Failed to chdir to /")?;

    mount_filesystems_root()
}

fn enter_rootfs(rootfs_path: &str, is_root: bool) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    if !rootfs.exists() {
        anyhow::bail!("Rootfs does not exist: {}", rootfs_path);
    }

    mount(
        Some(rootfs),
        rootfs,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("Failed to bind mount rootfs")?;

    // Bind-mount /proc and /sys from host into rootfs before pivot_root
    if !is_root && !nix::unistd::access(Path::new("/proc"), nix::unistd::AccessFlags::R_OK).is_err() {
        let proc_dst = rootfs.join("proc");
        std::fs::create_dir_all(&proc_dst).ok();
        mount(
            Some(Path::new("/proc")),
            &proc_dst,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )
        .context("Failed to bind-mount /proc")?;
        // Remount readonly for safety
        mount(
            Some(Path::new("/proc")),
            &proc_dst,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_RDONLY,
            None::<&str>,
        )
        .ok();
    }

    if !is_root && !nix::unistd::access(Path::new("/sys"), nix::unistd::AccessFlags::R_OK).is_err() {
        let sys_dst = rootfs.join("sys");
        std::fs::create_dir_all(&sys_dst).ok();
        mount(
            Some(Path::new("/sys")),
            &sys_dst,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC,
            None::<&str>,
        )
        .context("Failed to bind-mount /sys")?;
        mount(
            Some(Path::new("/sys")),
            &sys_dst,
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REC | MsFlags::MS_RDONLY,
            None::<&str>,
        )
        .ok();
    }

    // Bind-mount device nodes from host into rootfs/dev (mknod is blocked in userns)
    if !is_root {
        let dev = rootfs.join("dev");
        std::fs::create_dir_all(&dev).ok();
        let dev_nodes: &[&str] = &[
            "null", "zero", "full", "random", "urandom", "tty", "console", "ptmx",
        ];
        for name in dev_nodes {
            let src = Path::new("/dev").join(name);
            let dst = dev.join(name);
            if nix::unistd::access(&src, nix::unistd::AccessFlags::R_OK).is_ok() {
                std::fs::create_dir_all(dst.parent().unwrap()).ok();
                let _ = mount(
                    Some(&src),
                    &dst,
                    None::<&str>,
                    MsFlags::MS_BIND,
                    None::<&str>,
                );
            }
        }
        let _ = std::os::unix::fs::symlink("/proc/self/fd", dev.join("fd"));
        let _ = std::os::unix::fs::symlink("/proc/self/fd/0", dev.join("stdin"));
        let _ = std::os::unix::fs::symlink("/proc/self/fd/1", dev.join("stdout"));
        let _ = std::os::unix::fs::symlink("/proc/self/fd/2", dev.join("stderr"));
    }

    let old_root = rootfs.join(".z8s_old_root");
    std::fs::create_dir_all(&old_root)?;

    pivot_root(rootfs, &old_root).context("Failed to pivot_root")?;
    chdir("/").context("Failed to chdir to /")?;

    let _ = mount(
        None::<&str>,
        "/.z8s_old_root",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    );
    let _ = umount("/.z8s_old_root");
    let _ = std::fs::remove_dir("/.z8s_old_root");

    mount_filesystems(is_root)
}

fn mount_filesystems(is_root: bool) -> Result<()> {
    if is_root {
        mount(
            Some("proc"),
            "/proc",
            Some("proc"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            None::<&str>,
        )
        .context("Failed to mount /proc")?;

        mount(
            Some("sysfs"),
            "/sys",
            Some("sysfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
            None::<&str>,
        )
        .context("Failed to mount /sys")?;

        mount(
            Some("devtmpfs"),
            "/dev",
            Some("devtmpfs"),
            MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None::<&str>,
        )
        .context("Failed to mount /dev")?;
    } else {
        // In user namespace: /proc and /sys already bind-mounted before pivot_root.
        // /dev device nodes were pre-created in prepare_rootfs (outside userns).
        // No tmpfs mount on /dev — mknod is blocked in user namespaces on this kernel.
    }

    mount(
        Some("tmpfs"),
        "/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /tmp")?;

    mount(
        Some("tmpfs"),
        "/run",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /run")?;

    mount(
        Some("devpts"),
        "/dev/pts",
        Some("devpts"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
        None::<&str>,
    )
    .context("Failed to mount /dev/pts")?;

    let shm = Path::new("/dev/shm");
    if !shm.exists() {
        std::fs::create_dir_all(shm).ok();
    }
    mount(
        Some("tmpfs"),
        "/dev/shm",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
        None::<&str>,
    )
    .context("Failed to mount /dev/shm")?;

    info!("Container filesystem mounted");
    Ok(())
}

fn mount_filesystems_root() -> Result<()> {
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /proc")?;

    mount(
        Some("sysfs"),
        "/sys",
        Some("sysfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /sys")?;

    mount(
        Some("tmpfs"),
        "/tmp",
        Some("tmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /tmp")?;

    mount(
        Some("devtmpfs"),
        "/dev",
        Some("devtmpfs"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        None::<&str>,
    )
    .context("Failed to mount /dev")?;

    mount(
        Some("devpts"),
        "/dev/pts",
        Some("devpts"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
        None::<&str>,
    )
    .context("Failed to mount /dev/pts")?;

    info!("Container filesystem mounted (root mode)");
    Ok(())
}

pub fn setup_exec_mounts(rootfs: &str) -> Result<()> {
    let root_path = Path::new(rootfs);

    let proc_path = root_path.join("proc");
    let _ = std::fs::create_dir_all(&proc_path);
    let _ = mount(
        Some("proc"),
        &proc_path,
        Some("proc"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
        None::<&str>,
    );

    let pts = root_path.join("dev/pts");
    let _ = std::fs::create_dir_all(&pts);
    let _ = mount(
        Some("devpts"),
        &pts,
        Some("devpts"),
        MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
        None::<&str>,
    );

    let ptmx = root_path.join("dev/ptmx");
    if !ptmx.exists() {
        let _ = mknod(
            &ptmx,
            SFlag::S_IFCHR,
            Mode::S_IRWXU,
            makedev(5, 2),
        ).ok();
    }

    let etc_path = root_path.join("etc");
    let _ = std::fs::create_dir_all(&etc_path);
    let resolv = etc_path.join("resolv.conf");
    let existing = std::fs::read_to_string(&resolv).unwrap_or_default();
    if existing.trim().is_empty()
        || existing.contains("127.0.0.53")
        || existing.contains("systemd-resolved")
    {
        let host_resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_else(|_| {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        });
        let content = if host_resolv.contains("127.0.0.53")
            || host_resolv.contains("systemd-resolved")
        {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        } else {
            host_resolv
        };
        let _ = std::fs::write(&resolv, content);
    }

    Ok(())
}
