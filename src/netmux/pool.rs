use std::collections::BTreeSet;
use std::net::Ipv4Addr;

/// Unified IP pool allocator using a BTreeSet free-list.
/// Per-VNet for pod CIDRs, single for service CIDR.
/// Plan §5: one code path for pods, services, public IPs.
#[derive(Debug, Clone)]
pub struct IpPool {
    cidr: Ipv4Cidr,
    free: BTreeSet<u32>,
}

/// An IPv4 CIDR (network + prefix length).
#[derive(Debug, Clone)]
pub struct Ipv4Cidr {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Cidr {
    pub fn new(network: Ipv4Addr, prefix: u8) -> Self {
        Self { network, prefix }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let (ip_str, prefix_str) = s.split_once('/')?;
        let prefix: u8 = prefix_str.parse().ok()?;
        if prefix > 32 {
            return None;
        }
        let ip: Ipv4Addr = ip_str.parse().ok()?;
        let mask = !0u32 << (32 - prefix);
        let network_int = u32::from(ip) & mask;
        Some(Self {
            network: Ipv4Addr::from(network_int),
            prefix,
        })
    }

    pub fn network_u32(&self) -> u32 {
        u32::from(self.network)
    }

    pub fn host_count(&self) -> u32 {
        let bits = 32u32 - self.prefix as u32;
        (1u32 << bits) - 2 // exclude network & broadcast
    }

    pub fn contains(&self, ip: &Ipv4Addr) -> bool {
        let ip_int = u32::from(*ip);
        let mask = !0u32 << (32 - self.prefix);
        (ip_int & mask) == (u32::from(self.network) & mask)
    }
}

impl IpPool {
    pub fn new(cidr: Ipv4Cidr) -> Self {
        let network = cidr.network_u32();
        let bits = 32u32 - cidr.prefix as u32;
        let mut free = BTreeSet::new();
        let total = 1u32 << bits;
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

    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        let ip = self.free.pop_first()?;
        Some(Ipv4Addr::from(ip))
    }

    pub fn release(&mut self, ip: Ipv4Addr) {
        self.free.insert(u32::from(ip));
    }

    pub fn count_free(&self) -> usize {
        self.free.len()
    }

    /// Add a new CIDR range to the free set (for VNet expansion).
    /// Requires the new CIDR to be contiguous and adjacent to the current one.
    /// Returns an error if the new CIDR overlaps or is not adjacent.
    pub fn expand(&mut self, new_cidr: &Ipv4Cidr) -> Result<(), String> {
        let old_network = self.cidr.network_u32();
        let old_size = 1u32 << (32 - self.cidr.prefix as u32);
        let new_network = new_cidr.network_u32();
        let new_size = 1u32 << (32 - new_cidr.prefix as u32);

        // Check contiguity: new CIDR must start right after old CIDR
        if new_network != old_network + old_size {
            return Err("New CIDR is not adjacent to current CIDR".to_string());
        }

        let bits = 32 - new_cidr.prefix as u32;
        for i in 0..(1u32 << bits) {
            self.free.insert(new_network + i);
        }
        Ok(())
    }

    pub fn cidr(&self) -> &Ipv4Cidr {
        &self.cidr
    }

    /// Allocate a contiguous subnet from the pool.
    /// Returns the CIDR of the allocated block, or None if unavailable.
    pub fn allocate_subnet(&mut self, subnet_prefix: u8) -> Option<Ipv4Cidr> {
        let subnet_size = 1u32 << (32 - subnet_prefix as u32);
        let free_vec: Vec<u32> = self.free.iter().copied().collect();
        if (free_vec.len() as u32) < subnet_size {
            return None;
        }
        // Find first aligned contiguous block
        for window in free_vec.windows(subnet_size as usize) {
            let start = window[0];
            if (start & (subnet_size - 1)) != 0 {
                continue;
            }
            if window.iter().enumerate().all(|(i, &v)| v == start + i as u32) {
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
    fn test_ipv4cidr_parse() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.network, Ipv4Addr::new(10, 42, 0, 0));
        assert_eq!(cidr.prefix, 20);
    }

    #[test]
    fn test_ipv4cidr_host_count() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/20").unwrap();
        assert_eq!(cidr.host_count(), 4094);
    }

    #[test]
    fn test_pool_alloc_release() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/30").unwrap();
        let mut pool = IpPool::new(cidr);
        // /30 has 4 IPs: network=0, gateway=1 reserved, 2-3 usable, broadcast=3 excluded
        // So only IP .2 is available
        assert_eq!(pool.count_free(), 1);
        let ip1 = pool.allocate().unwrap();
        assert!(pool.allocate().is_none());
        pool.release(ip1);
        assert_eq!(pool.count_free(), 1);
    }

    #[test]
    fn test_pool_deterministic_order() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/29").unwrap();
        let mut pool = IpPool::new(cidr);
        // /29 has 8 IPs: network=0, gateway=1 reserved, 2-5 usable, broadcast=7 excluded
        let ip1 = pool.allocate().unwrap();
        let ip2 = pool.allocate().unwrap();
        assert_ne!(ip1, ip2);
        assert_eq!(format!("{}", ip1), "10.42.0.2");
        assert_eq!(format!("{}", ip2), "10.42.0.3");
        pool.release(ip1);
        pool.release(ip2);
        let ip3 = pool.allocate().unwrap();
        assert_eq!(ip3, ip1);
    }
}
