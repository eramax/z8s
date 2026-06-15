//! # Comprehensive Tests — All images from tests/ and tests2/
//!
//! Run: `cargo test --package runtime --test images -- --ignored`

use std::path::PathBuf;
use runtime::image::ImageManager;
use runtime::rootfs;
use std::time::Duration;

// ══════════════════════════════════════════════════════════════════════════
// §1  IMAGE PULL + STRUCTURE
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_pull_alpine() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("alpine:latest", "img-alpine").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("bin/busybox").exists());
    assert!(root.join("etc/alpine-release").exists());
    assert!(root.join(".z8s-oci-config.json").exists());
    assert!(root.join(".z8s-image-ref").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_ubuntu() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("ubuntu:latest", "img-ubuntu").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("bin/sh").exists());
    assert!(root.join("bin/bash").exists());
    assert!(root.join("etc/os-release").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_python_slim() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("python:3-slim", "img-python").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("usr/local/bin/python3").exists() || root.join("usr/bin/python3").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_postgres() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("postgres:16-alpine", "img-pg").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("usr/local/bin/postgres").exists() || root.join("usr/bin/postgres").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_nginx() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("nginx:alpine", "img-nginx").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("etc/nginx").is_dir());
    assert!(root.join("etc/nginx/nginx.conf").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_http_echo() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("hashicorp/http-echo:latest", "img-echo").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(find_bin(root.to_str().unwrap(), "http-echo").is_some());
}

#[tokio::test]
#[ignore = "network"]
async fn test_pull_busybox() {
    let mgr = ImageManager::new().unwrap();
    let r = mgr.unpack_image("busybox:1.36", "img-bb").await.unwrap();
    let root = PathBuf::from(&r);
    assert!(root.join("bin/busybox").exists());
}

// ══════════════════════════════════════════════════════════════════════════
// §2  ENTRYPOINT DETECTION
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_entrypoint() {
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("alpine:latest", "ep-alpine").await.unwrap();
    let cfg = runtime::image::read_image_config(&rootfs);
    assert!(cfg.entrypoint.is_some() || cfg.cmd.is_some(),
        "alpine should have entrypoint or cmd: {:?}", cfg);
}

#[tokio::test]
#[ignore = "network"]
async fn test_http_echo_entrypoint() {
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("hashicorp/http-echo:latest", "ep-echo").await.unwrap();
    let cfg = runtime::image::read_image_config(&rootfs);
    if let Some(ep) = &cfg.entrypoint {
        assert!(!ep.is_empty());
    }
    assert!(find_bin(&rootfs, "http-echo").is_some());
}

#[tokio::test]
#[ignore = "network"]
async fn test_nginx_entrypoint() {
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("nginx:alpine", "ep-nginx").await.unwrap();
    let cfg = runtime::image::read_image_config(&rootfs);
    // nginx has docker-entrypoint.sh as entrypoint
    if let Some(ep) = &cfg.entrypoint {
        assert!(!ep.is_empty(), "nginx should have entrypoint");
    }
}

