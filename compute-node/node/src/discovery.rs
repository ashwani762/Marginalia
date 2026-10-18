//! Tells the local network this node exists, so phones find it without anyone typing an address.
//! The announcement carries a name and the protocol version, never anything about the owner's data.

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use marginalia_proto::SERVICE_TYPE;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

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

/// True when both are IPv4 addresses that share their first three numbers (a home network).
fn same_network(a: &IpAddr, b: Option<IpAddr>) -> bool {
    match (a, b) {
        (IpAddr::V4(a), Some(IpAddr::V4(b))) => a.octets()[..3] == b.octets()[..3],
        _ => false,
    }
}

/// Another node seen announcing itself on the network. This is only for showing: nothing is trusted or joined because of it.
/// Becoming peers still needs a code from the other node.
#[derive(Clone, Debug, PartialEq)]
pub struct Nearby {
    /// The first 16 characters of its fingerprint, the same id `peers` shows.
    pub id: String,
    pub name: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    pub paused: bool,
}

impl Nearby {
    /// The address to give `join`.
    pub fn address(&self) -> Option<String> {
        self.addrs.first().map(|a| format!("{a}:{}", self.port))
    }
}

/// The nodes seen on the network right now, by service name. Fed by [`Advertiser::watch`].
#[derive(Default)]
pub struct NearbyList {
    seen: Mutex<HashMap<String, Nearby>>,
}

impl NearbyList {
    pub fn list(&self) -> Vec<Nearby> {
        let mut v: Vec<Nearby> = self.seen.lock().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        v
    }

    fn put(&self, fullname: &str, n: Nearby) {
        self.seen.lock().unwrap().insert(fullname.to_string(), n);
    }

    fn remove(&self, fullname: &str) {
        self.seen.lock().unwrap().remove(fullname);
    }

    /// A node by its id (or the start of it) or by its name, ignoring case.
    pub fn find(&self, who: &str) -> Option<Nearby> {
        let who = who.trim();
        if who.is_empty() {
            return None;
        }
        let lower = who.to_lowercase();
        self.list().into_iter().find(|n| n.name.to_lowercase() == lower || (who.len() >= 4 && n.id.starts_with(&lower)))
    }
}

/// What this node tells the network, kept so the announcement can be sent again when the node is paused, resumed or stopped.
pub struct Advertiser {
    daemon: ServiceDaemon,
    name: String,
    host: String,
    port: u16,
    fp: String,
    addrs: Vec<IpAddr>,
}

impl Advertiser {
    fn info(&self, paused: bool) -> Result<ServiceInfo> {
        let mut props = vec![("v", marginalia_proto::PROTO_VERSION.to_string()), ("id", self.fp[..16].to_string()), ("name", self.name.clone())];
        if paused {
            props.push(("paused", "1".to_string()));
        }
        Ok(ServiceInfo::new(SERVICE_TYPE, &self.name, &self.host, &self.addrs[..], self.port, &props[..])?)
    }

    /// Announces again with the new state, so a phone that is looking changes its light within a second.
    pub fn set_paused(&self, paused: bool) {
        if let Err(e) = self.info(paused).and_then(|i| Ok(self.daemon.register(i)?)) {
            tracing::debug!("could not announce the new state: {e:#}");
        }
    }

    /// Looks for other nodes on the network and keeps `list` up to date until the announcement is shut down. This node itself is left out.
    pub fn watch(&self, list: Arc<NearbyList>) {
        let rx = match self.daemon.browse(SERVICE_TYPE) {
            Ok(rx) => rx,
            Err(e) => {
                tracing::debug!("could not look for other nodes: {e:#}");
                return;
            }
        };
        let own = self.fp[..16].to_string();
        std::thread::spawn(move || {
            while let Ok(ev) = rx.recv() {
                match ev {
                    ServiceEvent::ServiceResolved(info) => {
                        let id = info.get_property_val_str("id").unwrap_or("").to_lowercase();
                        if id.len() != 16 || id == own {
                            continue;
                        }
                        let mut addrs: Vec<IpAddr> = info.get_addresses_v4().into_iter().map(|a| IpAddr::V4(*a)).filter(|a| !matches!(a, IpAddr::V4(v4) if v4.is_link_local())).collect();
                        // The address on the same network as this machine first, then the rest in a steady order.
                        addrs.sort_by_key(|a| (!same_network(a, default_route_address()), *a));
                        if addrs.is_empty() {
                            continue;
                        }
                        let name = info.get_property_val_str("name").unwrap_or("").trim().to_string();
                        list.put(info.get_fullname(), Nearby { id, name, addrs, port: info.get_port(), paused: info.get_property_val_str("paused") == Some("1") });
                    }
                    ServiceEvent::ServiceRemoved(_, full) => list.remove(&full),
                    _ => {}
                }
            }
        });
    }

    /// Says goodbye on the network and stops announcing. Waits a moment so the goodbye is sent before the process ends.
    pub fn goodbye(&self) {
        if let Ok(i) = self.info(false) {
            if let Ok(done) = self.daemon.unregister(i.get_fullname()) {
                let _ = done.recv_timeout(std::time::Duration::from_millis(500));
            }
        }
        let _ = self.daemon.shutdown();
    }
}

pub fn advertise(name: &str, port: u16, fp: &str) -> Result<Advertiser> {
    let a = Advertiser { daemon: ServiceDaemon::new()?, name: name.to_string(), host: format!("{}.local.", hostname::get()?.to_string_lossy()), port, fp: fp.to_string(), addrs: lan_addresses() };
    a.daemon.register(a.info(false)?)?;
    Ok(a)
}

#[cfg(test)]
mod nearby_tests {
    use super::*;

    fn node(name: &str, id: &str) -> Nearby {
        Nearby { id: id.into(), name: name.into(), addrs: vec!["192.168.1.7".parse().unwrap()], port: 47615, paused: false }
    }

    #[test]
    fn a_node_is_found_by_name_or_id_and_goes_when_it_says_goodbye() {
        let l = NearbyList::default();
        l.put("kitchen._marginalia._udp.local.", node("Kitchen node", "ca2d5dee00112233"));
        l.put("office._marginalia._udp.local.", node("Office node", "7d3cd26700112233"));
        assert_eq!(l.list().iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["Kitchen node", "Office node"]);
        assert_eq!(l.find("kitchen NODE").unwrap().id, "ca2d5dee00112233");
        assert_eq!(l.find("7d3cd267").unwrap().name, "Office node");
        assert!(l.find("7d").is_none(), "an id needs at least 4 characters");
        assert!(l.find("").is_none() && l.find("nobody").is_none());
        l.remove("kitchen._marginalia._udp.local.");
        assert!(l.find("Kitchen node").is_none());
        assert_eq!(l.find("Office node").unwrap().address().as_deref(), Some("192.168.1.7:47615"));
    }

    #[test]
    fn the_address_on_this_network_comes_first() {
        let on: IpAddr = "192.168.1.9".parse().unwrap();
        assert!(same_network(&"192.168.1.20".parse().unwrap(), Some(on)));
        assert!(!same_network(&"192.168.137.1".parse().unwrap(), Some(on)));
        assert!(!same_network(&"192.168.1.20".parse().unwrap(), None));
    }
}
