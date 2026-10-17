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
            !(n.starts_with("docker") || n.starts_with("br-") || n.starts_with("veth") || n.starts_with("virbr") || n.starts_with("mpqemubr") || n.starts_with("lxc") || n.starts_with("tailscale")
                // Windows names adapters in words: the virtual switches of WSL, Hyper-V, VMware and VirtualBox.
                || n.contains("WSL") || n.contains("Default Switch") || n.contains("VMware") || n.contains("VirtualBox") || n.contains("Host-Only"))
        })
        .map(|i| i.ip())
        .filter(|ip| ip.is_ipv4())
        // 169.254.x.x is what an adapter without a network gives itself, nothing a phone can reach.
        .filter(|ip| !matches!(ip, IpAddr::V4(v4) if v4.is_link_local()))
        .collect::<Vec<_>>()
        .pipe_default_first()
}

/// The address this machine uses to reach the internet. Nothing is sent: connecting a UDP socket only picks the route.
fn default_route_address() -> Option<IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:9").ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}

/// Puts `first` at the front when it is in the list. A phone tries the first address it is given, so the one on the network
/// the phone is on must come before the Windows hotspot adapter (192.168.137.1) or a VPN.
pub fn with_first(mut addrs: Vec<IpAddr>, first: Option<IpAddr>) -> Vec<IpAddr> {
    if let Some(pos) = first.and_then(|f| addrs.iter().position(|a| *a == f)) {
        let a = addrs.remove(pos);
        addrs.insert(0, a);
    }
    addrs
}

trait DefaultFirst {
    fn pipe_default_first(self) -> Vec<IpAddr>;
}

impl DefaultFirst for Vec<IpAddr> {
    fn pipe_default_first(self) -> Vec<IpAddr> {
        with_first(self, default_route_address())
    }
}

#[cfg(test)]
mod default_first_tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn the_default_route_address_moves_to_the_front() {
        let v = with_first(vec![ip("192.168.137.1"), ip("192.168.1.10")], Some(ip("192.168.1.10")));
        assert_eq!(v, vec![ip("192.168.1.10"), ip("192.168.137.1")]);
    }

    #[test]
    fn the_list_is_kept_when_the_route_is_unknown_or_not_listed() {
        let v = vec![ip("192.168.137.1"), ip("192.168.1.10")];
        assert_eq!(with_first(v.clone(), None), v);
        assert_eq!(with_first(v.clone(), Some(ip("10.0.0.5"))), v);
    }
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
