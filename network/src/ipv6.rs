//! # IPv6 Address Pool
//!
//! IPv6 address pool for public IP assignment from a host /64 prefix.
//! Each pod gets a /128 from the prefix, identified by an Interface
//! Identifier (IID) in the last 8 bytes.
//!
//! ## Model
//!
//! ```text
//! Prefix: 2001:db8::/64
//!   IID=1: 2001:db8::1
//!   IID=2: 2001:db8::2
//!   ...
//! ```
//!
//! Released addresses return to a free list, so an address can be
//! reused after release.

use std::collections::HashSet;
use std::net::Ipv6Addr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// IPv6 address pool backed by a /64 prefix.
pub struct Ipv6Pool {
    prefix_bytes: [u8; 16],
    next_iid: AtomicU64,
    /// Set of released IIDs available for reuse.
    available: RwLock<HashSet<u64>>,
}

impl Ipv6Pool {
    /// Create a new pool from a prefix like "2001:db8::/64".
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
        // Mask the address to align with the prefix. We zero out the IID
        // portion (the last `(128 - prefix_len)/8` bytes fully, and the
        // remaining bits within the next byte with a single mask).
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
            next_iid: AtomicU64::new(1),
            available: RwLock::new(HashSet::new()),
        })
    }

    /// Allocate a new IPv6 address from the pool.
    pub fn allocate(&self) -> Ipv6Addr {
        // Check available pool first
        {
            let mut avail = self.available.write().unwrap_or_else(|e| e.into_inner());
            if let Some(&iid) = avail.iter().next() {
                avail.remove(&iid);
                return self.iid_to_addr(iid);
            }
        }
        // Allocate new
        let iid = self.next_iid.fetch_add(1, Ordering::Relaxed);
        self.iid_to_addr(iid)
    }

    /// Release an address back to the pool for reuse.
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

    /// Total addresses ever allocated (including in-use).
    pub fn next_iid(&self) -> u64 {
        self.next_iid.load(Ordering::Relaxed) - 1
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
    fn pool_allocates_sequential() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        let a2 = pool.allocate();
        assert_ne!(a1, a2);
        // First 8 bytes are the prefix
        assert_eq!(a1.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        assert_eq!(a2.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        // IIDs are 1 and 2
        assert_eq!(u64::from_be_bytes(a1.octets()[8..16].try_into().unwrap()), 1);
        assert_eq!(u64::from_be_bytes(a2.octets()[8..16].try_into().unwrap()), 2);
    }

    #[test]
    fn pool_release_reuses() {
        let pool = Ipv6Pool::new("2001:db8::/64").unwrap();
        let a1 = pool.allocate();
        pool.release(a1);
        let a2 = pool.allocate();
        assert_eq!(a1, a2);
        assert_eq!(pool.available_count(), 0);
    }

    #[test]
    fn pool_invalid_prefix_fails() {
        assert!(Ipv6Pool::new("2001:db8::").is_err()); // no prefix
        assert!(Ipv6Pool::new("not-an-ip/64").is_err());
        assert!(Ipv6Pool::new("2001:db8::/129").is_err());
    }

    #[test]
    fn pool_prefix_alignment() {
        // /120 prefix should mask out the last byte properly
        let pool = Ipv6Pool::new("2001:db8::abcd/64").unwrap();
        let a = pool.allocate();
        // The non-prefix portion is the last 8 bytes, all but first 8 are 0
        assert_eq!(a.octets()[0..8], [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
    }
}
