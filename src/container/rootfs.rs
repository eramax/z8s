use anyhow::{Context, Result};
use nix::mount::{mount, umount, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::unistd::{chdir, pivot_root};
use std::path::Path;
use tracing::info;

pub struct RootFs;

impl RootFs {
    pub fn setup_namespaces() -> Result<()> {
        unshare(CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS)
            .context("Failed to create namespaces")?;
        Ok(())
    }

    pub fn setup_rootfs(rootfs_path: &str) -> Result<()> {
        let rootfs = Path::new(rootfs_path);
        if !rootfs.exists() {
            anyhow::bail!("Rootfs path does not exist: {}", rootfs_path);
        }

        let proc_path = rootfs.join("proc");
        std::fs::create_dir_all(&proc_path).context("Failed to create /proc in rootfs")?;

        let sys_path = rootfs.join("sys");
        std::fs::create_dir_all(&sys_path).context("Failed to create /sys in rootfs")?;

        let dev_path = rootfs.join("dev");
        std::fs::create_dir_all(&dev_path).context("Failed to create /dev in rootfs")?;

        let tmp_path = rootfs.join("tmp");
        std::fs::create_dir_all(&tmp_path).context("Failed to create /tmp in rootfs")?;

        let etc_path = rootfs.join("etc");
        std::fs::create_dir_all(&etc_path).context("Failed to create /etc in rootfs")?;

        let resolv_conf = etc_path.join("resolv.conf");
        if !resolv_conf.exists() {
            std::fs::write(&resolv_conf, "nameserver 1.1.1.1\nnameserver 8.8.8.8\n")
                .context("Failed to write /etc/resolv.conf")?;
        }

        let hosts = etc_path.join("hosts");
        if !hosts.exists() {
            std::fs::write(&hosts, "127.0.0.1 localhost\n")
                .context("Failed to write /etc/hosts")?;
        }

        info!("Rootfs prepared at: {}", rootfs_path);
        Ok(())
    }

    pub fn enter_chroot(rootfs_path: &str) -> Result<()> {
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

        info!("Successfully entered chroot at /");
        Ok(())
    }

    pub fn setup_mounts() -> Result<()> {
        mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_PRIVATE | MsFlags::MS_REC,
            None::<&str>,
        )
        .context("Failed to set private mount propagation")?;
        Ok(())
    }
}
