//! # Rootfs — Filesystem Isolation for Containers
//!
//! Handles the transition from host filesystem to container filesystem.
//! Supports three isolation levels depending on privilege:
//!
//! - **Pivot** (root) — `pivot_root` into OCI rootfs + fresh proc/sys mounts
//! - **Chroot** (rootless) — `chroot` into OCI rootfs with bind-mounted /proc
//! - **Degraded** — host mount view, no filesystem isolation
//!
//! ## Key Operations
//!
//! - `prepare_rootfs` — create /proc, /dev, /tmp, device nodes, resolv.conf
//! - `setup_container_rootfs` — mount propagation + pivot_root/chroot
//! - `drop_capabilities` — restrict to OCI default capability set
//! - `apply_landlock` — LSM filesystem restriction (kernel 5.13+)
//! - `resolve_exec_path` — find binary inside container rootfs
//! - `build_container_argv` — resolve entrypoint + busybox wrapping

use anyhow::{Context, Result};
use rustix::mount::{MountFlags, MountPropagationFlags, UnmountFlags};
use std::path::Path;
use tracing::{debug, info, warn};

use z8s_core::sys;

use super::spec::ResolvedVolume;

// ── Constants ──────────────────────────────────────────────────────────────

/// Device nodes to create/bind-mount inside the container.
const DEV_NODES: &[(&str, u64, u64)] = &[
    ("null", 1, 3),
    ("zero", 1, 5),
    ("full", 1, 7),
    ("random", 1, 8),
    ("urandom", 1, 9),
    ("tty", 5, 0),
    ("console", 5, 1),
    ("ptmx", 5, 2),
];

// ── Isolation Level ────────────────────────────────────────────────────────

/// How the container process sees its filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootfsIsolation {
    Pivot,
    Chroot,
    Degraded,
}

/// Whether we're running as root (determines isolation strategy).
pub fn is_root() -> bool {
    rustix::process::getuid().is_root()
}

// ── Rootfs Preparation ─────────────────────────────────────────────────────

/// Prepare the rootfs directory: create essential dirs, device nodes, configs.
pub fn prepare_rootfs(rootfs_path: &str) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    anyhow::ensure!(rootfs.exists(), "Rootfs path does not exist: {}", rootfs_path);

    // Create essential directories
    for dir in ["proc", "sys", "dev", "dev/pts", "tmp", "etc", "run", "dev/shm"] {
        std::fs::create_dir_all(rootfs.join(dir))
            .with_context(|| format!("Failed to create /{} in rootfs", dir))?;
    }

    // Create device nodes
    for &(name, major, minor) in DEV_NODES {
        let path = rootfs.join("dev").join(name);
        let _ = std::fs::remove_file(&path);
        sys::mknod(&path, major, minor).ok();
    }

    // Write /etc/resolv.conf
    let content = build_resolv_conf();
    std::fs::write(rootfs.join("etc/resolv.conf"), content)
        .context("Failed to write /etc/resolv.conf")?;

    // Write /etc/hosts
    let hosts = rootfs.join("etc/hosts");
    if !hosts.exists() {
        std::fs::write(&hosts, "127.0.0.1 localhost\n")
            .context("Failed to write /etc/hosts")?;
    }

    info!("Rootfs prepared at: {}", rootfs_path);
    Ok(())
}

// ── Namespace + Rootfs Setup ───────────────────────────────────────────────

/// Phase 1 (intermediate child): unshare namespaces including PID.
pub fn unshare_container_ns(isolate_net: bool, hostname: &str, pid_ns: bool) -> Result<()> {
    let mut flags = rustix::thread::UnshareFlags::NEWNS
        | rustix::thread::UnshareFlags::NEWUTS
        | rustix::thread::UnshareFlags::NEWIPC;
    if isolate_net {
        flags |= rustix::thread::UnshareFlags::NEWNET;
    }
    if pid_ns {
        flags |= rustix::thread::UnshareFlags::NEWPID;
    }
    sys::unshare(flags).context("Failed to unshare namespaces")?;
    sys::sethostname(hostname).context("Failed to set container hostname")?;
    if isolate_net {
        sys::loopback_up().ok();
    }
    Ok(())
}

