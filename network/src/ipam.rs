//! # IPAM — IP Address Management
//!
//! This module provides IP address allocation pools for:
//! - **Pod CIDR** — the main pod network (e.g., `10.42.0.0/20`)
//! - **Service CIDR** — ClusterIP allocations (e.g., `10.96.0.0/16`)
//! - **VNet subnets** — per-VNet allocations
//!
//! All pools use a `BTreeSet` free-list for O(log n) allocation and
//! deterministic address ordering.
//!
//! ## Design
//!
//! - One pool per CIDR
//! - First two addresses reserved (network, gateway)
//! - Last address reserved (broadcast)
//! - Released addresses return to the free set
//! - Subnet allocation finds the first aligned, contiguous block

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

/// An IPv4 CIDR (network address + prefix length).
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
        let mask = !0u32 << (32 - self.prefix);
        (u32::from(*ip) & mask) == (u32::from(self.network) & mask)
    }

    /// Get the broadcast address (last address in CIDR).
    pub fn broadcast(&self) -> Ipv4Addr {
        let bits = 32 - self.prefix as u32;
        let size = 1u32 << bits;
        Ipv4Addr::from(self.network_u32() + size - 1)
    }

    /// Get the network address (first address in CIDR).
    pub fn network_addr(&self) -> Ipv4Addr {
        self.network
    }

    /// Get the gateway address (first usable host).
    pub fn gateway(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.network_u32() + 1)
    }

    /// Get the last usable host address (one before broadcast).
    pub fn last_host(&self) -> Option<Ipv4Addr> {
        if self.prefix >= 31 {
            // /31 and /32 are point-to-point — every address is usable
            Some(self.broadcast())
        } else {
            let bcast = u32::from(self.broadcast());
            if bcast == 0 {
                None
            } else {
                Some(Ipv4Addr::from(bcast - 1))
            }
        }
    }

    /// Format as "address/prefix" string.
    pub fn to_string(&self) -> String {
        format!("{}/{}", self.network, self.prefix)
    }
}

impl std::fmt::Display for Ipv4Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.network, self.prefix)
    }
}