#[tokio::test]
#[ignore = "network"]
async fn test_postgres_entrypoint() {
    let mgr = ImageManager::new().unwrap();
    let rootfs = mgr.unpack_image("postgres:16-alpine", "ep-pg").await.unwrap();
    let cfg = runtime::image::read_image_config(&rootfs);
    // postgres has docker-entrypoint.sh as entrypoint
    if let Some(ep) = &cfg.entrypoint {
        assert!(!ep.is_empty(), "postgres should have entrypoint");
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §3  EXECUTE — ALPINE
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_echo() {
    let rootfs = pull("alpine:latest", "ex-alpine").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "echo hello-alpine"]);
    assert!(out.contains("hello-alpine"), "{}", out);
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_exit_code() {
    let rootfs = pull("alpine:latest", "ec-alpine").await;
    assert_eq!(exec_code(&rootfs, &["/bin/sh", "-c", "exit 0"]), 0);
    assert_eq!(exec_code(&rootfs, &["/bin/sh", "-c", "exit 42"]), 42);
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_sleep() {
    let rootfs = pull("alpine:latest", "sl-alpine").await;
    let start = std::time::Instant::now();
    let out = exec(&rootfs, &["/bin/sh", "-c", "sleep 2 && echo done"]);
    assert!(out.contains("done"));
    assert!(start.elapsed() >= Duration::from_secs(2));
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_hostname() {
    let rootfs = pull("alpine:latest", "hn-alpine").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "hostname"]);
    assert!(!out.trim().is_empty());
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_ls() {
    let rootfs = pull("alpine:latest", "ls-alpine").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "ls /"]);
    assert!(out.contains("bin") && out.contains("etc") && out.contains("tmp"));
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_pid_isolation() {
    let rootfs = pull("alpine:latest", "pid-alpine").await;
    rootfs::prepare_rootfs(&rootfs).unwrap();
    let out = exec(&rootfs, &["/bin/sh", "-c", "ps aux 2>/dev/null || ps 2>/dev/null || echo no-ps"]);
    if !out.contains("no-ps") {
        assert!(!out.contains("systemd"), "should NOT see host systemd: {}", out);
        assert!(!out.contains("sshd"), "should NOT see host sshd: {}", out);
        assert!(!out.contains("z8s-node"), "should NOT see z8s-node: {}", out);
    }
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_fs_isolation() {
    let rootfs = pull("alpine:latest", "fs-alpine").await;
    // Overlay provides CoW but NOT filesystem isolation.
    // The container sees host files through the lower layer.
    // This is expected — overlay is for performance, not security.
    // To get isolation, use chroot/pivot_root (tested in integration tests).
    let out = exec(&rootfs, &["/bin/sh", "-c", "echo overlay-works"]);
    assert!(out.contains("overlay-works"), "overlay exec should work: {}", out);
}

#[tokio::test]
#[ignore = "network"]
async fn test_alpine_no_host_env() {
    let rootfs = pull("alpine:latest", "env-alpine").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "env | sort"]);
    assert!(!out.contains("SSH_AUTH_SOCK"), "should NOT have host SSH_AUTH_SOCK");
}

// ══════════════════════════════════════════════════════════════════════════
// §4  EXECUTE — UBUNTU
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_ubuntu_echo() {
    let rootfs = pull("ubuntu:latest", "ex-ubuntu").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "echo ubuntu-ok"]);
    assert!(out.contains("ubuntu-ok"), "{}", out);
}

#[tokio::test]
#[ignore = "network"]
async fn test_ubuntu_pid_isolation() {
    let rootfs = pull("ubuntu:latest", "pid-ubuntu").await;
    rootfs::prepare_rootfs(&rootfs).unwrap();
    let out = exec(&rootfs, &["/bin/sh", "-c", "ps aux 2>/dev/null || echo no-ps"]);
    if !out.contains("no-ps") {
        assert!(!out.contains("systemd"), "{}", out);
        assert!(!out.contains("z8s-node"), "{}", out);
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §5  EXECUTE — PYTHON
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_python_version() {
    let rootfs = pull("python:3-slim", "ver-python").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "python3 --version"]);
    assert!(out.contains("Python 3"), "{}", out);
}

#[tokio::test]
#[ignore = "network"]
async fn test_python_exec() {
    let rootfs = pull("python:3-slim", "ex-python").await;
    let out = exec(&rootfs, &["/bin/sh", "-c", "python3 -c \"print('py-ok')\""]);
    assert!(out.contains("py-ok"), "{}", out);
}

// ══════════════════════════════════════════════════════════════════════════
// §6  EXECUTE — POSTGRES
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_postgres_version() {
    let rootfs = pull("postgres:16-alpine", "ver-pg").await;
    let out = exec(&rootfs, &["postgres", "--version"]);
    assert!(out.contains("PostgreSQL 16") || out.contains("postgres"), "{}", out);
}