/// Phase 2 (grandchild): mount rootfs, pivot_root or chroot, mount proc/sys/tmp.
pub fn setup_container_rootfs(
    rootfs_path: &str,
    volumes: &[ResolvedVolume],
) -> Result<RootfsIsolation> {
    // Set slave propagation to prevent mount leaks to host
    rustix::mount::mount_change(
        "/",
        MountPropagationFlags::DOWNSTREAM | MountPropagationFlags::REC,
    )
    .context("Failed to set slave mount propagation")?;

    // Try pivot_root for strongest isolation
    if enter_rootfs(rootfs_path, true, volumes).is_ok() {
        return Ok(RootfsIsolation::Pivot);
    }

    // Fallback: chroot
    warn!("pivot_root failed, falling back to chroot");
    if !volumes.is_empty() {
        warn!("Binding volumes in chroot fallback (may fail with EPERM)");
        bind_mount_volumes(rootfs_path, volumes);
    }
    sys::chroot(rootfs_path).context("Failed to chroot")?;
    sys::chdir("/").context("Failed to chdir to /")?;
    mount_filesystems(true)?;
    sys::mount(
        Some("devtmpfs"),
        "/dev",
        Some("devtmpfs"),
        MountFlags::NOSUID | MountFlags::NODEV,
        None,
    )
    .context("Failed to mount /dev")?;
    info!("Container filesystem mounted (root mode)");
    Ok(RootfsIsolation::Chroot)
}

/// Combined namespace + rootfs setup for single-fork path (backward compat).
pub fn child_enter_ns_root(
    rootfs_path: &str,
    volumes: &[ResolvedVolume],
    isolate_net: bool,
    hostname: &str,
) -> Result<RootfsIsolation> {
    unshare_container_ns(isolate_net, hostname, false)?;
    setup_container_rootfs(rootfs_path, volumes)
}

/// User-namespace path: unshare user+mount+uts+ipc, then chroot.
pub fn child_enter_ns_fork(
    rootfs_path: &str,
    sync_w: rustix::fd::OwnedFd,
    ack_r: rustix::fd::OwnedFd,
    volumes: &[ResolvedVolume],
    isolate_net: bool,
    hostname: &str,
) -> Result<RootfsIsolation> {
    let mut flags = rustix::thread::UnshareFlags::NEWUSER
        | rustix::thread::UnshareFlags::NEWNS
        | rustix::thread::UnshareFlags::NEWUTS
        | rustix::thread::UnshareFlags::NEWIPC;
    if isolate_net {
        flags |= rustix::thread::UnshareFlags::NEWNET;
    }
    sys::unshare(flags).context("Failed to unshare user/mount/uts/ipc")?;
    sys::sethostname(hostname).context("Failed to set hostname")?;
    if isolate_net {
        sys::loopback_up().ok();
    }

    // Signal parent that namespaces are ready
    sys::write_fd(&sync_w, b"S").context("child: failed to write sync byte")?;
    let mut ack = [0u8; 1];
    let n = sys::read_fd(&ack_r, &mut ack).context("child: failed to read ack")?;
    anyhow::ensure!(n == 1 && ack[0] == b'A', "child: invalid ack");

    // MS_SLAVE on / (required in user namespaces where MS_PRIVATE fails)
    rustix::mount::mount_change(
        "/",
        MountPropagationFlags::DOWNSTREAM | MountPropagationFlags::REC,
    )
    .ok();

    // Try chroot (works if rootfs is bind-mounted inside user ns)
    if mount_rootfs_components(rootfs_path, false, volumes).is_ok()
        && sys::chroot(rootfs_path).is_ok()
        && sys::chdir("/").is_ok()
        && mount_filesystems(false).is_ok()
    {
        return Ok(RootfsIsolation::Chroot);
    }

    tracing::error!("z8s: FILESYSTEM ISOLATION UNAVAILABLE — RUNNING DEGRADED!");
    if !volumes.is_empty() {
        bind_mount_volumes_degraded(volumes);
    }
    Ok(RootfsIsolation::Degraded)
}

// ── Capabilities ───────────────────────────────────────────────────────────

