//! The other compute nodes this one has joined. Kept in `peers.json` in the state dir (readable by this user only).
//! A peer is a certificate fingerprint, a name and the address it was last reached at. Nothing about any job.

use crate::state::write_private;
use anyhow::Result;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub fp: String,
    pub name: String,
    /// `ip:port` of the peer's compute node port.
    pub addr: String,
    pub added_at: u64,
}

pub struct Peers {
    path: PathBuf,
    list: Mutex<Vec<Peer>>,
}

impl Peers {
    pub fn open(dir: &std::path::Path) -> Result<Peers> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("peers.json");
        let list = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Ok(Peers { path, list: Mutex::new(list) })
    }

    fn save(&self, list: &[Peer]) -> Result<()> {
        write_private(&self.path, &serde_json::to_vec_pretty(list)?)
    }

    pub fn list(&self) -> Vec<Peer> {
        self.list.lock().clone()
    }

    pub fn find(&self, fp: &str) -> Option<Peer> {
        self.list.lock().iter().find(|p| p.fp == fp).cloned()
    }

    /// Adds the peer, or replaces the one with the same fingerprint.
    pub fn add(&self, peer: Peer) -> Result<()> {
        let mut g = self.list.lock();
        g.retain(|p| p.fp != peer.fp);
        g.push(peer);
        self.save(&g)
    }

    /// Records where the peer was reached. Returns whether it changed.
    pub fn set_addr(&self, fp: &str, addr: &str) -> Result<bool> {
        let mut g = self.list.lock();
        match g.iter_mut().find(|p| p.fp == fp) {
            Some(p) if p.addr != addr => {
                p.addr = addr.to_string();
                self.save(&g)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Forgets the peers whose fingerprint starts with `who` (at least 4 characters) or whose name is exactly `who`.
    pub fn remove(&self, who: &str) -> Result<Vec<Peer>> {
        let mut g = self.list.lock();
        let (gone, kept): (Vec<Peer>, Vec<Peer>) = g.drain(..).partition(|p| (who.len() >= 4 && p.fp.starts_with(who)) || p.name == who);
        *g = kept;
        self.save(&g)?;
        Ok(gone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8, name: &str) -> Peer {
        Peer { fp: format!("{:02x}", n).repeat(32), name: name.into(), addr: format!("192.168.1.{n}:47615"), added_at: 1000 + n as u64 }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("marginalia-peers-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn peers_survive_a_restart() {
        let d = dir("round");
        let p = Peers::open(&d).unwrap();
        assert!(p.list().is_empty());
        p.add(peer(1, "falcon")).unwrap();
        p.add(peer(2, "heron")).unwrap();
        let again = Peers::open(&d).unwrap();
        assert_eq!(again.list(), vec![peer(1, "falcon"), peer(2, "heron")]);
        assert_eq!(again.find(&peer(2, "").fp), Some(peer(2, "heron")));
        assert_eq!(again.find("nope"), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn adding_the_same_node_again_replaces_it() {
        let d = dir("again");
        let p = Peers::open(&d).unwrap();
        p.add(peer(1, "falcon")).unwrap();
        let mut moved = peer(1, "falcon");
        moved.addr = "10.0.0.9:47615".into();
        p.add(moved.clone()).unwrap();
        assert_eq!(p.list(), vec![moved]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_moved_peer_gets_its_new_address() {
        let d = dir("moved");
        let p = Peers::open(&d).unwrap();
        p.add(peer(1, "falcon")).unwrap();
        assert!(!p.set_addr(&peer(1, "").fp, "192.168.1.1:47615").unwrap());
        assert!(p.set_addr(&peer(1, "").fp, "10.0.0.9:47615").unwrap());
        assert_eq!(Peers::open(&d).unwrap().list()[0].addr, "10.0.0.9:47615");
        assert!(!p.set_addr("unknown", "10.0.0.9:1").unwrap());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unjoin_by_name_or_id() {
        let d = dir("unjoin");
        let p = Peers::open(&d).unwrap();
        p.add(peer(1, "falcon")).unwrap();
        p.add(peer(2, "heron")).unwrap();
        p.add(peer(3, "otter")).unwrap();
        assert_eq!(p.remove("heron").unwrap().len(), 1);
        assert_eq!(p.remove("0303").unwrap().len(), 1);
        // Too short to be an id, and no such name.
        assert!(p.remove("01").unwrap().is_empty());
        assert!(p.remove("swan").unwrap().is_empty());
        assert_eq!(Peers::open(&d).unwrap().list(), vec![peer(1, "falcon")]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("mode");
        let p = Peers::open(&d).unwrap();
        p.add(peer(1, "falcon")).unwrap();
        let mode = std::fs::metadata(d.join("peers.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&d);
    }
}
