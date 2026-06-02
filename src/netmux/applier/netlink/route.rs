use anyhow::{Context, Result};
use std::net::Ipv4Addr;

use super::link::get_ifindex;
use super::socket::{
    check_nl_response, nlattr, netlink_socket, recv_nlmsg, send_nlmsg, AF_INET, NLM_F_ACK,
    NLM_F_CREATE, NLM_F_REQUEST, RTA_DST, RTA_GATEWAY, RTA_OIF, RTM_DELROUTE, RTM_NEWROUTE,
    RTN_LOCAL, RTN_UNICAST, RTPROT_BOOT, RT_SCOPE_LINK, RT_SCOPE_UNIVERSE, RT_TABLE_MAIN,
};

pub fn add_local_service_cidr(dest: &Ipv4Addr, prefix: u8) -> Result<()> {
    let lo = get_ifindex("lo").context("get lo ifindex")?;
    add_route_with_type(dest, prefix, None, Some(lo), RTN_LOCAL)
}

fn add_route_with_type(
    dest: &Ipv4Addr,
    prefix: u8,
    gateway: Option<&Ipv4Addr>,
    oif: Option<u32>,
    rtn_type: u8,
) -> Result<()> {
    let fd = netlink_socket()?;

    let dest_bytes = dest.octets();
    let mut attrs: Vec<Vec<u8>> = Vec::new();
    if prefix > 0 {
        attrs.push(nlattr(RTA_DST, &u32::from_ne_bytes(dest_bytes)));
    }

    if let Some(gw) = gateway {
        let gw_bytes = gw.octets();
        attrs.push(nlattr(RTA_GATEWAY, &u32::from_ne_bytes(gw_bytes)));
    }
    if let Some(idx) = oif {
        attrs.push(nlattr(RTA_OIF, &idx));
    }

    let scope = if gateway.is_some() {
        RT_SCOPE_UNIVERSE
    } else {
        RT_SCOPE_LINK
    };
    let attrs_len: usize = attrs.iter().map(|a| a.len()).sum();
    let total_len = 16 + 12 + attrs_len;
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWROUTE.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20] = RT_TABLE_MAIN as u8;
    buf[21] = RTPROT_BOOT;
    buf[22] = scope;
    buf[23] = rtn_type;
    let rtm_flags: u32 = if gateway.is_some() { 4 } else { 0 };
    buf[24..28].copy_from_slice(&rtm_flags.to_ne_bytes());

    let mut offset = 28;
    for attr in &attrs {
        buf[offset..offset + attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "add_route_with_type")
}

pub fn add_route(
    dest: &Ipv4Addr,
    prefix: u8,
    gateway: Option<&Ipv4Addr>,
    oif: Option<u32>,
) -> Result<()> {
    add_route_with_type(dest, prefix, gateway, oif, RTN_UNICAST)
}

pub fn del_route(
    dest: &Ipv4Addr,
    prefix: u8,
    gateway: Option<&Ipv4Addr>,
    oif: Option<u32>,
) -> Result<()> {
    let fd = netlink_socket()?;

    let dest_bytes = dest.octets();
    let mut attrs: Vec<Vec<u8>> = Vec::new();
    if prefix > 0 {
        attrs.push(nlattr(RTA_DST, &u32::from_ne_bytes(dest_bytes)));
    }

    if let Some(gw) = gateway {
        let gw_bytes = gw.octets();
        attrs.push(nlattr(RTA_GATEWAY, &u32::from_ne_bytes(gw_bytes)));
    }
    if let Some(idx) = oif {
        attrs.push(nlattr(RTA_OIF, &idx));
    }

    let attrs_len: usize = attrs.iter().map(|a| a.len()).sum();
    let total_len = 16 + 12 + attrs_len;
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELROUTE.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = prefix;
    buf[18] = 0;
    buf[19] = 0;
    buf[20] = RT_TABLE_MAIN as u8;
    buf[21] = RTPROT_BOOT;
    buf[22] = RT_SCOPE_UNIVERSE;
    buf[23] = RTN_UNICAST;
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());

    let mut offset = 28;
    for attr in &attrs {
        buf[offset..offset + attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "del_route")
}