/// OCI default capability set — the only caps non-privileged containers keep.
const OCI_DEFAULT_CAPS: &[&str] = &[
    "CAP_CHOWN",
    "CAP_FSETID",
    "CAP_FOWNER",
    "CAP_SETGID",
    "CAP_SETUID",
    "CAP_SETPCAP",
    "CAP_NET_BIND_SERVICE",
    "CAP_SYS_CHROOT",
    "CAP_AUDIT_WRITE",
];

/// Drop excess capabilities from all 5 cap sets.
/// Retains OCI defaults plus any caps listed in `extra_caps`.
pub fn drop_capabilities(privileged: bool, extra_caps: &[String]) {
    if privileged {
        return;
    }
    use caps::{CapSet, Capability};
    use std::collections::HashSet;

    let mut keep: HashSet<Capability> = OCI_DEFAULT_CAPS
        .iter()
        .filter_map(|c| c.parse::<Capability>().ok())
        .collect();

    for name in extra_caps {
        let normalized = if name.to_uppercase().starts_with("CAP_") {
            name.to_uppercase()
        } else {
            format!("CAP_{}", name.to_uppercase())
        };
        if let Ok(cap) = normalized.parse::<Capability>() {
            keep.insert(cap);
        }
    }

    for cap in caps::all() {
        if !keep.contains(&cap) {
            caps::drop(None, CapSet::Bounding, cap).ok();
        }
    }

    let permitted = caps::read(None, CapSet::Permitted).unwrap_or_default();
    let new_caps: HashSet<Capability> = permitted.into_iter().filter(|c| keep.contains(c)).collect();
    caps::set(None, CapSet::Effective, &new_caps).ok();
    caps::set(None, CapSet::Permitted, &new_caps).ok();
    caps::clear(None, CapSet::Inheritable).ok();
    caps::clear(None, CapSet::Ambient).ok();

    debug!("Capabilities restricted to OCI default set (+{} extra)", extra_caps.len());
}

/// Apply Landlock LSM to restrict filesystem access. Best-effort.
pub fn apply_landlock() {
    if let Err(e) = try_apply_landlock() {
        debug!("Landlock not applied: {}", e);
    }
}

fn try_apply_landlock() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use landlock::{
        ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    };
    let abi = ABI::V3;
    let access_fs = AccessFs::from_all(abi);
    Ruleset::default()
        .handle_access(access_fs)?
        .create()?
        .add_rule(PathBeneath::new(PathFd::new("/")?, access_fs))?
        .restrict_self()?;
    debug!("Landlock filesystem restriction applied");
    Ok(())
}

// ── Exec Path Resolution ──────────────────────────────────────────────────

/// Resolve a binary path inside the container rootfs.
pub fn resolve_exec_path(entrypoint: &str, rootfs_path: &str) -> String {
    let root = rootfs_path.trim_end_matches('/');
    if entrypoint.starts_with('/') {
        return format!("{root}{entrypoint}");
    }

    // Try to get actual container PATH from running processes
    let container_path = get_container_path_from_proc(rootfs_path);

    let candidates = container_path
        .iter()
        .flat_map(|p| p.split(':'))
        .filter(|d| !d.is_empty())
        .map(|dir| format!("{root}/{}/{entrypoint}", dir.trim_start_matches('/')))
        .chain(
            ["bin", "usr/bin", "usr/local/bin", "sbin", "usr/sbin", "usr/local/sbin"]
                .iter()
                .map(|dir| format!("{root}/{dir}/{entrypoint}")),
        )
        .chain(postgres_dynamic_paths(root, entrypoint));

    for p in candidates {
        let path = Path::new(&p);
        if path.exists() || path.is_symlink() {
            if let Ok(target) = std::fs::read_link(path) {
                let t = target.to_string_lossy();
                if t.starts_with('/') {
                    let in_root = format!("{root}{t}");
                    if Path::new(&in_root).exists() {
                        return in_root;
                    }
                }
            }
            return p;
        }
    }

    if Path::new(entrypoint).exists() {
        entrypoint.to_string()
    } else if entrypoint.starts_with('/') {
        format!("{root}{entrypoint}")
    } else {
        entrypoint.to_string()
    }
}

/// Build the exec path and argv for a container entrypoint.
pub fn build_container_argv(
    entrypoint: &str,
    args: &[String],
    rootfs_path: &str,
) -> (String, Vec<String>) {
    let exec_path = resolve_exec_path(entrypoint, rootfs_path);
    let argv = busybox_argv(&exec_path, entrypoint, args);
    (exec_path, argv)
}

