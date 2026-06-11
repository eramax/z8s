//! # IPAM — IP Address Management
//!
//! Pure functional IP address pools for IPv4 and IPv6.
//!
//! - [`Ipv4Cidr`] — parsed CIDR with network, prefix, broadcast, gateway
//! - [`IpPool`] — BTreeSet-based free-list allocator
//! - [`Ipv6Pool`] — /64-prefix allocator with Interface Identifier (IID)
//!
//! All pools are deterministic, ascending-order, and reserve the
//! standard network/gateway/broadcast addresses for IPv4.

use std::collections::{BTreeSet, HashSet};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

// ═══════════════════════════════════════════════════════════════════════════
// IPv4 CIDR
// ═══════════════════════════════════════════════════════════════════════════

/// An IPv4 CIDR (network + prefix length).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ipv4Cidr {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Cidr {
    /// Create a CIDR from network and prefix. Network is masked to align.
    pub fn new(network: Ipv4Addr, prefix: u8) -> Self {
        let mask = if prefix == 0 {
            0u32
        } else if prefix >= 32 {
            !0u32
        } else {
            !0u32 << (32 - prefix)
        };
        let aligned = u32::from(network) & mask;
        Self {
            network: Ipv4Addr::from(aligned),
            prefix: prefix.min(32),
        }
    }

    /// Parse a CIDR string like "10.42.0.0/20".
    pub fn parse(s: &str) -> Option<Self> {
        let (ip_str, prefix_str) = s.split_once('/')?;
        let prefix: u8 = prefix_str.parse().ok()?;
        if prefix > 32 {
            return None;
        }
        let ip: Ipv4Addr = ip_str.parse().ok()?;
        Some(Self::new(ip, prefix))
    }

    /// Get the network address as a u32.
    pub fn network_u32(&self) -> u32 {
        u32::from(self.network)
    }

    /// Number of usable host addresses (excluding network and broadcast).
    pub fn host_count(&self) -> u32 {
        let bits = 32u32 - self.prefix as u32;
        if bits == 0 {
            0
        } else {
            (1u32 << bits).saturating_sub(2)
        }
    }

    /// Check if an IP is within this CIDR.
    pub fn contains(&self, ip: &Ipv4Addr) -> bool {
        let mask = if self.prefix == 0 {
            0
        } else {
            !0u32 << (32 - self.prefix)
        };
        (u32::from(*ip) & mask) == (u32::from(self.network) & mask)
    }

    /// Get the broadcast address.
    pub fn broadcast(&self) -> Ipv4Addr {
        let bits = 32 - self.prefix as u32;
        let size = 1u32 << bits;
        Ipv4Addr::from(self.network_u32() + size - 1)
    }

    /// Get the gateway address (first usable host).
    pub fn gateway(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.network_u32() + 1)
    }

    /// Compute gateway for a CIDR (convenience).
    pub fn gateway_for(cidr: &Ipv4Cidr) -> Ipv4Addr {
        cidr.gateway()
    }
}

impl std::fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// IPv4 Pool
// ═══════════════════════════════════════════════════════════════════════════

/// A free-list based IP pool for IPv4 addresses.
#[derive(Debug, Clone)]
pub struct IpPool {
    cidr: Ipv4Cidr,
    free: BTreeSet<u32>,
}

impl IpPool {
    /// Create a new pool from a CIDR. Reserves network, gateway, and broadcast.
    pub fn new(cidr: Ipv4Cidr) -> Self {
        let network = cidr.network_u32();
        let bits = 32u32 - cidr.prefix as u32;
        let total = 1u32 << bits;
        let mut free = BTreeSet::new();
        if cidr.prefix >= 31 {
            for i in 0..total {
                free.insert(network + i);
            }
        } else {
            for i in 2..(total - 1) {
                free.insert(network + i);
            }
        }
        Self { cidr, free }
    }

