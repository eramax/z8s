use anyhow::Result;

use super::socket::{
    check_nl_response, nlattr, nlattr_bytes, nlattr_nested, netlink_socket,
    recv_nlmsg, send_nlmsg, AF_INET, IFLA_IFNAME, IFLA_INFO_DATA, IFLA_INFO_KIND, IFLA_LINKINFO,
    IFLA_MTU, IFLA_NET_NS_PID, IFLA_VETH_PEER, IFF_UP, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL,
    NLM_F_REQUEST, RTM_DELLINK, RTM_GETLINK, RTM_NEWLINK,
};

pub fn create_veth_pair(
    host_name: &str,
    peer_name: &str,
    peer_pid: Option<u32>,
) -> Result<(u32, u32)> {
    let fd = netlink_socket()?;

    let mut peer_data = Vec::new();
    let peer_infomsg = vec![0u8; 16];
    peer_data.extend_from_slice(&peer_infomsg);
    peer_data.extend_from_slice(&nlattr_bytes(IFLA_IFNAME, peer_name.as_bytes()));
    if let Some(pid) = peer_pid {
        peer_data.extend_from_slice(&nlattr(IFLA_NET_NS_PID, &pid));
    }
    let peer_nested = nlattr_nested(IFLA_VETH_PEER, &peer_data);

    let mut info_data = Vec::new();
    info_data.extend_from_slice(&nlattr_bytes(IFLA_INFO_KIND, b"veth\0"));
    info_data.extend_from_slice(&nlattr_nested(IFLA_INFO_DATA, &peer_nested));
    let info_nested = nlattr_nested(IFLA_LINKINFO, &info_data);

    let ifname_attr = nlattr_bytes(IFLA_IFNAME, host_name.as_bytes());
    let mtu_attr = nlattr(IFLA_MTU, &1500u32);

    let payload_len = 16 + ifname_attr.len() + mtu_attr.len() + info_nested.len();
    let total_len = 16 + payload_len;
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8]
        .copy_from_slice(&(NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = 0;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&0i32.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    let mut offset = 32;
    for attr in [&ifname_attr, &mtu_attr, &info_nested] {
        buf[offset..offset + attr.len()].copy_from_slice(attr);
        offset += attr.len();
    }

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "create_veth")?;

    let host_idx = get_ifindex(host_name)?;
    let peer_idx = if peer_pid.is_some() {
        0
    } else {
        get_ifindex(peer_name)?
    };

    Ok((host_idx, peer_idx))
}

pub fn get_ifindex(name: &str) -> Result<u32> {
    let fd = netlink_socket()?;

    let name_attr = nlattr_bytes(IFLA_IFNAME, name.as_bytes());
    let total_len = 16 + 16 + name_attr.len();
    let mut buf = vec![0u8; total_len];

    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_GETLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&0i32.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    let offset = 32;
    buf[offset..offset + name_attr.len()].copy_from_slice(&name_attr);

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;

    if resp.len() >= 20 {
        let msg_type = u16::from_ne_bytes([resp[4], resp[5]]);
        if msg_type != super::socket::NLMSG_ERROR {
            let idx = u32::from_ne_bytes([resp[20], resp[21], resp[22], resp[23]]);
            if idx != 0 {
                return Ok(idx);
            }
        }
    }

    Err(anyhow::anyhow!("get_ifindex: interface {} not found", name))
}

pub fn set_link_up(ifindex: u32) -> Result<()> {
    let fd = netlink_socket()?;

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_NEWLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());
    buf[24..28].copy_from_slice(&(IFF_UP as u32).to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "set_link_up")
}

pub fn del_link(ifindex: u32) -> Result<()> {
    let fd = netlink_socket()?;

    let total_len = 16 + 16;
    let mut buf = vec![0u8; total_len];
    buf[0..4].copy_from_slice(&(total_len as u32).to_ne_bytes());
    buf[4..6].copy_from_slice(&RTM_DELLINK.to_ne_bytes());
    buf[6..8].copy_from_slice(&(NLM_F_REQUEST | NLM_F_ACK).to_ne_bytes());
    buf[8..12].copy_from_slice(&1u32.to_ne_bytes());
    buf[12..16].copy_from_slice(&0u32.to_ne_bytes());

    buf[16] = AF_INET as u8;
    buf[17] = 0;
    buf[18..20].copy_from_slice(&0u16.to_ne_bytes());
    buf[20..24].copy_from_slice(&ifindex.to_ne_bytes());
    buf[24..28].copy_from_slice(&0u32.to_ne_bytes());
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());

    send_nlmsg(&fd, &buf)?;
    let resp = recv_nlmsg(&fd)?;
    check_nl_response(&resp, "del_link")
}