/// Build exec path and argv after setns into the container mount namespace.
pub fn build_container_argv_in_mount_ns(
    entrypoint: &str,
    args: &[String],
    rootfs_path: &str,
) -> (String, Vec<String>) {
    let (host_exec, argv) = build_container_argv(entrypoint, args, rootfs_path);
    let exec_path = host_path_in_container_root(&host_exec, rootfs_path);
    (exec_path, argv)
}

/// Wrap an ELF binary via its recorded dynamic linker (degraded exec).
pub fn wrap_dynamic_linker(
    exec_path: &str,
    prog_args: Vec<String>,
    rootfs_path: &str,
) -> (String, Vec<String>) {
    let root = rootfs_path.trim_end_matches('/');
    if !exec_path.starts_with(root) {
        return (exec_path.to_string(), prog_args);
    }
    let Some(interp) = read_elf_interpreter(exec_path) else {
        return (exec_path.to_string(), prog_args);
    };
    let loader = if interp.starts_with('/') {
        format!("{root}{interp}")
    } else {
        format!("{root}/{interp}")
    };
    if !Path::new(&loader).exists() {
        return (exec_path.to_string(), prog_args);
    }
    let mut args = vec![exec_path.to_string()];
    args.extend(prog_args);
    (loader, args)
}

/// Map a host path to the path seen inside the container mount namespace.
pub fn host_path_in_container_root(host_path: &str, rootfs_path: &str) -> String {
    let root = rootfs_path.trim_end_matches('/');
    match host_path.strip_prefix(root) {
        Some("") => "/".to_string(),
        Some(rest) if rest.starts_with('/') => rest.to_string(),
        Some(rest) => format!("/{rest}"),
        None => host_path.to_string(),
    }
}

// ── Internal Helpers ──────────────────────────────────────────────────────

fn build_resolv_conf() -> String {
    // Try host resolv.conf first
    if let Ok(host_resolv) = std::fs::read_to_string("/etc/resolv.conf")
        && !host_resolv.trim().is_empty()
        && !host_resolv.contains("127.0.0.53")
        && !host_resolv.contains("systemd-resolved")
    {
        return host_resolv;
    }
    "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
}

fn mount_rootfs_components(
    rootfs_path: &str,
    is_root: bool,
    volumes: &[ResolvedVolume],
) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    anyhow::ensure!(rootfs.exists(), "Rootfs does not exist: {}", rootfs_path);

    make_parent_mount_private(rootfs).ok();

    sys::mount(
        Some(rootfs_path),
        rootfs_path,
        None,
        MountFlags::BIND | MountFlags::REC,
        None,
    )
    .context("Failed to bind mount rootfs")?;

    if !volumes.is_empty() {
        bind_mount_volumes(rootfs_path, volumes);
    }

    if !is_root {
        bind_host_fs(rootfs, "/proc", MountFlags::BIND | MountFlags::REC)?;
        bind_host_fs(rootfs, "/sys", MountFlags::BIND | MountFlags::REC)?;
    }

    // Bind-mount device nodes from host
    let dev = rootfs.join("dev");
    std::fs::create_dir_all(&dev).ok();
    for &(name, _, _) in DEV_NODES {
        let src = Path::new("/dev").join(name);
        let dst = dev.join(name);
        if rustix::fs::accessat(rustix::fs::CWD, &src, rustix::fs::Access::READ_OK, rustix::fs::AtFlags::empty()).is_ok() {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            sys::mount(Some(src.to_str().unwrap()), dst.to_str().unwrap(), None, MountFlags::BIND, None).ok();
        }
    }
    let _ = std::os::unix::fs::symlink("/proc/self/fd", dev.join("fd"));
    let _ = std::os::unix::fs::symlink("/proc/self/fd/0", dev.join("stdin"));
    let _ = std::os::unix::fs::symlink("/proc/self/fd/1", dev.join("stdout"));
    let _ = std::os::unix::fs::symlink("/proc/self/fd/2", dev.join("stderr"));

    Ok(())
}