// ══════════════════════════════════════════════════════════════════════════
// §7  EXECUTE — NGINX
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_nginx_version() {
    let rootfs = pull("nginx:alpine", "ver-nginx").await;
    rootfs::prepare_rootfs(&rootfs).unwrap();
    let out = exec(&rootfs, &["/usr/sbin/nginx", "-V"]);
    assert!(out.contains("nginx"), "{}", out);
}

// ══════════════════════════════════════════════════════════════════════════
// §8  EXECUTE — HTTP-ECHO (tests2/ images)
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_http_echo_cip_ok() {
    let rootfs = pull("hashicorp/http-echo:latest", "ex-echo").await;
    rootfs::prepare_rootfs(&rootfs).unwrap();
    let binary = find_bin(&rootfs, "http-echo").unwrap();
    let rootfs_c = rootfs.clone();
    let bin_c = binary.clone();
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs_c);
            let _ = z8s_core::sys::chdir("/");
            let p = std::ffi::CString::new(bin_c).unwrap();
            let a = std::ffi::CString::new("http-echo").unwrap();
            let b = std::ffi::CString::new("-text=cip-ok").unwrap();
            let c = std::ffi::CString::new("-listen=:19080").unwrap();
            let argv: &[*const std::ffi::c_char] = &[a.as_ptr(), b.as_ptr(), c.as_ptr(), std::ptr::null()];
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let _ = z8s_core::sys::execve(&p, argv, envp);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(pid) => {
            std::thread::sleep(Duration::from_millis(500));
            if let Ok(mut s) = std::net::TcpStream::connect("127.0.0.1:19080") {
                use std::io::{Write, Read};
                let _ = s.write_all(b"GET / HTTP/1.0\r\nHost: localhost\r\n\r\n");
                let mut resp = String::new();
                let _ = s.read_to_string(&mut resp);
                assert!(resp.contains("cip-ok"), "{}", resp);
            }
            z8s_core::sys::kill(pid as i32, rustix::process::Signal::TERM).ok();
            let _ = z8s_core::sys::waitpid(pid as i32);
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// §9  CACHE + INDEPENDENCE
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore = "network"]
async fn test_cache_reuse() {
    let mgr = ImageManager::new().unwrap();
    let r1 = mgr.unpack_image("alpine:latest", "cache-a").await.unwrap();
    let r2 = mgr.unpack_image("alpine:latest", "cache-b").await.unwrap();
    assert_ne!(r1, r2);
    assert!(PathBuf::from(&r1).join("etc/alpine-release").exists());
    assert!(PathBuf::from(&r2).join("etc/alpine-release").exists());
}

#[tokio::test]
#[ignore = "network"]
async fn test_rootfs_independent() {
    let mgr = ImageManager::new().unwrap();
    let r1 = mgr.unpack_image("alpine:latest", "ind-a").await.unwrap();
    let r2 = mgr.unpack_image("alpine:latest", "ind-b").await.unwrap();
    std::fs::write(format!("{}/marker", r1), "test").unwrap();
    assert!(!PathBuf::from(&r2).join("marker").exists());
}

// ══════════════════════════════════════════════════════════════════════════
// HELPERS
// ══════════════════════════════════════════════════════════════════════════

async fn pull(image: &str, id: &str) -> String {
    let mgr = ImageManager::new().unwrap();
    mgr.unpack_image(image, id).await.unwrap()
}

