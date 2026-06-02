//! Auto-generate self-signed TLS cert on first boot.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::info;

pub struct TlsCerts {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Ensure TLS cert+key exist in `data_dir`. Generate if missing.
pub fn ensure_tls_certs(data_dir: &str) -> Result<TlsCerts> {
    let cert_path = Path::new(data_dir).join("tls-cert.pem");
    let key_path = Path::new(data_dir).join("tls-key.pem");

    if cert_path.exists() && key_path.exists() {
        let cert_pem = std::fs::read_to_string(&cert_path)
            .context("read tls-cert.pem")?;
        let key_pem = std::fs::read_to_string(&key_path)
            .context("read tls-key.pem")?;
        info!("Using existing TLS cert from {}", cert_path.display());
        return Ok(TlsCerts { cert_pem, key_pem });
    }

    info!("Generating self-signed TLS certificate...");

    let mut params = rcgen::CertificateParams::new(vec![
        "localhost".to_string(),
    ])
    .context("create cert params")?;

    // Add IP SANs for local access
    params.subject_alt_names.push(
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))),
    );
    params.subject_alt_names.push(
        rcgen::SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0))),
    );

    let key_pair = rcgen::KeyPair::generate().context("generate key pair")?;
    let cert = params.self_signed(&key_pair).context("self-sign cert")?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();

    std::fs::create_dir_all(data_dir).context("create data dir")?;
    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("write {}", cert_path.display()))?;
    std::fs::write(&key_path, &key_pem)
        .with_context(|| format!("write {}", key_path.display()))?;

    // Restrict key file permissions
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).ok();
    }

    info!("TLS certificate generated at {}", cert_path.display());
    Ok(TlsCerts { cert_pem, key_pem })
}
