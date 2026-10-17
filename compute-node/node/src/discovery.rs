//! Tells the local network this node exists, so phones find it without anyone typing an address.
//! The announcement carries a name and the protocol version, never anything about the owner's data.

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use marginalia_proto::SERVICE_TYPE;
use std::net::IpAddr;

/// Addresses on real networks: no loopback, none of the virtual ones containers and VMs add.
pub fn lan_addresses() -> Vec<IpAddr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter(|i| {
            let n = i.name.as_str();
            !(n.starts_with("docker") || n.starts_with("br-") || n.starts_with("veth") || n.starts_with("virbr") || n.starts_with("mpqemubr") || n.starts_with("lxc") || n.starts_with("tailscale"))
        })
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4())
        .collect()
}

pub fn advertise(name: &str, port: u16, fp: &str) -> Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    let host = format!("{}.local.", hostname::get()?.to_string_lossy());
    let addrs = lan_addresses();
    let props = [("v", marginalia_proto::PROTO_VERSION.to_string()), ("id", fp[..16].to_string()), ("name", name.to_string())];
    let info = ServiceInfo::new(SERVICE_TYPE, name, &host, &addrs[..], port, &props[..])?;
    daemon.register(info)?;
    Ok(daemon)
}
