pub mod dns;
pub mod port_publish;
pub mod service_proxy;

/// The port the z8s DNS server is listening on (53 or 5353). Set once at startup.
static DNS_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

pub fn set_dns_port(port: u16) {
    DNS_PORT.set(port).ok();
}

pub fn dns_port() -> Option<u16> {
    DNS_PORT.get().copied()
}

pub use crate::components::network::service::{NetworkManager, ServiceEndpoint};
