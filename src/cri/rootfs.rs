use anyhow::{Context, Result};
use nix::mount::{mount, umount, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::sys::stat::{makedev, mknod, Mode, SFlag};
use nix::unistd::{chdir, chroot, getgid, getuid, pivot_root, sethostname, Uid};
use std::os::fd::OwnedFd;
use std::path::Path;
use tracing::{debug, info, warn};

pub fn is_root() -> bool {
    Uid::effective().is_root()
}

/// How the container process sees its filesystem after setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootfsIsolation {
    /// pivot_root into the OCI rootfs (full isolation).
    Pivot,
    /// chroot into the OCI rootfs.
    Chroot,
    /// User namespace only; process runs on the host mount view.
    Degraded,
}

/// Resolve binary path when the process runs on the host mount view (no chroot).
fn get_container_path_from_proc(rootfs_path: &str) -> Option<String> {
    let root = rootfs_path.trim_end_matches('/');
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return None;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let pid = name_str;
        let proc_root = format!("/proc/{pid}/root");
        let Ok(target) = std::fs::read_link(&proc_root) else {
            continue;
        };
        if target.to_string_lossy().trim_end_matches('/') == root {
            // Found a process in this container! Read its environment
            if let Ok(data) = std::fs::read(format!("/proc/{pid}/environ")) {
                for var in data.split(|&b| b == 0) {
                    if var.starts_with(b"PATH=") {
                        if let Ok(s) = String::from_utf8(var[5..].to_vec()) {
                            return Some(s);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Resolve binary path when the process runs on the host mount view (no chroot).
pub fn resolve_exec_path(entrypoint: &str, rootfs_path: &str) -> String {
    let root = rootfs_path.trim_end_matches('/');
    if entrypoint.starts_with('/') {
        return format!("{root}{entrypoint}");
    }

    let mut candidates = Vec::new();

    // Try to get actual container PATH first
    if let Some(container_path) = get_container_path_from_proc(rootfs_path) {
        for dir in container_path.split(':') {
            if !dir.is_empty() {
                candidates.push(format!("{root}/{}/{entrypoint}", dir.trim_start_matches('/')));
            }
        }
    }

    // Standard fallback paths
    for dir in &["bin", "usr/bin", "usr/local/bin", "sbin", "usr/sbin", "usr/local/sbin"] {
        candidates.push(format!("{root}/{dir}/{entrypoint}"));
    }

    // Postgres dynamic paths fallback
    let pg_dir = format!("{root}/usr/lib/postgresql");
    if let Ok(entries) = std::fs::read_dir(&pg_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                let bin_dir = entry.path().join("bin");
                if bin_dir.exists() {
                    candidates.push(format!("{}/{entrypoint}", bin_dir.to_string_lossy()));
                }
            }
        }
    }

    for p in candidates {
        let path = std::path::Path::new(&p);
        if !path.exists() && !path.is_symlink() {
            continue;
        }
        if let Ok(target) = std::fs::read_link(path) {
            let t = target.to_string_lossy();
            if t.starts_with('/') {
                let in_root = format!("{root}{t}");
                if std::path::Path::new(&in_root).exists() {
                    return in_root;
                }
            }
        }
        return p;
    }

    if std::path::Path::new(entrypoint).exists() {
        entrypoint.to_string()
    } else if entrypoint.starts_with('/') {
        format!("{root}{entrypoint}")
    } else {
        entrypoint.to_string()
    }
}

/// True when the container process has pivot_root/chroot into its OCI rootfs.
pub fn container_fs_isolated(container_pid: u32, rootfs_path: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let proc_root = format!("/proc/{container_pid}/root");
    let proc_meta = match std::fs::metadata(&proc_root) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let rootfs_meta = match std::fs::metadata(rootfs_path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    proc_meta.dev() == rootfs_meta.dev() && proc_meta.ino() == rootfs_meta.ino()
}

/// Read the PT_INTERP path from an ELF binary (e.g. `/lib/ld-musl-x86_64.so.1`).
fn read_elf_interpreter(path: &str) -> Option<String> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 64 || data.get(0..4)? != b"\x7fELF" {
        return None;
    }
    let elf_class = *data.get(4)?;
    let (e_phoff, phentsize, phnum): (usize, usize, usize) = if elf_class == 2 {
        let e_phoff = u64::from_le_bytes(data.get(32..40)?.try_into().ok()?) as usize;
        let phentsize = u16::from_le_bytes(data.get(54..56)?.try_into().ok()?) as usize;
        let phnum = u16::from_le_bytes(data.get(56..58)?.try_into().ok()?) as usize;
        (e_phoff, phentsize, phnum)
    } else if elf_class == 1 {
        let e_phoff = u32::from_le_bytes(data.get(28..32)?.try_into().ok()?) as usize;
        let phentsize = u16::from_le_bytes(data.get(42..44)?.try_into().ok()?) as usize;
        let phnum = u16::from_le_bytes(data.get(44..46)?.try_into().ok()?) as usize;
        (e_phoff, phentsize, phnum)
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
        let (p_offset, p_filesz): (usize, usize) = if elf_class == 2 {
            let p_offset = u64::from_le_bytes(data.get(off + 8..off + 16)?.try_into().ok()?) as usize;
            let p_filesz = u64::from_le_bytes(data.get(off + 32..off + 40)?.try_into().ok()?) as usize;
            (p_offset, p_filesz)
        } else {
            let p_offset = u32::from_le_bytes(data.get(off + 4..off + 8)?.try_into().ok()?) as usize;
            let p_filesz = u32::from_le_bytes(data.get(off + 16..off + 20)?.try_into().ok()?) as usize;
            (p_offset, p_filesz)
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

/// Run a rootfs ELF via its recorded dynamic linker (degraded / host-path exec).
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
    if !std::path::Path::new(&loader).exists() {
        return (exec_path.to_string(), prog_args);
    }
    let mut args = vec![exec_path.to_string()];
    args.extend(prog_args);
    (loader, args)
}

/// Build exec path and argv (Alpine busybox applets: `sleep` → `busybox sleep …`).
pub fn build_container_argv(
    entrypoint: &str,
    args: &[String],
    rootfs_path: &str,
) -> (String, Vec<String>) {
    let exec_path = resolve_exec_path(entrypoint, rootfs_path);
    let argv = busybox_argv(&exec_path, entrypoint, args);
    (exec_path, argv)
}

/// Map a host path under `rootfs_path` to the path seen after setns into the container mount namespace.
pub fn host_path_in_container_root(host_path: &str, rootfs_path: &str) -> String {
    let root = rootfs_path.trim_end_matches('/');
    if let Some(rest) = host_path.strip_prefix(root) {
        if rest.is_empty() {
            "/".to_string()
        } else if rest.starts_with('/') {
            rest.to_string()
        } else {
            format!("/{rest}")
        }
    } else if host_path.starts_with('/') {
        host_path.to_string()
    } else {
        host_path.to_string()
    }
}

/// Paths for exec after setns(CLONE_NEWNS): resolve on the host, execute using in-container paths.
pub fn build_container_argv_in_mount_ns(
    entrypoint: &str,
    args: &[String],
    rootfs_path: &str,
) -> (String, Vec<String>) {
    let (host_exec, argv) = build_container_argv(entrypoint, args, rootfs_path);
    let exec_path = host_path_in_container_root(&host_exec, rootfs_path);
    (exec_path, argv)
}

fn busybox_argv(exec_path: &str, entrypoint: &str, args: &[String]) -> Vec<String> {
    if exec_path.ends_with("/busybox") && !entrypoint.contains("busybox") {
        let mut v = vec![entrypoint.to_string()];
        v.extend(args.iter().cloned());
        v
    } else {
        args.to_vec()
    }
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

    use std::os::unix::fs::PermissionsExt;
    let dev_nodes: &[&str] = &[
        "null", "zero", "full", "random", "urandom", "tty", "console", "ptmx",
    ];
    for name in dev_nodes {
        let path = rootfs.join("dev").join(name);
        let _ = std::fs::write(&path, []);
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666));
    }

    let resolv_conf = rootfs.join("etc/resolv.conf");
    let content = if crate::config::dns_port().is_some() {
        let domain = &crate::config::get().cluster_domain;
        format!("nameserver 127.0.0.1\nsearch default.svc.{domain} svc.{domain} {domain}\noptions ndots:5\n")
    } else {
        let host_resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        if host_resolv.trim().is_empty()
            || host_resolv.contains("127.0.0.53")
            || host_resolv.contains("systemd-resolved")
        {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        } else {
            host_resolv
        }
    };
    std::fs::write(&resolv_conf, content)
        .context("Failed to write /etc/resolv.conf")?;

    let hosts = rootfs.join("etc/hosts");
    if !hosts.exists() {
        std::fs::write(&hosts, "127.0.0.1 localhost\n")
            .context("Failed to write /etc/hosts")?;
    }

    info!("Rootfs prepared at: {}", rootfs_path);
    Ok(())
}

pub fn write_userns_maps(
    child_pid: i32,
    run_as_user: Option<u32>,
    run_as_group: Option<u32>,
) -> Result<()> {
    let uid = getuid().as_raw();
    let gid = getgid().as_raw();

    // Prefer newuidmap/newgidmap (uidmap package): writes full subuid range and
    // does NOT set setgroups=deny, so setgroups(2)/seteuid(2) work inside the
    // container. This allows apt, su, sudo, and any tool that drops privileges.
    if try_newid_maps(child_pid, uid, gid).is_ok() {
        info!("Wrote userns maps via newuidmap/newgidmap for child pid {} (uid={} gid={})", child_pid, uid, gid);
        return Ok(());
    }

    if try_write_subid_maps_direct(child_pid, uid, gid).is_ok() {
        info!(
            "Wrote userns maps via /proc uid_map for child pid {} (uid={} gid={})",
            child_pid, uid, gid
        );
        return Ok(());
    }

    // Fallback: one container UID mapped to the host user. When runAsUser is set,
    // map that UID only so setuid(runAsUser) works without the uidmap package.
    std::fs::write(format!("/proc/{}/setgroups", child_pid), "deny")
        .with_context(|| format!("Failed to write setgroups for pid {}", child_pid))?;

    let map_uid = run_as_user.unwrap_or(0);
    let map_gid = run_as_group.unwrap_or(map_uid);

    std::fs::write(
        format!("/proc/{}/uid_map", child_pid),
        format!("{} {} 1\n", map_uid, uid),
    )
    .with_context(|| format!("Failed to write uid_map for pid {}", child_pid))?;
    std::fs::write(
        format!("/proc/{}/gid_map", child_pid),
        format!("{} {} 1\n", map_gid, gid),
    )
    .with_context(|| format!("Failed to write gid_map for pid {}", child_pid))?;

    info!(
        "Wrote single UID/GID map for child pid {} (container uid={} gid={} → host uid={} gid={})",
        child_pid, map_uid, map_gid, uid, gid
    );
    Ok(())
}

fn idmap_bin(name: &str) -> String {
    for path in [format!("/usr/bin/{name}"), format!("/bin/{name}")] {
        if std::path::Path::new(&path).exists() {
            return path;
        }
    }
    name.to_string()
}

fn try_newid_maps(child_pid: i32, uid: u32, gid: u32) -> Result<()> {
    let uid_args = build_idmap_args(child_pid, uid, "/etc/subuid")?;
    let gid_args = build_idmap_args(child_pid, gid, "/etc/subgid")?;

    let ok = std::process::Command::new(idmap_bin("newuidmap"))
        .args(&uid_args)
        .status()
        .context("Failed to run newuidmap")?
        .success();
    anyhow::ensure!(ok, "newuidmap exited with error");

    let ok = std::process::Command::new(idmap_bin("newgidmap"))
        .args(&gid_args)
        .status()
        .context("Failed to run newgidmap")?
        .success();
    anyhow::ensure!(ok, "newgidmap exited with error");

    Ok(())
}

/// Write subuid/subgid ranges directly when newuidmap is missing from PATH.
fn try_write_subid_maps_direct(child_pid: i32, uid: u32, gid: u32) -> Result<()> {
    let (uid_start, uid_count) = read_subid("/etc/subuid", uid)
        .ok_or_else(|| anyhow::anyhow!("no subuid entry for uid {}", uid))?;
    let (gid_start, gid_count) = read_subid("/etc/subgid", gid)
        .ok_or_else(|| anyhow::anyhow!("no subgid entry for gid {}", gid))?;

    std::fs::write(format!("/proc/{}/setgroups", child_pid), "deny")
        .context("Failed to write setgroups")?;
    std::fs::write(
        format!("/proc/{}/uid_map", child_pid),
        format!("0 {} 1\n1 {} {}\n", uid, uid_start, uid_count),
    )
    .context("Failed to write uid_map")?;
    std::fs::write(
        format!("/proc/{}/gid_map", child_pid),
        format!("0 {} 1\n1 {} {}\n", gid, gid_start, gid_count),
    )
    .context("Failed to write gid_map")?;
    Ok(())
}

// Builds args for newuidmap/newgidmap:
//   <pid> 0 <host_id> 1 [1 <subid_start> <subid_count>]
// The first entry maps container root (0) to the host user.
// The second entry maps container UIDs 1-N to the subordinate ID range,
// allowing tools like apt to switch to non-root UIDs (e.g. _apt = UID 42).
fn build_idmap_args(child_pid: i32, host_id: u32, subid_file: &str) -> Result<Vec<String>> {
    let mut args = vec![
        child_pid.to_string(),
        "0".to_string(), host_id.to_string(), "1".to_string(),
    ];
    if let Some((start, count)) = read_subid(subid_file, host_id) {
        args.extend(["1".to_string(), start.to_string(), count.to_string()]);
    }
    Ok(args)
}

fn read_subid(path: &str, host_id: u32) -> Option<(u64, u64)> {
    let content = std::fs::read_to_string(path).ok()?;
    let username = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(host_id))
        .ok()
        .flatten()
        .map(|u| u.name);

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(3, ':');
        let id_field = parts.next()?;
        let start: u64 = parts.next()?.parse().ok()?;
        let count: u64 = parts.next()?.parse().ok()?;

        let matches = id_field.parse::<u32>().map_or(false, |id| id == host_id)
            || username.as_deref().map_or(false, |name| id_field == name);

        if matches {
            return Some((start, count));
        }
    }
    None
}

pub fn child_enter_ns_fork(
    rootfs_path: &str,
    sync_w: OwnedFd,
    ack_r: OwnedFd,
    volumes: &[crate::cri::volumes::ResolvedVolume],
    isolate_net: bool,
    hostname: &str,
) -> Result<RootfsIsolation> {
    // Do not include CLONE_NEWPID: Go runtimes (whoami, http-echo) fail to spawn threads
    // with EINVAL in a PID namespace when filesystem isolation falls back to host mounts.
    let mut flags = CloneFlags::CLONE_NEWUSER
        | CloneFlags::CLONE_NEWNS
        | CloneFlags::CLONE_NEWUTS
        | CloneFlags::CLONE_NEWIPC;
    if isolate_net {
        flags |= CloneFlags::CLONE_NEWNET;
    }
    unshare(flags).context("Failed to unshare user/mount/uts/ipc namespaces")?;
    sethostname(hostname).context("Failed to set container hostname")?;

    if isolate_net {
        crate::netmux::veth::setup_loopback().ok();
    }

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

    // In user namespaces the kernel marks inherited mounts MNT_LOCKED, so MS_PRIVATE
    // on "/" fails with EPERM. MS_SLAVE succeeds and is sufficient: it prevents
    // bind-mounts from propagating to the host peer group while still allowing
    // mount events to flow inward from the host. make_parent_mount_private() then
    // makes only the direct parent of rootfs private before we pivot into it.
    if let Err(e) = mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        None::<&str>,
    ) {
        warn!("mount MS_SLAVE on / failed ({e}) — attempting chroot fallback");
    }

    // Non-root user ns: chroot (works if rootfs is bind-mounted inside the user ns)
    if mount_rootfs_components(rootfs_path, false, volumes).is_ok() {
        if chroot(rootfs_path).is_ok() {
            if chdir("/").is_ok() {
                if mount_filesystems(false).is_ok() {
                    return Ok(RootfsIsolation::Chroot);
                }
            }
        }
    }

    // Fallback: degraded with a loud warning
    tracing::error!("z8s: WARNING: FILESYSTEM ISOLATION UNAVAILABLE IN THIS ENVIRONMENT! RUNNING DEGRADED!");
    if !volumes.is_empty() {
        crate::cri::volumes::bind_mount_volumes_degraded(volumes);
    }
    Ok(RootfsIsolation::Degraded)
}

pub fn child_enter_ns_root(
    rootfs_path: &str,
    volumes: &[crate::cri::volumes::ResolvedVolume],
    isolate_net: bool,
    hostname: &str,
) -> Result<RootfsIsolation> {
    // Do NOT include CLONE_NEWPID: unshare(CLONE_NEWPID) only affects future fork()s from
    // this child, not the child itself. The kernel then requires a fresh /proc mount scoped
    // to the new PID namespace before the exec'd process can fork workers. Without a proper
    // double-fork / subreaper setup (which requires z8s to be PID 1), multi-process daemons
    // like nginx get ENOMEM when spawning workers. CLONE_NEWNS + chroot/pivot_root provides
    // sufficient filesystem isolation without breaking fork() inside the container.
    let mut flags = CloneFlags::CLONE_NEWNS
        | CloneFlags::CLONE_NEWUTS
        | CloneFlags::CLONE_NEWIPC;
    if isolate_net {
        flags |= CloneFlags::CLONE_NEWNET;
    }
    unshare(flags).context("Failed to unshare mount/uts/ipc")?;
    sethostname(hostname).context("Failed to set container hostname")?;

    if isolate_net {
        crate::netmux::veth::setup_loopback().ok();
    }

    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_SLAVE | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("Failed to set slave mount propagation")?;

    // Try pivot_root for stronger isolation (is_root=true: mount proc/sys/dev fresh)
    if enter_rootfs(rootfs_path, true, volumes).is_ok() {
        return Ok(RootfsIsolation::Pivot);
    }

    // Fallback: chroot
    warn!("pivot_root failed in root mode, falling back to chroot");
    if !volumes.is_empty() {
        crate::cri::volumes::bind_mount_volumes(rootfs_path, volumes);
    }
    chroot(rootfs_path).context("Failed to chroot")?;
    chdir("/").context("Failed to chdir to /")?;

    mount_filesystems_root()?;
    Ok(RootfsIsolation::Chroot)
}

/// Parse /proc/self/mountinfo to make the parent mount of `rootfs` MS_PRIVATE.
/// This is required before bind-mounting rootfs so the bind doesn't propagate
/// to shared peer groups on the host (youki technique for user namespaces).
fn make_parent_mount_private(rootfs: &Path) -> Result<()> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .context("Failed to read /proc/self/mountinfo")?;

    let rootfs_str = rootfs.to_string_lossy();
    let mut best_mount: Option<String> = None;
    let mut best_len: usize = 0;
    let mut best_is_shared = false;

    for line in mountinfo.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        // Field 5 (0-indexed 4) is the mount point
        let mount_point = match parts.get(4) {
            Some(p) => *p,
            None => continue,
        };

        let mp = mount_point.trim_end_matches('/');
        let mp_matches = if mp.is_empty() {
            rootfs_str.starts_with('/')
        } else {
            rootfs_str == mp
                || rootfs_str.starts_with(&format!("{mp}/"))
        };

        if mp_matches && mp.len() >= best_len {
            // Optional fields are everything after field 6 up to the " - " separator
            let is_shared = if let Some(dash_pos) = line.find(" - ") {
                line[..dash_pos].split_whitespace().skip(6).any(|f| f.starts_with("shared:"))
            } else {
                line.split_whitespace().skip(6).any(|f| f.starts_with("shared:"))
            };

            best_mount = Some(mount_point.to_string());
            best_len = mp.len();
            best_is_shared = is_shared;
        }
    }

    if let Some(mp) = best_mount {
        if best_is_shared {
            debug!("Making parent mount private: {}", mp);
            mount(None::<&str>, mp.as_str(), None::<&str>, MsFlags::MS_PRIVATE, None::<&str>)
                .with_context(|| format!("Failed to make parent mount private: {mp}"))?;
        }
    }

    Ok(())
}