/// A free-list based IP pool for IPv4 addresses.
#[derive(Debug, Clone)]
pub struct IpPool {
    cidr: Ipv4Cidr,
    /// Set of free IP addresses as u32.
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
            // Point-to-point: every address is usable
            for i in 0..total {
                free.insert(network + i);
            }
        } else {
            // Skip network (.0), gateway (.1), and broadcast (last)
            for i in 2..(total - 1) {
                free.insert(network + i);
            }
        }
        Self { cidr, free }
    }

    /// Allocate the next available IP (deterministic, ascending order).
    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        let ip = self.free.pop_first()?;
        Some(Ipv4Addr::from(ip))
    }

    /// Return an IP to the pool.
    pub fn release(&mut self, ip: Ipv4Addr) {
        if self.cidr.contains(&ip) {
            self.free.insert(u32::from(ip));
        }
    }

    /// Number of free addresses remaining.
    pub fn count_free(&self) -> usize {
        self.free.len()
    }

    /// Total capacity (including reserved).
    pub fn capacity(&self) -> u32 {
        self.cidr.host_count() + 2
    }

    /// CIDR of this pool.
    pub fn cidr(&self) -> &Ipv4Cidr {
        &self.cidr
    }

    /// Add a contiguous CIDR range to the free set (for VNet expansion).
    pub fn expand(&mut self, new_cidr: &Ipv4Cidr) -> Result<(), String> {
        let old_network = self.cidr.network_u32();
        let old_size = 1u32 << (32 - self.cidr.prefix as u32);
        let new_network = new_cidr.network_u32();

        if new_network != old_network + old_size {
            return Err("New CIDR is not adjacent to current CIDR".to_string());
        }

        let bits = 32 - new_cidr.prefix as u32;
        for i in 0..(1u32 << bits) {
            self.free.insert(new_network + i);
        }
        Ok(())
    }

    /// Allocate a contiguous subnet block (aligned, sized to subnet_prefix).
    /// Returns the allocated CIDR or None if no aligned block is free.
    pub fn allocate_subnet(&mut self, subnet_prefix: u8) -> Option<Ipv4Cidr> {
        if subnet_prefix < self.cidr.prefix {
            return None; // can only allocate larger (smaller prefix) blocks
        }
        let subnet_size = 1u32 << (32 - subnet_prefix as u32);
        if (self.free.len() as u32) < subnet_size {
            return None;
        }
        let free_vec: Vec<u32> = self.free.iter().copied().collect();
        for window in free_vec.windows(subnet_size as usize) {
            let start = window[0];
            if (start & (subnet_size - 1)) != 0 {
                continue; // not aligned
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
    fn cidr_parse_misaligned_network_normalized() {
        let cidr = Ipv4Cidr::parse("10.42.5.13/20").unwrap();
        assert_eq!(cidr.network, Ipv4Addr::new(10, 42, 0, 0));
    }

    #[test]
    fn cidr_parse_invalid_prefix() {
        assert!(Ipv4Cidr::parse("10.0.0.0/33").is_none());
        assert!(Ipv4Cidr::parse("not-an-ip/24").is_none());
        assert!(Ipv4Cidr::parse("10.0.0.0").is_none()); // no prefix
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
    fn cidr_broadcast() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        assert_eq!(cidr.broadcast(), Ipv4Addr::new(10, 42, 0, 255));
    }

    #[test]
    fn cidr_gateway() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.gateway(), Ipv4Addr::new(10, 42, 0, 1));
    }

    #[test]
    fn cidr_last_host() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        assert_eq!(cidr.last_host(), Some(Ipv4Addr::new(10, 42, 0, 254)));
    }

    #[test]
    fn pool_slash_30_has_one_host() {
        // /30: 4 addresses: .0 (network), .1 (gateway), .2 (host), .3 (broadcast)
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
    fn pool_allocates_in_ascending_order() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/29").unwrap();
        let mut pool = IpPool::new(cidr);
        // /29: 8 addresses: .0 net, .1 gw, .2-.6 hosts, .7 bcast → 5 hosts
        let ip1 = pool.allocate().unwrap();
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip1, Ipv4Addr::new(10, 42, 0, 2));
        assert_eq!(ip2, Ipv4Addr::new(10, 42, 0, 3));
        assert_ne!(ip1, ip2);
    }

    #[test]
    fn pool_release_reuses_address() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/29").unwrap();
        let mut pool = IpPool::new(cidr);
        let ip1 = pool.allocate().unwrap();
        pool.release(ip1);
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn pool_release_outside_cidr_ignored() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        let mut pool = IpPool::new(cidr);
        let count_before = pool.count_free();
        pool.release(Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(pool.count_free(), count_before);
    }

    #[test]
    fn pool_allocate_subnet_aligned() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/16").unwrap();
        let mut pool = IpPool::new(cidr);
        let subnet = pool.allocate_subnet(24).unwrap();
        assert_eq!(subnet.prefix, 24);
        // Subnet must be aligned to /24 boundary
        assert_eq!(u32::from(subnet.network) & 0xFF, 0);
    }

    #[test]
    fn pool_capacity() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        let pool = IpPool::new(cidr);
        // /24: 256 addresses, host_count reports 254 (excludes network + broadcast),
        // but the pool additionally reserves .1 for the gateway, so 253 are free.
        assert_eq!(pool.capacity(), 256);
        assert_eq!(pool.count_free(), 253);
    }

    #[test]
    fn pool_expand_must_be_adjacent() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        let mut pool = IpPool::new(cidr);
        let bad = Ipv4Cidr::parse("10.43.0.0/24").unwrap();
        assert!(pool.expand(&bad).is_err());
    }

    #[test]
    fn pool_expand_adjacent_works() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/24").unwrap();
        let mut pool = IpPool::new(cidr);
        let adj = Ipv4Cidr::parse("10.42.1.0/24").unwrap();
        assert!(pool.expand(&adj).is_ok());
        assert!(pool.count_free() > 254);
    }

    #[test]
    fn cidr_to_string_roundtrip() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.to_string(), "10.42.0.0/20");
    }
}
