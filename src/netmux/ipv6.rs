use std::net::Ipv6Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// IPv6 address pool for public IP assignment from a host /64 prefix.
/// Each pod gets a /128 from the prefix.
pub struct Ipv6Pool {
    prefix_bytes: [u8; 16],
    next_iid: AtomicU64,
    available: RwLock<Vec<u64>>,
}

impl Ipv6Pool {
    /// Create a new pool from a prefix like "2001:db8::/64"
    pub fn new(prefix: &str) -> Result<Self, String> {
        let (addr_str, prefix_len) = prefix
            .split_once('/')
            .ok_or("Expected CIDR notation, e.g. 2001:db8::/64")?;
        let prefix_len: u8 = prefix_len.parse().map_err(|e| format!("Invalid prefix: {}", e))?;
        if prefix_len > 64 {
            return Err("Prefix length must be <= 64 for /64 allocation".to_string());
        }
        let addr: Ipv6Addr = addr_str
            .parse()
            .map_err(|e| format!("Invalid IPv6 address: {}", e))?;
        Ok(Self {
            prefix_bytes: addr.octets(),
            next_iid: AtomicU64::new(1),
            available: RwLock::new(Vec::new()),
        })
    }

    /// Allocate a new IPv6 address from the pool.
    pub fn allocate(&self) -> Ipv6Addr {
        // Check available pool first
        {
            let mut avail = self.available.write().unwrap_or_else(|e| {
                tracing::warn!("IPv6 pool RwLock poisoned");
                e.into_inner()
            });
            if let Some(iid) = avail.pop() {
                return self.iid_to_addr(iid);
            }
        }
        // Allocate new
        let iid = self.next_iid.fetch_add(1, Ordering::Relaxed);
        self.iid_to_addr(iid)
    }

    /// Release an address back to the pool.
    pub fn release(&self, addr: Ipv6Addr) {
        let iid = self.addr_to_iid(addr);
        self.available
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(iid);
    }

    fn iid_to_addr(&self, iid: u64) -> Ipv6Addr {
        let mut octets = self.prefix_bytes;
        octets[8..16].copy_from_slice(&iid.to_be_bytes());
        Ipv6Addr::from(octets)
    }

    fn addr_to_iid(&self, addr: Ipv6Addr) -> u64 {
        let octets = addr.octets();
        u64::from_be_bytes(octets[8..16].try_into().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipv6_pool_allocates_sequential() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        let a2 = pool.allocate();
        assert_ne!(a1, a2);
        assert_eq!(a1.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        assert_eq!(a2.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        // IID should be 1 and 2
        assert_eq!(u64::from_be_bytes(a1.octets()[8..16].try_into().unwrap()), 1);
        assert_eq!(u64::from_be_bytes(a2.octets()[8..16].try_into().unwrap()), 2);
    }

    #[test]
    fn test_ipv6_pool_release_reuse() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        pool.release(a1);
        let a2 = pool.allocate();
        assert_eq!(a1, a2);
    }
}
