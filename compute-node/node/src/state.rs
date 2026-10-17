//! What the node remembers between runs: its own identity and which phones are paired or blocked. Nothing
//! about any job is ever written here.

use anyhow::Result;
use crate::records::Records;
use marginalia_proto::tls::Identity;
use marginalia_proto::{PhoneRecord, RecordKind};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
pub struct Client {
    pub fp: String,
    pub name: String,
    pub paired_at: u64,
}

/// A phone that is turned away for good, even when connecting is open or it knows a pairing code.
/// Kept as its own list so that, when nodes share their lists, a block spreads like any other record.
#[derive(Clone, Serialize, Deserialize)]
pub struct Blocked {
    pub fp: String,
    pub name: String,
    pub at: u64,
}

/// The phones this node lets in or turns away, kept as the records nodes copy to each other (see [`crate::records`]).
/// `records.json` is the truth. `clients.json` and `blocked.json` are written beside it as plain lists of the same
/// phones, and are also read at start, so a node from before the records keeps its phones.
pub struct Store {
    dir: PathBuf,
    records: Mutex<Records>,
    /// Woken when a record changes here or arrives from a peer, so the change is sent on at once.
    pub changed: tokio::sync::Notify,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Store> {
        std::fs::create_dir_all(&dir)?;
        let mut records = Records::from_list(std::fs::read(dir.join("records.json")).ok().and_then(|b| serde_json::from_slice::<Vec<PhoneRecord>>(&b).ok()).unwrap_or_default());
        // Phones listed by an older node count as records of the time they were added. A newer record wins over them.
        let clients: Vec<Client> = std::fs::read(dir.join("clients.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let blocked: Vec<Blocked> = std::fs::read(dir.join("blocked.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        for c in clients {
            records.put(PhoneRecord { fp: c.fp, name: c.name, kind: RecordKind::Allow, at_ms: c.paired_at * 1000 });
        }
        for b in blocked {
            records.put(PhoneRecord { fp: b.fp, name: b.name, kind: RecordKind::Block, at_ms: b.at * 1000 });
        }
        let store = Store { dir, records: Mutex::new(records), changed: tokio::sync::Notify::new() };
        store.save(&store.records.lock())?;
        Ok(store)
    }

    fn save(&self, r: &Records) -> Result<()> {
        write_private(&self.dir.join("records.json"), &serde_json::to_vec_pretty(&r.all())?)?;
        write_private(&self.dir.join("clients.json"), &serde_json::to_vec_pretty(&clients_of(r))?)?;
        write_private(&self.dir.join("blocked.json"), &serde_json::to_vec_pretty(&blocked_of(r))?)
    }

    /// Whether new phones have to be allowed. Kept as a file so it survives restarts.
    pub fn locked(&self) -> bool {
        self.dir.join("locked").exists()
    }

    pub fn set_locked(&self, on: bool) -> Result<()> {
        let f = self.dir.join("locked");
        if on {
            std::fs::write(f, b"")?;
        } else {
            let _ = std::fs::remove_file(f);
        }
        Ok(())
    }

    pub fn identity(&self) -> Result<Identity> {
        let path = self.dir.join("identity.json");
        if let Some(id) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            return Ok(id);
        }
        let id = Identity::generate()?;
        write_private(&path, &serde_json::to_vec(&id)?)?;
        Ok(id)
    }

    /// Writes new records, saves, and tells the peers.
    fn write(&self, mut g: parking_lot::MutexGuard<Records>, new: Vec<PhoneRecord>) -> Result<()> {
        for r in new {
            g.put(r);
        }
        let saved = self.save(&g);
        drop(g);
        self.changed.notify_one();
        saved
    }

    pub fn find(&self, fp: &str) -> Option<Client> {
        self.records.lock().allow_record(fp).map(client_of)
    }

    pub fn list(&self) -> Vec<Client> {
        clients_of(&self.records.lock())
    }

    pub fn add(&self, fp: String, name: String) -> Result<()> {
        let g = self.records.lock();
        let at_ms = g.next_at(&fp, now_ms());
        self.write(g, vec![PhoneRecord { fp, name, kind: RecordKind::Allow, at_ms }])
    }

    pub fn is_blocked(&self, fp: &str) -> bool {
        self.records.lock().is_blocked(fp)
    }

    pub fn blocked_list(&self) -> Vec<Blocked> {
        blocked_of(&self.records.lock())
    }

    /// Blocks a phone given the start of its id or its name, and forgets it as a paired phone.
    /// `extra` lists phones that are not paired but known (for example waiting ones): (fingerprint, name).
    pub fn block(&self, who: &str, extra: &[(String, String)]) -> Result<Vec<String>> {
        let g = self.records.lock();
        let mut found: Vec<(String, String)> = g.allowed().iter().map(|c| (c.fp.clone(), c.name.clone())).collect();
        found.extend(extra.iter().cloned());
        found.retain(|(fp, name)| fp.starts_with(who) || name == who);
        found.sort();
        found.dedup();
        if found.is_empty() {
            anyhow::bail!("no phone matches that id or name");
        }
        // A block and a revoke with the same time: the phone is turned away, and after an unblock it has to ask again.
        let now = now_ms();
        let new = found
            .iter()
            .flat_map(|(fp, name)| {
                let at_ms = g.next_at(fp, now);
                [RecordKind::Block, RecordKind::Revoke].map(|kind| PhoneRecord { fp: fp.clone(), name: name.clone(), kind, at_ms })
            })
            .collect();
        self.write(g, new)?;
        Ok(found.into_iter().map(|(_, n)| n).collect())
    }

    pub fn unblock(&self, who: &str) -> Result<usize> {
        let g = self.records.lock();
        let now = now_ms();
        let new: Vec<PhoneRecord> = g
            .blocked()
            .iter()
            .filter(|b| b.fp.starts_with(who) || b.name == who)
            .map(|b| PhoneRecord { fp: b.fp.clone(), name: b.name.clone(), kind: RecordKind::Unblock, at_ms: g.next_at(&b.fp, now) })
            .collect();
        let n = new.len();
        self.write(g, new)?;
        Ok(n)
    }

    /// Removes every phone whose fingerprint starts with `prefix` (or whose name is exactly `prefix`).
    pub fn revoke(&self, prefix: &str) -> Result<usize> {
        let g = self.records.lock();
        let now = now_ms();
        let new: Vec<PhoneRecord> = g
            .allowed()
            .iter()
            .filter(|c| c.fp.starts_with(prefix) || c.name == prefix)
            .map(|c| PhoneRecord { fp: c.fp.clone(), name: c.name.clone(), kind: RecordKind::Revoke, at_ms: g.next_at(&c.fp, now) })
            .collect();
        let n = new.len();
        self.write(g, new)?;
        Ok(n)
    }

    /// Every record, for a peer.
    pub fn records(&self) -> Vec<PhoneRecord> {
        self.records.lock().all()
    }

    /// Takes in the records a peer sent. Returns how many changed anything here. A change is sent on to the other
    /// peers, which stops once every node holds the same records.
    pub fn merge(&self, theirs: &[PhoneRecord]) -> Result<usize> {
        let mut g = self.records.lock();
        let adopted = g.merge(theirs);
        if adopted.is_empty() {
            return Ok(0);
        }
        let saved = self.save(&g);
        drop(g);
        self.changed.notify_one();
        saved?;
        Ok(adopted.len())
    }
}

fn client_of(r: &PhoneRecord) -> Client {
    Client { fp: r.fp.clone(), name: r.name.clone(), paired_at: r.at_ms / 1000 }
}

/// Oldest first, like the order phones were let in.
fn clients_of(r: &Records) -> Vec<Client> {
    let mut v: Vec<Client> = r.allowed().into_iter().map(client_of).collect();
    v.sort_by(|a, b| (a.paired_at, &a.fp).cmp(&(b.paired_at, &b.fp)));
    v
}

fn blocked_of(r: &Records) -> Vec<Blocked> {
    let mut v: Vec<Blocked> = r.blocked().into_iter().map(|b| Blocked { fp: b.fp.clone(), name: b.name.clone(), at: b.at_ms / 1000 }).collect();
    v.sort_by(|a, b| (a.at, &a.fp).cmp(&(b.at, &b.fp)));
    v
}

pub fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(n: u8) -> String {
        format!("{:02x}", n).repeat(32)
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("marginalia-store-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn phones_added_by_an_older_node_are_kept() {
        let d = dir("old");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("clients.json"), serde_json::to_vec(&vec![Client { fp: fp(1), name: "one".into(), paired_at: 100 }, Client { fp: fp(2), name: "two".into(), paired_at: 200 }]).unwrap()).unwrap();
        std::fs::write(d.join("blocked.json"), serde_json::to_vec(&vec![Blocked { fp: fp(3), name: "three".into(), at: 300 }]).unwrap()).unwrap();
        let s = Store::open(d.clone()).unwrap();
        assert_eq!(s.list().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["one", "two"]);
        assert!(s.find(&fp(2)).is_some());
        assert!(s.is_blocked(&fp(3)));
        assert_eq!(s.blocked_list()[0].at, 300);
        // After the first start it is all in the records, and a revoke stays a revoke across restarts.
        assert_eq!(s.revoke("one").unwrap(), 1);
        let again = Store::open(d.clone()).unwrap();
        assert!(again.find(&fp(1)).is_none());
        assert!(again.find(&fp(2)).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn block_unblock_and_revoke_work_as_before_on_one_node() {
        let d = dir("local");
        let s = Store::open(d.clone()).unwrap();
        s.add(fp(1), "one".into()).unwrap();
        s.add(fp(2), "two".into()).unwrap();
        assert_eq!(s.block("one", &[]).unwrap(), vec!["one"]);
        assert!(s.is_blocked(&fp(1)) && s.find(&fp(1)).is_none());
        assert!(s.block("nobody", &[]).is_err());
        // Unblocking lets the phone ask again, it does not bring the pairing back.
        assert_eq!(s.unblock("one").unwrap(), 1);
        assert!(!s.is_blocked(&fp(1)) && s.find(&fp(1)).is_none());
        assert_eq!(s.unblock("one").unwrap(), 0);
        assert_eq!(s.revoke("two").unwrap(), 1);
        assert_eq!(s.revoke("two").unwrap(), 0);
        s.add(fp(1), "one".into()).unwrap();
        assert!(s.find(&fp(1)).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_phone_allowed_on_one_node_is_allowed_on_the_other_and_a_block_follows() {
        let (da, db) = (dir("a"), dir("b"));
        let (a, b) = (Store::open(da.clone()).unwrap(), Store::open(db.clone()).unwrap());
        a.add(fp(1), "one".into()).unwrap();
        assert!(b.find(&fp(1)).is_none());
        assert_eq!(b.merge(&a.records()).unwrap(), 1);
        assert!(b.find(&fp(1)).is_some());
        // Nothing new the second time, so nothing is sent on.
        assert_eq!(b.merge(&a.records()).unwrap(), 0);
        a.block("one", &[]).unwrap();
        b.merge(&a.records()).unwrap();
        assert!(b.is_blocked(&fp(1)) && b.find(&fp(1)).is_none());
        // The other way round: b unblocks and a follows, whatever the clocks say.
        assert_eq!(b.unblock("one").unwrap(), 1);
        a.merge(&b.records()).unwrap();
        assert!(!a.is_blocked(&fp(1)));
        // The records are saved: a restart does not forget a block.
        a.block("one", &[(fp(1), "one".into())]).unwrap();
        assert!(Store::open(da.clone()).unwrap().is_blocked(&fp(1)));
        let _ = std::fs::remove_dir_all(&da);
        let _ = std::fs::remove_dir_all(&db);
    }
}
