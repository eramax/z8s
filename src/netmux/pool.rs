use std::collections::BTreeSet;
use std::net::Ipv4Addr;

#[derive(Debug, Clone)]
pub struct IpPool {
    cidr: Ipv4Cidr,
    free: BTreeSet<u32>,
}

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
        if cidr.prefix == 32 {
            // /32 pool — single IP
            free.insert(network + 1);
        } else {
            for i in 1..((1u32 << bits) - 1) {
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

    pub fn cidr(&self) -> &Ipv4Cidr {
        &self.cidr
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
        assert_eq!(pool.count_free(), 2); // .1 and .2
        let ip1 = pool.allocate().unwrap();
        let _ip2 = pool.allocate().unwrap();
        assert!(pool.allocate().is_none());
        pool.release(ip1);
        assert_eq!(pool.count_free(), 1);
    }

    #[test]
    fn test_pool_deterministic_order() {
        let cidr = Ipv4Cidr::parse("10.42.0.0/30").unwrap();
        let mut pool = IpPool::new(cidr);
        let ip1 = pool.allocate().unwrap();
        let ip2 = pool.allocate().unwrap();
        assert_ne!(ip1, ip2);
        pool.release(ip1);
        pool.release(ip2);
        // Same allocation order after release
        let ip3 = pool.allocate().unwrap();
        assert_eq!(ip3, ip1);
    }
}
