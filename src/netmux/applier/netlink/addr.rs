use anyhow::Result;
use std::net::Ipv4Addr;

use super::socket::{
    check_nl_response, nlattr, netlink_socket, recv_nlmsg, send_nlmsg, AF_INET, IFA_ADDRESS,
    IFA_LOCAL, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST, RTM_NEWADDR,
};

pub fn add_addr(ifindex: u32, ip: &Ipv4Addr, prefix: u8) -> Result<()> {
    let fd = netlink_socket()?;

    let ip_bytes = ip.octets();
    let local_attr = nlattr(IFA_LOCAL, &u32::from_ne_bytes(ip_bytes));
    let addr_attr = nlattr(IFA_ADDRESS, &u32::from_ne_bytes(ip_bytes));

    let total_len = 16 + 8 + local_attr.len() + addr_attr.len();
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWADDR.to_ne_bytes());
    buf[6..8]
        .copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());

    let mut offset = 24;
    for attr in [&local_attr, &addr_attr] {
        buf[offset..offset + attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "add_addr")
}