fn mount_rootfs_components(
    rootfs_path: &str,
    is_root: bool,
    volumes: &[crate::cri::volumes::ResolvedVolume],
) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    if !rootfs.exists() {
        anyhow::bail!("Rootfs does not exist: {}", rootfs_path);
    }

    // Make parent mount private before binding rootfs so the bind doesn't
    // propagate to peer groups on the host (required in user namespaces).
    make_parent_mount_private(rootfs).ok();

    mount(
        Some(rootfs),
        rootfs,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("Failed to bind mount rootfs")?;

    if !volumes.is_empty() {
        crate::cri::volumes::bind_mount_volumes(rootfs_path, volumes);
    }

    // Bind-mount /proc and /sys from host into rootfs before pivot_root/chroot
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

    Ok(())
}

fn enter_rootfs(
    rootfs_path: &str,
    is_root: bool,
    volumes: &[crate::cri::volumes::ResolvedVolume],
) -> Result<()> {
    let rootfs = Path::new(rootfs_path);
    mount_rootfs_components(rootfs_path, is_root, volumes)?;

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
    } else {
        // In user namespace: /proc and /sys already bind-mounted before pivot_root.
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

/// Drop excess capabilities from all 5 cap sets.
/// Non-privileged containers retain the OCI default cap set plus any caps
/// listed in `securityContext.capabilities.add` (e.g. ["NET_ADMIN", "SYS_PTRACE"]).
pub fn drop_capabilities(privileged: bool, extra_caps: &[String]) {
    if privileged {
        return;
    }
    use caps::{CapSet, Capability};
    use std::collections::HashSet;

    let mut keep: HashSet<Capability> = [
        Capability::CAP_CHOWN,
        Capability::CAP_DAC_OVERRIDE,
        Capability::CAP_FSETID,
        Capability::CAP_FOWNER,
        Capability::CAP_MKNOD,
        Capability::CAP_NET_RAW,
        Capability::CAP_SETGID,
        Capability::CAP_SETUID,
        Capability::CAP_SETFCAP,
        Capability::CAP_SETPCAP,
        Capability::CAP_NET_BIND_SERVICE,
        Capability::CAP_SYS_CHROOT,
        Capability::CAP_KILL,
        Capability::CAP_AUDIT_WRITE,
    ]
    .iter()
    .cloned()
    .collect();

    // Add capabilities from securityContext.capabilities.add
    // Kubernetes uses names without the "CAP_" prefix (e.g. "NET_ADMIN")
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

/// Restrict the process to its rootfs (now "/") using Linux Landlock LSM.
/// Best-effort: silently skipped on kernels older than 5.13.
pub fn apply_landlock() {
    if let Err(e) = try_apply_landlock() {
        debug!("Landlock not applied ({})", e);
    }
}

fn try_apply_landlock() -> std::result::Result<(), Box<dyn std::error::Error>> {
    use landlock::{Access, AccessFs, ABI, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr};

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

/// Placeholder for seccomp BPF filter application.
/// Full implementation requires the syscallz crate (libseccomp-dev).
pub fn apply_seccomp(privileged: bool) {
    if privileged {
        return;
    }
    // TODO: Phase 1.5 — load etc/seccomp/default.json and apply via libseccomp/syscallz
    debug!("seccomp: filter not yet applied (deferred to Phase 1.5)");
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
    let content = if crate::config::dns_port().is_some() {
        let domain = &crate::config::get().cluster_domain;
        format!("nameserver 127.0.0.1\nsearch default.svc.{domain} svc.{domain} {domain}\noptions ndots:5\n")
    } else {
        let host_resolv = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_else(|_| {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        });
        if host_resolv.contains("127.0.0.53") || host_resolv.contains("systemd-resolved") {
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n".to_string()
        } else {
            host_resolv
        }
    };
    let _ = std::fs::write(&resolv, content);

    Ok(())
}