fn enter_rootfs(rootfs_path: &str, is_root: bool, volumes: &[ResolvedVolume]) -> Result<()> {
    mount_rootfs_components(rootfs_path, is_root, volumes)?;

    let rootfs = Path::new(rootfs_path);
    let old_root = rootfs.join(".z8s_old_root");
    std::fs::create_dir_all(&old_root)?;

    sys::pivot_root(rootfs_path, old_root.to_str().unwrap()).context("Failed to pivot_root")?;
    sys::chdir("/").context("Failed to chdir to /")?;

    rustix::mount::mount_change("/.z8s_old_root", MountPropagationFlags::PRIVATE | MountPropagationFlags::REC).ok();
    sys::umount2("/.z8s_old_root", UnmountFlags::DETACH).ok();
    let _ = std::fs::remove_dir("/.z8s_old_root");

    mount_filesystems(is_root)?;
    Ok(())
}

fn mount_filesystems(is_root: bool) -> Result<()> {
    if is_root {
        sys::mount(Some("proc"), "/proc", Some("proc"), MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV, None)
            .context("Failed to mount /proc")?;
        sys::mount(Some("sysfs"), "/sys", Some("sysfs"), MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV, None)
            .context("Failed to mount /sys")?;
    }

    sys::mount(Some("tmpfs"), "/tmp", Some("tmpfs"), MountFlags::NOSUID | MountFlags::NODEV, None)
        .context("Failed to mount /tmp")?;
    sys::mount(Some("devpts"), "/dev/pts", Some("devpts"), MountFlags::NOSUID | MountFlags::NOEXEC, None)
        .context("Failed to mount /dev/pts")?;

    let shm = Path::new("/dev/shm");
    if !shm.exists() {
        std::fs::create_dir_all(shm).ok();
    }
    sys::mount(Some("tmpfs"), "/dev/shm", Some("tmpfs"), MountFlags::NOSUID | MountFlags::NODEV | MountFlags::NOEXEC, None)
        .context("Failed to mount /dev/shm")?;

    info!("Container filesystem mounted");
    Ok(())
}

fn bind_host_fs(rootfs: &Path, mount_point: &str, flags: MountFlags) -> Result<()> {
    let host_path = Path::new(mount_point);
    let dst = rootfs.join(mount_point.trim_start_matches('/'));
    if rustix::fs::accessat(rustix::fs::CWD, host_path, rustix::fs::Access::READ_OK, rustix::fs::AtFlags::empty()).is_err() {
        return Ok(());
    }
    std::fs::create_dir_all(&dst).ok();
    sys::mount(Some(mount_point), dst.to_str().unwrap(), None, flags, None)
        .context(format!("Failed to bind-mount {}", mount_point))?;
    // Remount readonly
    sys::mount(Some(mount_point), dst.to_str().unwrap(), None, flags | MountFlags::RDONLY, None).ok();
    Ok(())
}

/// Parse /proc/self/mountinfo to make the parent mount of `rootfs` private.
fn make_parent_mount_private(rootfs: &Path) -> Result<()> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .context("Failed to read /proc/self/mountinfo")?;

    let rootfs_str = rootfs.to_string_lossy();
    let mut best: Option<(String, bool)> = None;
    let mut best_len = 0;

    for line in mountinfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let mount_point = match parts.get(4) {
            Some(p) => *p,
            None => continue,
        };
        let mp = mount_point.trim_end_matches('/');
        let matches = if mp.is_empty() {
            rootfs_str.starts_with('/')
        } else {
            rootfs_str == mp || rootfs_str.starts_with(&format!("{mp}/"))
        };
        if matches && mp.len() >= best_len {
            let is_shared = if let Some(dash_pos) = line.find(" - ") {
                line[..dash_pos].split_whitespace().skip(6).any(|f| f.starts_with("shared:"))
            } else {
                line.split_whitespace().skip(6).any(|f| f.starts_with("shared:"))
            };
            best = Some((mount_point.to_string(), is_shared));
            best_len = mp.len();
        }
    }

    if let Some((mp, true)) = best {
        debug!("Making parent mount private: {}", mp);
        rustix::mount::mount_change(&mp, MountPropagationFlags::PRIVATE)
            .with_context(|| format!("Failed to make parent mount private: {}", mp))?;
    }
    Ok(())
}