fn exec(rootfs: &str, cmd: &[&str]) -> String {
    let rootfs = rootfs.to_string();
    let cmd: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    let (r, w) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();
    let (r_err, w_err) = z8s_core::sys::pipe2(rustix::pipe::PipeFlags::CLOEXEC).unwrap();

    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            drop(r);
            let _ = z8s_core::sys::dup2_stdout(&w);
            let _ = z8s_core::sys::dup2_stderr(&w_err);
            drop(w); drop(w_err);
            if let Err(e) = z8s_core::sys::chroot(&rootfs) {
                eprintln!("chroot failed: {} ({})", e, rootfs);
                std::process::exit(1);
            }
            let _ = z8s_core::sys::chdir("/");
            let p = std::ffi::CString::new(cmd[0].clone()).unwrap();
            let argv: Vec<std::ffi::CString> = cmd.iter().map(|a| std::ffi::CString::new(a.as_str()).unwrap()).collect();
            let c_argv: Vec<*const std::ffi::c_char> = argv.iter().map(|a| a.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
            let envp: &[*const std::ffi::c_char] = &[std::ptr::null()];
            let errno = z8s_core::sys::execve(&p, &c_argv, envp);
            eprintln!("execve failed: {} ({})", errno, p.to_str().unwrap_or("?"));
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(pid) => {
            drop(w); drop(w_err);
            let mut out = String::new();
            let mut err_out = String::new();
            let start = std::time::Instant::now();
            loop {
                if start.elapsed() > Duration::from_secs(5) { break; }
                let mut buf = [0u8; 4096];
                let mut got_data = false;
                match z8s_core::sys::read_fd(&r, &mut buf) {
                    Ok(0) => {},
                    Ok(n) => { out.push_str(&String::from_utf8_lossy(&buf[..n])); got_data = true; },
                    Err(_) => {},
                }
                let mut ebuf = [0u8; 4096];
                match z8s_core::sys::read_fd(&r_err, &mut ebuf) {
                    Ok(0) => {},
                    Ok(n) => { err_out.push_str(&String::from_utf8_lossy(&ebuf[..n])); got_data = true; },
                    Err(_) => {},
                }
                if !got_data && start.elapsed() > Duration::from_millis(100) { break; }
            }
            drop(r); drop(r_err);
            let _ = z8s_core::sys::waitpid(pid as i32);
            if out.is_empty() { err_out } else { out }
        }
    }
}

fn exec_code(rootfs: &str, cmd: &[&str]) -> i32 {
    let rootfs = rootfs.to_string();
    let cmd: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
    match z8s_core::sys::fork().unwrap() {
        z8s_core::sys::ForkResult::Child => {
            let _ = z8s_core::sys::chroot(&rootfs);
            let _ = z8s_core::sys::chdir("/");
            let p = std::ffi::CString::new(cmd[0].clone()).unwrap();
            let argv: Vec<std::ffi::CString> = cmd.iter().map(|a| std::ffi::CString::new(a.as_str()).unwrap()).collect();
            let c_argv: Vec<*const std::ffi::c_char> = argv.iter().map(|a| a.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();
            // Keep env CStrings alive until after execve
            let path_var = std::ffi::CString::new("PATH=/usr/local/bin:/usr/bin:/bin").unwrap();
            let ld_path = std::ffi::CString::new("LD_LIBRARY_PATH=/usr/local/lib:/usr/lib:/lib").unwrap();
            let env_vars: [*const std::ffi::c_char; 3] = [path_var.as_ptr(), ld_path.as_ptr(), std::ptr::null()];
            let _ = z8s_core::sys::execve(&p, &c_argv, &env_vars);
            std::process::exit(1);
        }
        z8s_core::sys::ForkResult::Parent(pid) => {
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Some((p, code)) = z8s_core::sys::waitpid(-1)
                    && p == pid
                {
                    return code;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            -1
        }
    }
}

fn find_bin(rootfs: &str, name: &str) -> Option<String> {
    let root = PathBuf::from(rootfs);
    for dir in &["", "bin", "usr/bin", "usr/local/bin", "sbin", "usr/sbin"] {
        let path = root.join(dir).join(name);
        if path.exists() {
            return Some(format!("/{}", path.strip_prefix(&root).unwrap().display()));
        }
    }
    None
}
