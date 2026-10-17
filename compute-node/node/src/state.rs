//! What the node remembers between runs: its own identity and which phones are paired. Nothing
//! about any job is ever written here.

use anyhow::Result;
use marginalia_proto::tls::Identity;
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

pub struct Store {
    dir: PathBuf,
    clients: Mutex<Vec<Client>>,
    blocked: Mutex<Vec<Blocked>>,
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Store> {
        std::fs::create_dir_all(&dir)?;
        let clients = std::fs::read(dir.join("clients.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let blocked = std::fs::read(dir.join("blocked.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Ok(Store { dir, clients: Mutex::new(clients), blocked: Mutex::new(blocked) })
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

    pub fn find(&self, fp: &str) -> Option<Client> {
        self.clients.lock().iter().find(|c| c.fp == fp).cloned()
    }

    pub fn list(&self) -> Vec<Client> {
        self.clients.lock().clone()
    }

    pub fn add(&self, fp: String, name: String) -> Result<()> {
        let mut g = self.clients.lock();
        g.retain(|c| c.fp != fp);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        g.push(Client { fp, name, paired_at: now });
        write_private(&self.dir.join("clients.json"), &serde_json::to_vec_pretty(&*g)?)
    }

    pub fn is_blocked(&self, fp: &str) -> bool {
        self.blocked.lock().iter().any(|b| b.fp == fp)
    }

    pub fn blocked_list(&self) -> Vec<Blocked> {
        self.blocked.lock().clone()
    }

    /// Blocks a phone given the start of its id or its name, and forgets it as a paired phone.
    /// `extra` lists phones that are not paired but known (for example waiting ones): (fingerprint, name).
    pub fn block(&self, who: &str, extra: &[(String, String)]) -> Result<Vec<String>> {
        let mut found: Vec<(String, String)> = self.clients.lock().iter().map(|c| (c.fp.clone(), c.name.clone())).collect();
        found.extend(extra.iter().cloned());
        found.retain(|(fp, name)| fp.starts_with(who) || name == who);
        found.sort();
        found.dedup();
        if found.is_empty() {
            anyhow::bail!("no phone matches that id or name");
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut b = self.blocked.lock();
        for (fp, name) in &found {
            if !b.iter().any(|x| x.fp == *fp) {
                b.push(Blocked { fp: fp.clone(), name: name.clone(), at: now });
            }
        }
        write_private(&self.dir.join("blocked.json"), &serde_json::to_vec_pretty(&*b)?)?;
        drop(b);
        let mut c = self.clients.lock();
        c.retain(|x| !found.iter().any(|(fp, _)| *fp == x.fp));
        write_private(&self.dir.join("clients.json"), &serde_json::to_vec_pretty(&*c)?)?;
        Ok(found.into_iter().map(|(_, n)| n).collect())
    }

    pub fn unblock(&self, who: &str) -> Result<usize> {
        let mut b = self.blocked.lock();
        let before = b.len();
        b.retain(|x| !(x.fp.starts_with(who) || x.name == who));
        write_private(&self.dir.join("blocked.json"), &serde_json::to_vec_pretty(&*b)?)?;
        Ok(before - b.len())
    }

    /// Removes every phone whose fingerprint starts with `prefix` (or whose name is exactly `prefix`).
    pub fn revoke(&self, prefix: &str) -> Result<usize> {
        let mut g = self.clients.lock();
        let before = g.len();
        g.retain(|c| !(c.fp.starts_with(prefix) || c.name == prefix));
        write_private(&self.dir.join("clients.json"), &serde_json::to_vec_pretty(&*g)?)?;
        Ok(before - g.len())
    }
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