fn get_container_path_from_proc(rootfs_path: &str) -> Option<String> {
    let root = rootfs_path.trim_end_matches('/');
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let proc_root = format!("/proc/{}/root", name_str);
        let Ok(target) = std::fs::read_link(&proc_root) else {
            continue;
        };
        if target.to_string_lossy().trim_end_matches('/') == root
            && let Ok(data) = std::fs::read(format!("/proc/{}/environ", name_str))
        {
            for var in data.split(|&b| b == 0) {
                if var.starts_with(b"PATH=") {
                    return String::from_utf8(var[5..].to_vec()).ok();
                }
            }
        }
    }
    None
}

fn postgres_dynamic_paths(root: &str, entrypoint: &str) -> Vec<String> {
    let pg_dir = format!("{root}/usr/lib/postgresql");
    let Ok(entries) = std::fs::read_dir(&pg_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let bin_dir = e.path().join("bin");
            if bin_dir.exists() {
                Some(format!("{}/{entrypoint}", bin_dir.to_string_lossy()))
            } else {
                None
            }
        })
        .collect()
}

fn busybox_argv(exec_path: &str, entrypoint: &str, args: &[String]) -> Vec<String> {
    if exec_path.ends_with("/busybox") && !entrypoint.contains("busybox") {
        std::iter::once(entrypoint.to_string())
            .chain(args.iter().cloned())
            .collect()
    } else {
        args.to_vec()
    }
}

fn read_elf_interpreter(path: &str) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 64 || data.get(0..4)? != b"\x7fELF" {
        return None;
    }
    let elf_class = *data.get(4)?;
    let (e_phoff, phentsize, phnum): (usize, usize, usize) = if elf_class == 2 {
        (
            u64::from_le_bytes(data.get(32..40)?.try_into().ok()?) as usize,
            u16::from_le_bytes(data.get(54..56)?.try_into().ok()?) as usize,
            u16::from_le_bytes(data.get(56..58)?.try_into().ok()?) as usize,
        )
    } else if elf_class == 1 {
        (
            u32::from_le_bytes(data.get(28..32)?.try_into().ok()?) as usize,
            u16::from_le_bytes(data.get(42..44)?.try_into().ok()?) as usize,
            u16::from_le_bytes(data.get(44..46)?.try_into().ok()?) as usize,
        )
    } else {
        return None;
    };
    if phentsize < 8 || e_phoff + phnum.saturating_mul(phentsize) > data.len() {
        return None;
    }
    for i in 0..phnum {
        let off = e_phoff + i * phentsize;
        let p_type = u32::from_le_bytes(data.get(off..off + 4)?.try_into().ok()?);
        if p_type != 3 {
            continue;
        }
        let (p_offset, p_filesz) = if elf_class == 2 {
            (
                u64::from_le_bytes(data.get(off + 8..off + 16)?.try_into().ok()?) as usize,
                u64::from_le_bytes(data.get(off + 32..off + 40)?.try_into().ok()?) as usize,
            )
        } else {
            (
                u32::from_le_bytes(data.get(off + 4..off + 8)?.try_into().ok()?) as usize,
                u32::from_le_bytes(data.get(off + 16..off + 20)?.try_into().ok()?) as usize,
            )
        };
        if p_filesz == 0 || p_offset + p_filesz > data.len() {
            return None;
        }
        let bytes = &data[p_offset..p_offset + p_filesz];
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        return Some(String::from_utf8_lossy(&bytes[..end]).into_owned());
    }
    None
}

// ── Volume Mounting ────────────────────────────────────────────────────────