    /// Allocate the next available IP (ascending).
    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        self.free.pop_first().map(Ipv4Addr::from)
    }

    /// Return an IP to the pool.
    pub fn release(&mut self, ip: Ipv4Addr) {
        if self.cidr.contains(&ip) {
            self.free.insert(u32::from(ip));
        }
    }

    /// Number of free addresses.
    pub fn count_free(&self) -> usize {
        self.free.len()
    }

    /// Total capacity.
    pub fn capacity(&self) -> u32 {
        self.cidr.host_count() + 2
    }

    /// CIDR of this pool.
    pub fn cidr(&self) -> &Ipv4Cidr {
        &self.cidr
    }

    /// Allocate a contiguous aligned subnet block.
    pub fn allocate_subnet(&mut self, subnet_prefix: u8) -> Option<Ipv4Cidr> {
        if subnet_prefix < self.cidr.prefix {
            return None;
        }
        let subnet_size = 1u32 << (32 - subnet_prefix as u32);
        if (self.free.len() as u32) < subnet_size {
            return None;
        }
        let free_vec: Vec<u32> = self.free.iter().copied().collect();
        for window in free_vec.windows(subnet_size as usize) {
            let start = window[0];
            if (start & (subnet_size - 1)) != 0 {
                continue;
            }
            if window
                .iter()
                .enumerate()
                .all(|(i, &v)| v == start + i as u32)
            {
                for ip in start..start + subnet_size {
                    self.free.remove(&ip);
                }
                return Some(Ipv4Cidr {
                    network: Ipv4Addr::from(start),
                    prefix: subnet_prefix,
                });
            }
        }
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// IPv6 Pool
// ═══════════════════════════════════════════════════════════════════════════

/// IPv6 address pool for /64 prefix allocation. Each address is /128.
#[derive(Debug, Clone)]
pub struct Ipv6Pool {
    prefix_bytes: [u8; 16],
    next_iid: Arc<AtomicU64>,
    available: Arc<RwLock<HashSet<u64>>>,
}

impl Ipv6Pool {
    /// Create from a prefix like "2001:db8::/64".
    pub fn new(prefix: &str) -> Result<Self, String> {
        let (addr_str, prefix_len) = prefix
            .split_once('/')
            .ok_or("Expected CIDR notation, e.g. 2001:db8::/64")?;
        let prefix_len: u8 = prefix_len
            .parse()
            .map_err(|e| format!("Invalid prefix: {}", e))?;
        if prefix_len > 128 {
            return Err("Prefix length must be <= 128".to_string());
        }
        let addr: Ipv6Addr = addr_str
            .parse()
            .map_err(|e| format!("Invalid IPv6 address: {}", e))?;
        let mut octets = addr.octets();
        let host_bits = (128 - prefix_len) as u16;
        let full_zero_bytes = (host_bits / 8) as usize;
        for i in (16 - full_zero_bytes)..16 {
            octets[i] = 0;
        }
        if host_bits % 8 != 0 {
            let partial_idx = 16 - full_zero_bytes - 1;
            let keep_bits = 8 - (host_bits % 8);
            octets[partial_idx] &= ((1u16 << keep_bits) - 1) as u8;
        }
        Ok(Self {
            prefix_bytes: octets,
            next_iid: Arc::new(AtomicU64::new(1)),
            available: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Allocate a new IPv6 address.
    pub fn allocate(&self) -> Ipv6Addr {
        {
            let mut avail = self.available.write().unwrap_or_else(|e| e.into_inner());
            if let Some(&iid) = avail.iter().next() {
                avail.remove(&iid);
                return self.iid_to_addr(iid);
            }
        }
        let iid = self.next_iid.fetch_add(1, Ordering::Relaxed);
        self.iid_to_addr(iid)
    }

    /// Release an address for reuse.
    pub fn release(&self, addr: Ipv6Addr) {
        let iid = self.addr_to_iid(addr);
        self.available
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(iid);
    }

    /// Number of currently available (released) addresses.
    pub fn available_count(&self) -> usize {
        self.available.read().unwrap_or_else(|e| e.into_inner()).len()
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

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_parse_basic() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.network, Ipv4Addr::new(10, 42, 0, 0));
        assert_eq!(cidr.prefix, 20);
    }

    #[test]
    fn cidr_parse_misaligned_normalizes() {
        let cidr = Ipv4Cidr::parse("10.42.5.13/20").unwrap();
        assert_eq!(cidr.network, Ipv4Addr::new(10, 42, 0, 0));
    }

    #[test]
    fn cidr_invalid() {
        assert!(Ipv4Cidr::parse("10.0.0.0/33").is_none());
        assert!(Ipv4Cidr::parse("not-ip/24").is_none());
        assert!(Ipv4Cidr::parse("10.0.0.0").is_none());
        assert!(Ipv4Cidr::parse("10.0.0.0/0").is_some()); // /0 is valid
    }

    #[test]
    fn cidr_zero_prefix_does_not_underflow() {
        // Was UB before; shift by 32 would be UB
        let cidr = Ipv4Cidr::new(Ipv4Addr::new(192, 168, 1, 50), 0);
        assert_eq!(cidr.prefix, 0);
        assert_eq!(cidr.network, Ipv4Addr::new(0, 0, 0, 0));
    }

    #[test]
    fn cidr_full_32_prefix() {
        let cidr = Ipv4Cidr::new(Ipv4Addr::new(10, 0, 0, 5), 32);
        assert_eq!(cidr.prefix, 32);
        assert_eq!(cidr.network, Ipv4Addr::new(10, 0, 0, 5));
    }

    #[test]
    fn cidr_host_count_slash_20() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.host_count(), 4094);
    }

    #[test]
    fn cidr_contains() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        assert!(cidr.contains(&Ipv4Addr::new(10, 42, 0, 5)));
        assert!(cidr.contains(&Ipv4Addr::new(10, 42, 0, 255)));
        assert!(!cidr.contains(&Ipv4Addr::new(10, 42, 1, 0)));
    }

    #[test]
    fn pool_slash_30_has_one_host() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/30").unwrap();
        let mut pool = IpPool::new(cidr);
        assert_eq!(pool.count_free(), 1);
        let ip = pool.allocate().unwrap();
        assert_eq!(ip, Ipv4Addr::new(10, 42, 0, 2));
        assert!(pool.allocate().is_none());
        pool.release(ip);
        assert_eq!(pool.count_free(), 1);
    }

    #[test]
    fn pool_allocates_ascending() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/29").unwrap();
        let mut pool = IpPool::new(cidr);
        let ip1 = pool.allocate().unwrap();
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip1, Ipv4Addr::new(10, 42, 0, 2));
        assert_eq!(ip2, Ipv4Addr::new(10, 42, 0, 3));
    }

    #[test]
    fn pool_release_reuses() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/29").unwrap();
        let mut pool = IpPool::new(cidr);
        let ip1 = pool.allocate().unwrap();
        pool.release(ip1);
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn pool_allocate_subnet_aligned() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
        let mut pool = IpPool::new(cidr);
        let subnet = pool.allocate_subnet(24).unwrap();
        assert_eq!(subnet.prefix, 24);
        assert_eq!(u32::from(subnet.network) & 0xFF, 0);
    }

    #[test]
    fn ipv6_pool_sequential() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        let a2 = pool.allocate();
        assert_ne!(a1, a2);
        assert_eq!(a1.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        assert_eq!(u64::from_be_bytes(a1.octets()[8..16].try_into().unwrap()), 1);
        assert_eq!(u64::from_be_bytes(a2.octets()[8..16].try_into().unwrap()), 2);
    }

    #[test]
    fn ipv6_pool_release_reuse() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        pool.release(a1);
        let a2 = pool.allocate();
        assert_eq!(a1, a2);
    }
}