/// Bind-mount volumes into the container rootfs.
pub fn bind_mount_volumes(rootfs_path: &str, volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = format!("{}{}", rootfs_path, vol.container_path);
        let src = Path::new(&vol.host_path);
        let dst_path = Path::new(&dst);

        if !src.exists() {
            let _ = std::fs::create_dir_all(src);
        }

        if dst_path.exists() || dst_path.is_symlink() {
            if dst_path.is_symlink() || dst_path.is_file() {
                std::fs::remove_file(dst_path).ok();
            } else if dst_path.is_dir() && is_emptydir(&vol.host_path) {
                std::fs::remove_dir_all(dst_path).ok();
            }
        }

        if src.is_dir() {
            std::fs::create_dir_all(dst_path).ok();
        } else if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent).ok();
            if !dst_path.exists() {
                let _ = std::fs::write(dst_path, []);
            }
        }

        let flags = MountFlags::BIND | MountFlags::REC;
        if sys::mount(Some(vol.host_path.as_str()), &dst, None, flags, None).is_ok() {
            if vol.read_only {
                let ro = MountFlags::RDONLY;
                rustix::mount::mount_remount(&dst, ro, "").ok();
            }
            info!("Mounted volume {} → {}", vol.host_path, vol.container_path);
            continue;
        }

        // Fallback: copy or symlink
        if is_emptydir(&vol.host_path) {
            std::os::unix::fs::symlink(src, dst_path).ok();
        } else if copy_tree(src, dst_path).is_ok() {
            info!("Copied volume {} → {} (bind unavailable)", vol.host_path, vol.container_path);
        } else {
            warn!("Failed to install volume {} → {}", vol.host_path, vol.container_path);
        }
    }
}

/// Bind-mount volumes in degraded mode (host mount view).
pub fn bind_mount_volumes_degraded(volumes: &[ResolvedVolume]) {
    for vol in volumes {
        let dst = vol.container_path.clone();
        let src = Path::new(&vol.host_path);
        let dst_path = Path::new(&dst);

        if !src.exists() {
            let _ = std::fs::create_dir_all(src);
        }

        if dst_path.exists() {
            if dst_path.is_symlink() {
                std::fs::remove_file(dst_path).ok();
            } else if is_emptydir(&vol.host_path) {
                std::fs::remove_dir_all(dst_path).ok();
            }
        }

        let flags = MountFlags::BIND | MountFlags::REC;
        if sys::mount(Some(vol.host_path.as_str()), &dst, None, flags, None).is_ok() {
            if vol.read_only {
                let ro = MountFlags::RDONLY;
                rustix::mount::mount_remount(&dst, ro, "").ok();
            }
            info!("Mounted volume (degraded) {} → {}", vol.host_path, vol.container_path);
            continue;
        }

        if is_emptydir(&vol.host_path) {
            std::os::unix::fs::symlink(src, dst_path).ok();
        } else if copy_tree(src, dst_path).is_ok() {
            info!("Copied volume (degraded) {} → {}", vol.host_path, vol.container_path);
        } else {
            warn!("Failed to install volume (degraded) {} → {}", vol.host_path, vol.container_path);
        }
    }
}

/// Clean up EmptyDir data for a pod.
pub fn cleanup_emptydir(pod_uid: &str) {
    let base = "/home/abb/.local/share/z8s".to_string();
    let emptydir_base = format!("{}/emptydir", base);
    let safe_uid = pod_uid.replace('/', "_");
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("rm -rf {}/{}*", emptydir_base, safe_uid))
        .spawn();
}

/// Setup mounts needed for exec (proc, devpts, ptmx).
pub fn setup_exec_mounts(rootfs: &str) -> Result<()> {
    let root_path = Path::new(rootfs);

    let proc_path = root_path.join("proc");
    let _ = std::fs::create_dir_all(&proc_path);
    sys::mount(Some("proc"), proc_path.to_str().unwrap(), Some("proc"), MountFlags::NOSUID | MountFlags::NOEXEC | MountFlags::NODEV, None).ok();

    let pts = root_path.join("dev/pts");
    let _ = std::fs::create_dir_all(&pts);
    sys::mount(Some("devpts"), pts.to_str().unwrap(), Some("devpts"), MountFlags::NOSUID | MountFlags::NOEXEC, None).ok();

    let ptmx = root_path.join("dev/ptmx");
    if !ptmx.exists() {
        sys::mknod(&ptmx, 5, 2).ok();
    }

    let etc_path = root_path.join("etc");
    let _ = std::fs::create_dir_all(&etc_path);
    let resolv = etc_path.join("resolv.conf");
    let _ = std::fs::write(&resolv, build_resolv_conf());

    Ok(())
}

// ── Path Helpers ──────────────────────────────────────────────────────────

fn is_emptydir(host_path: &str) -> bool {
    host_path.contains("/emptydir/")
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        return Ok(());
    }
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
    }
    Ok(())
}
