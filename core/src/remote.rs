//! Home computers: other machines on the same network that run `marginalia-node` and can do the AI
//! work faster than the phone. This is only ever a shortcut. Every call here answers "not handled"
//! when no computer is paired, switched on and reachable, and the phone then does the work itself
//! exactly as it did before.
//!
//! The nodes keep nothing: a request is sent, answered, and forgotten on their side.

use crate::engine::llm::{GenEvent, GenRequest, GenStats, Role};
use anyhow::{anyhow, bail, Result};
use marginalia_proto::tls::Identity;
use marginalia_proto::{client, wire, ChatEvent, ChatMsg, ChatRequest, EmbedReply, EmbedRequest, KnockReply, Priority, Request, StatusReply};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A computer is only used for chat when it is clearly faster than a phone (about 10 tokens a second).
const MIN_CHAT_TOK_S: f32 = 15.0;
/// And for search when it beats the phone's one passage a second by a wide margin.
const MIN_EMBED_PER_S: f32 = 4.0;
const DIAL: Duration = Duration::from_millis(2500);
/// After a failed attempt a computer is left alone for this long.
const RETRY_AFTER: Duration = Duration::from_secs(20);

static REMOTE: OnceLock<Arc<Remote>> = OnceLock::new();

pub fn get() -> Option<&'static Arc<Remote>> {
    REMOTE.get()
}

pub fn init(data_dir: &std::path::Path) -> Result<()> {
    if REMOTE.get().is_some() {
        return Ok(());
    }
    let r = Remote::open(data_dir.join("remote.json"))?;
    let _ = REMOTE.set(r);
    Ok(())
}

#[derive(Serialize, Deserialize, Clone)]
struct Saved {
    enabled: bool,
    identity: Option<Identity>,
    nodes: Vec<SavedNode>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SavedNode {
    fp: String,
    name: String,
    addrs: Vec<String>,
    /// False while the computer has not yet allowed this phone.
    #[serde(default = "yes")]
    approved: bool,
}

fn yes() -> bool {
    true
}

struct Live {
    conn: Option<quinn::Connection>,
    status: Option<StatusReply>,
    rtt_ms: u32,
    failed_at: Option<Instant>,
    checked_at: Option<Instant>,
    last_error: String,
    /// The computer turned this phone down.
    refused: bool,
}

struct Node {
    saved: SavedNode,
    live: Live,
}

struct Seen {
    name: String,
    id: String,
    addrs: Vec<SocketAddr>,
    at: Instant,
}

struct Inner {
    enabled: bool,
    nodes: Vec<Node>,
    seen: HashMap<String, Seen>,
}

pub struct Remote {
    rt: tokio::runtime::Runtime,
    ep: quinn::Endpoint,
    id: Identity,
    path: PathBuf,
    inner: Mutex<Inner>,
}

impl Remote {
    fn open(path: PathBuf) -> Result<Arc<Remote>> {
        let mut saved: Saved = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Saved { enabled: true, identity: None, nodes: vec![] });
        let id = match saved.identity.clone() {
            Some(i) => i,
            None => {
                let i = Identity::generate()?;
                saved.identity = Some(i.clone());
                i
            }
        };
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("remote").enable_all().build()?;
        let ep = rt.block_on(async { client::new_endpoint() })?;
        let nodes = saved
            .nodes
            .iter()
            .cloned()
            .map(|n| Node { saved: n, live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false } })
            .collect();
        let r = Arc::new(Remote { rt, ep, id, path, inner: Mutex::new(Inner { enabled: saved.enabled, nodes, seen: HashMap::new() }) });
        r.save();
        r.rt.spawn(monitor(r.clone()));
        Ok(r)
    }

    fn save(&self) {
        let g = self.inner.lock();
        let saved = Saved { enabled: g.enabled, identity: Some(self.id.clone()), nodes: g.nodes.iter().map(|n| n.saved.clone()).collect() };
        drop(g);
        if let Ok(b) = serde_json::to_vec(&saved) {
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.inner.lock().enabled = on;
        self.save();
    }

    pub fn enabled(&self) -> bool {
        self.inner.lock().enabled
    }

    // ------------------------------------------------------------------ discovery and pairing

    /// A computer the phone's network discovery just found.
    pub fn seen(&self, name: &str, id: &str, port: u16, addrs: &[String]) {
        let addrs: Vec<SocketAddr> = addrs.iter().filter_map(|a| a.parse::<std::net::IpAddr>().ok()).filter(|a| a.is_ipv4()).map(|a| SocketAddr::new(a, port)).collect();
        if addrs.is_empty() {
            return;
        }
        let mut g = self.inner.lock();
        // A paired computer that moved to a new address is followed.
        for n in g.nodes.iter_mut().filter(|n| id.len() >= 16 && n.saved.fp.starts_with(id)) {
            n.saved.addrs = addrs.iter().map(|a| a.to_string()).collect();
            n.live.failed_at = None;
        }
        g.seen.insert(id.to_string(), Seen { name: name.to_string(), id: id.to_string(), addrs, at: Instant::now() });
    }

    /// Pairs with a computer, given either its address or the id discovery reported, and the code
    /// the computer is showing.
    pub fn pair(self: &Arc<Self>, addr: Option<&str>, seen_id: Option<&str>, code: &str, phone_name: &str) -> Result<Value> {
        let mut candidates: Vec<SocketAddr> = vec![];
        if let Some(a) = addr.filter(|a| !a.trim().is_empty()) {
            let a = a.trim();
            let parsed = a.parse::<SocketAddr>().or_else(|_| format!("{a}:{}", marginalia_proto::DEFAULT_PORT).parse::<SocketAddr>());
            candidates.push(parsed.map_err(|_| anyhow!("that does not look like an address, try 192.168.1.10"))?);
        }
        if let Some(id) = seen_id {
            if let Some(s) = self.inner.lock().seen.get(id) {
                candidates.extend(s.addrs.iter().copied());
            }
        }
        if candidates.is_empty() {
            bail!("no address to try");
        }
        let (id, ep) = (self.id.clone(), self.ep.clone());
        let (code, phone) = (code.to_string(), phone_name.to_string());
        let out = self.rt.block_on(async move {
            let mut last = anyhow!("no answer");
            for a in candidates {
                match client::connect(&ep, &id, a, None, Duration::from_secs(4)).await {
                    Ok((conn, fp)) => {
                        let r = client::pair(&conn, &id, &fp, &phone, &code).await;
                        conn.close(0u32.into(), b"done");
                        match r {
                            Ok(name) => return Ok((fp, name, a)),
                            Err(e) => return Err(e),
                        }
                    }
                    Err(e) => last = e,
                }
            }
            Err(last)
        });
        let (fp, name, a) = out?;
        let mut g = self.inner.lock();
        g.nodes.retain(|n| n.saved.fp != fp);
        g.nodes.push(Node {
            saved: SavedNode { fp: fp.clone(), name: name.clone(), addrs: vec![a.to_string()], approved: true },
            live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false },
        });
        drop(g);
        self.save();
        self.refresh();
        Ok(json!({"fp": fp, "name": name}))
    }

    /// Asks a computer to let this phone use it, without a code. The person has already said they
    /// trust the computer. `seen_id` is the id the computer announced: if the certificate that answers
    /// does not match it, something else is at that address and nothing is saved.
    pub fn trust(self: &Arc<Self>, addr: Option<&str>, seen_id: Option<&str>, phone_name: &str) -> Result<Value> {
        let mut candidates: Vec<SocketAddr> = vec![];
        if let Some(a) = addr.map(str::trim).filter(|a| !a.is_empty()) {
            let parsed = a.parse::<SocketAddr>().or_else(|_| format!("{a}:{}", marginalia_proto::DEFAULT_PORT).parse::<SocketAddr>());
            candidates.push(parsed.map_err(|_| anyhow!("that does not look like an address, try 192.168.1.10"))?);
        }
        if let Some(id) = seen_id.filter(|i| !i.is_empty()) {
            if let Some(s) = self.inner.lock().seen.get(id) {
                candidates.extend(s.addrs.iter().copied());
            }
        }
        if candidates.is_empty() {
            bail!("no address to try");
        }
        let (id, ep) = (self.id.clone(), self.ep.clone());
        let (phone, want) = (phone_name.to_string(), seen_id.unwrap_or("").to_string());
        let out = self.rt.block_on(async move {
            let mut last = anyhow!("no answer");
            for a in candidates {
                match client::connect(&ep, &id, a, None, Duration::from_secs(4)).await {
                    Ok((conn, fp)) => {
                        if !want.is_empty() && !fp.starts_with(&want) {
                            conn.close(0u32.into(), b"mismatch");
                            return Err(anyhow!("a different computer answered than the one that was announced"));
                        }
                        let r = client::knock(&conn, &phone).await;
                        conn.close(0u32.into(), b"done");
                        return r.map(|k| (fp, k, a));
                    }
                    Err(e) => last = e,
                }
            }
            Err(last)
        });
        let (fp, reply, a) = out?;
        let (name, approved) = match reply {
            KnockReply::Accepted { node_name } => (node_name, true),
            KnockReply::Pending { node_name } => (node_name, false),
            KnockReply::Refused(why) => bail!(why),
        };
        let mut g = self.inner.lock();
        g.nodes.retain(|n| n.saved.fp != fp);
        g.nodes.push(Node {
            saved: SavedNode { fp: fp.clone(), name: name.clone(), addrs: vec![a.to_string()], approved },
            live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false },
        });
        drop(g);
        self.save();
        self.refresh();
        Ok(json!({"fp": fp, "name": name, "state": if approved { "approved" } else { "waiting" }}))
    }

    /// Answers another phone that is waiting at one of the computers this phone is paired with.
    pub fn decide(self: &Arc<Self>, fp: &str, who: &str, allow: bool) -> Result<()> {
        let conn = {
            let g = self.inner.lock();
            g.nodes.iter().find(|n| n.saved.fp == fp).and_then(|n| n.live.conn.clone()).filter(|c| c.close_reason().is_none())
        }
        .ok_or_else(|| anyhow!("that computer is not reachable right now"))?;
        let who = who.to_string();
        self.rt.block_on(async move { client::decide(&conn, &who, allow).await })?;
        self.refresh();
        Ok(())
    }

    pub fn forget(&self, fp: &str) {
        let mut g = self.inner.lock();
        for n in g.nodes.iter().filter(|n| n.saved.fp == fp) {
            if let Some(c) = &n.live.conn {
                c.close(0u32.into(), b"forgotten");
            }
        }
        g.nodes.retain(|n| n.saved.fp != fp);
        drop(g);
        self.save();
    }

    // ------------------------------------------------------------------ what the settings screen shows

    pub fn list(&self) -> Value {
        let g = self.inner.lock();
        let now = Instant::now();
        let paired: Vec<Value> = g
            .nodes
            .iter()
            .map(|n| {
                let online = n.live.conn.as_ref().map(|c| c.close_reason().is_none()).unwrap_or(false) && n.live.status.is_some();
                let caps = n.live.status.as_ref().map(|s| s.caps.clone()).unwrap_or_default();
                let state = if !n.saved.approved { if n.live.refused { "refused" } else { "waiting" } } else { "approved" };
                let pending: Vec<Value> = n.live.status.as_ref().map(|s| s.pending.iter().map(|p| json!({"id": p.id, "name": p.name, "waited_s": p.waited_s})).collect()).unwrap_or_default();
                json!({
                    "state": state, "pending": pending,
                    "fp": n.saved.fp, "name": n.saved.name, "online": online, "rtt_ms": n.live.rtt_ms,
                    "chat": caps.chat, "embed": caps.embed, "chat_tok_s": caps.chat_tok_s, "embed_per_s": caps.embed_per_s,
                    "busy": n.live.status.as_ref().map(|s| s.running + s.waiting).unwrap_or(0),
                    "problem": if online || state == "waiting" { String::new() } else { n.live.last_error.clone() },
                })
            })
            .collect();
        let found: Vec<Value> = g
            .seen
            .values()
            .filter(|s| now.duration_since(s.at) < Duration::from_secs(120))
            .filter(|s| !g.nodes.iter().any(|n| n.saved.fp.starts_with(&s.id)))
            .map(|s| json!({"id": s.id, "name": s.name, "addrs": s.addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>()}))
            .collect();
        json!({"enabled": g.enabled, "paired": paired, "found": found})
    }

    /// Looks at every paired computer now (answers when they have all replied or timed out).
    pub fn refresh(self: &Arc<Self>) {
        let targets: Vec<usize> = (0..self.inner.lock().nodes.len()).collect();
        self.rt.block_on(async {
            let mut set = tokio::task::JoinSet::new();
            for i in targets {
                let this = self.clone();
                set.spawn(async move { this.ensure_idx(i, true).await });
            }
            while set.join_next().await.is_some() {}
        });
    }

    // ------------------------------------------------------------------ doing work

    /// Chat on a computer, or None when the phone should do it.
    pub fn chat(self: &Arc<Self>, req: &GenRequest) -> Option<Result<GenStats>> {
        if !req.images.is_empty() || !self.enabled() {
            return None;
        }
        let t0 = Instant::now();
        let conn = self.rt.block_on(self.pick(Kind::Chat))?;
        let picked = t0.elapsed();
        let msgs = req
            .messages
            .iter()
            .map(|m| ChatMsg {
                role: match m.role {
                    Role::System => marginalia_proto::Role::System,
                    Role::User => marginalia_proto::Role::User,
                    Role::Assistant => marginalia_proto::Role::Assistant,
                },
                content: m.content.clone(),
            })
            .collect();
        let creq = ChatRequest {
            messages: msgs,
            max_tokens: req.params.max_tokens as u32,
            temperature: req.params.temperature,
            top_p: req.params.top_p,
            top_k: req.params.top_k,
            priority: if req.priority == crate::engine::llm::Priority::High { Priority::Interactive } else { Priority::Background },
        };
        let started = Instant::now();
        let r = self.rt.block_on(async {
            let (mut send, mut recv) = client::open(&conn.conn, &Request::Chat(creq)).await.map_err(|e| (false, e))?;
            let _ = send.finish();
            let mut got_tokens = false;
            let mut queued = false;
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                let wait = if got_tokens { Duration::from_secs(60) } else if queued { Duration::from_secs(180) } else { Duration::from_secs(30) };
                let ev = tokio::select! {
                    ev = tokio::time::timeout(wait, wire::read_frame::<ChatEvent>(&mut recv)) => match ev {
                        Ok(Ok(Some(e))) => e,
                        Ok(Ok(None)) => return Err((got_tokens, anyhow!("the computer stopped answering"))),
                        Ok(Err(e)) => return Err((got_tokens, e)),
                        Err(_) => return Err((got_tokens, anyhow!("the computer took too long"))),
                    },
                    _ = async { loop { tick.tick().await; if req.cancel.load(Ordering::Relaxed) { break; } } } => {
                        let _ = recv.stop(0u32.into());
                        return Ok(GenStats::default());
                    }
                };
                match ev {
                    ChatEvent::Queued { .. } => queued = true,
                    ChatEvent::Token(t) => {
                        got_tokens = true;
                        (req.on_event)(GenEvent::Token(t));
                    }
                    ChatEvent::Done { prompt_tokens, generated_tokens, elapsed_ms } => {
                        return Ok(GenStats { prompt_tokens: prompt_tokens as usize, generated_tokens: generated_tokens as usize, served_by: Some(conn.name.clone()), remote_ms: elapsed_ms as u64, ..Default::default() });
                    }
                    ChatEvent::Error(e) => return Err((got_tokens, anyhow!(e))),
                }
            }
        });
        match r {
            Ok(stats) => {
                tracing::info!("chat done on {}: picking {:.2}s, then {:.2}s ({} tokens)", conn.name, picked.as_secs_f32(), started.elapsed().as_secs_f32(), stats.generated_tokens);
                Some(Ok(stats))
            }
            Err((true, e)) => {
                self.mark_failed(&conn.fp, &e);
                Some(Err(e))
            }
            Err((false, e)) => {
                // Nothing reached the screen yet, so the phone can simply do it itself.
                tracing::warn!("{} could not do the chat ({e:#}), doing it here", conn.name);
                self.mark_failed(&conn.fp, &e);
                None
            }
        }
    }

    /// Embeddings on a computer, or None when the phone should do them.
    pub fn embed(self: &Arc<Self>, texts: &[String]) -> Option<Vec<Vec<f32>>> {
        if !self.enabled() || texts.is_empty() {
            return None;
        }
        let conn = self.rt.block_on(self.pick(Kind::Embed))?;
        let started = Instant::now();
        let r: Result<Vec<Vec<f32>>> = self.rt.block_on(async {
            let mut all = Vec::with_capacity(texts.len());
            for chunk in texts.chunks(64) {
                let (mut send, mut recv) = client::open(&conn.conn, &Request::Embed(EmbedRequest { texts: chunk.to_vec(), priority: Priority::Bulk })).await?;
                let _ = send.finish();
                loop {
                    match tokio::time::timeout(Duration::from_secs(120), wire::read_frame::<EmbedReply>(&mut recv)).await {
                        Err(_) => bail!("the computer took too long"),
                        Ok(r) => match r?.ok_or_else(|| anyhow!("the computer stopped answering"))? {
                            EmbedReply::Queued { .. } => {}
                            EmbedReply::Vectors(v) => {
                                if v.len() != chunk.len() {
                                    bail!("wrong number of vectors");
                                }
                                all.extend(v);
                                break;
                            }
                            EmbedReply::Error(e) => bail!(e),
                        },
                    }
                }
            }
            Ok(all)
        });
        match r {
            Ok(v) => {
                tracing::debug!("{} passages embedded on {} in {:.2}s", v.len(), conn.name, started.elapsed().as_secs_f32());
                Some(v)
            }
            Err(e) => {
                tracing::warn!("{} could not embed ({e:#}), doing it here", conn.name);
                self.mark_failed(&conn.fp, &e);
                None
            }
        }
    }

    fn mark_failed(&self, fp: &str, e: &anyhow::Error) {
        let mut g = self.inner.lock();
        if let Some(n) = g.nodes.iter_mut().find(|n| n.saved.fp == fp) {
            n.live.conn = None;
            n.live.status = None;
            n.live.failed_at = Some(Instant::now());
            n.live.last_error = format!("{e:#}");
        }
    }

    /// The best reachable computer for this kind of work.
    async fn pick(self: &Arc<Self>, kind: Kind) -> Option<Picked> {
        let count = self.inner.lock().nodes.len();
        if count == 0 {
            return None;
        }
        let mut set = tokio::task::JoinSet::new();
        for i in 0..count {
            let this = self.clone();
            set.spawn(async move { this.ensure_idx(i, false).await });
        }
        while set.join_next().await.is_some() {}
        let g = self.inner.lock();
        let mut best: Option<(u32, u32, Picked)> = None;
        for n in g.nodes.iter().filter(|n| n.saved.approved) {
            let (Some(conn), Some(st)) = (n.live.conn.clone(), n.live.status.as_ref()) else { continue };
            if conn.close_reason().is_some() {
                continue;
            }
            let ok = match kind {
                Kind::Chat => st.caps.chat.is_some() && (st.caps.chat_tok_s == 0.0 || st.caps.chat_tok_s >= MIN_CHAT_TOK_S),
                Kind::Embed => st.caps.embed.is_some() && st.caps.embed_dim == 768 && (st.caps.embed_per_s == 0.0 || st.caps.embed_per_s >= MIN_EMBED_PER_S),
            };
            if !ok {
                continue;
            }
            let key = (st.running + st.waiting, n.live.rtt_ms);
            if best.as_ref().map(|b| key < (b.0, b.1)).unwrap_or(true) {
                best = Some((key.0, key.1, Picked { conn, fp: n.saved.fp.clone(), name: n.saved.name.clone() }));
            }
        }
        best.map(|b| b.2)
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Chat,
    Embed,
}

struct Picked {
    conn: quinn::Connection,
    fp: String,
    name: String,
}

impl Remote {
    /// Makes sure node `i` has a live connection and a recent status. With `force` it looks again
    /// even if it looked a moment ago.
    async fn ensure_idx(self: Arc<Self>, i: usize, force: bool) {
        let r = &self;
        let (fp, addrs, fresh, skip) = {
            let g = r.inner.lock();
            let Some(n) = g.nodes.get(i) else { return };
            let mut addrs: Vec<SocketAddr> = n.saved.addrs.iter().filter_map(|a| a.parse().ok()).collect();
            for s in g.seen.values().filter(|s| n.saved.fp.starts_with(&s.id)) {
                for a in &s.addrs {
                    if !addrs.contains(a) {
                        addrs.insert(0, *a);
                    }
                }
            }
            let alive = n.live.conn.as_ref().map(|c| c.close_reason().is_none()).unwrap_or(false);
            let fresh = alive && n.live.checked_at.map(|t| t.elapsed() < Duration::from_secs(8)).unwrap_or(false);
            let skip = !force && n.live.failed_at.map(|t| t.elapsed() < RETRY_AFTER).unwrap_or(false);
            (n.saved.fp.clone(), addrs, fresh, skip)
        };
        if (fresh && !force) || skip {
            return;
        }
        let existing = {
            let g = r.inner.lock();
            g.nodes.get(i).and_then(|n| n.live.conn.clone()).filter(|c| c.close_reason().is_none())
        };
        let mut result: Result<(quinn::Connection, StatusReply, u32)> = Err(anyhow!("no address"));
        let mut conns: Vec<quinn::Connection> = existing.into_iter().collect();
        if conns.is_empty() {
            let mut set = tokio::task::JoinSet::new();
            for a in addrs.iter().copied() {
                let (ep, id, pin) = (self.ep.clone(), self.id.clone(), fp.clone());
                set.spawn(async move { client::connect(&ep, &id, a, Some(pin), DIAL).await.map(|(c, _)| (c, a)) });
            }
            let mut last = anyhow!("no address");
            let mut winner = None;
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Ok((c, a))) => {
                        winner = Some((c, a));
                        break;
                    }
                    Ok(Err(e)) => last = e,
                    Err(_) => {}
                }
            }
            set.abort_all();
            match winner {
                Some((c, a)) => {
                    let mut g = r.inner.lock();
                    if let Some(n) = g.nodes.get_mut(i) {
                        let s = a.to_string();
                        n.saved.addrs.retain(|x| *x != s);
                        n.saved.addrs.insert(0, s);
                    }
                    conns.push(c);
                }
                None => result = Err(last),
            }
        }
        let approved = r.inner.lock().nodes.get(i).map(|n| n.saved.approved).unwrap_or(true);
        if !approved {
            // This phone has knocked: see whether the computer has answered.
            let c = conns.pop();
            let reply = match &c {
                Some(c) => match tokio::time::timeout(Duration::from_secs(3), client::knock_status(c)).await {
                    Ok(Ok(k)) => Ok(k),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(anyhow!("no answer")),
                },
                None => Err(anyhow!("no answer")),
            };
            let mut g = r.inner.lock();
            if let Some(n) = g.nodes.get_mut(i) {
                n.live.checked_at = Some(Instant::now());
                match reply {
                    Ok(KnockReply::Accepted { .. }) => {
                        n.saved.approved = true;
                        n.live.refused = false;
                        n.live.conn = c.clone();
                    }
                    Ok(KnockReply::Pending { .. }) => {
                        n.live.conn = c.clone();
                        n.live.failed_at = None;
                    }
                    Ok(KnockReply::Refused(why)) => {
                        n.live.refused = true;
                        n.live.last_error = why;
                        n.live.conn = None;
                    }
                    Err(e) => {
                        n.live.conn = None;
                        n.live.failed_at = Some(Instant::now());
                        n.live.last_error = short_error(&e);
                    }
                }
            }
            let now_ok = g.nodes.get(i).map(|n| n.saved.approved).unwrap_or(false);
            drop(g);
            if !now_ok {
                return;
            }
            r.save();
            // Allowed just now: carry on with a normal status check on the same connection.
            conns.extend(c);
        }
        if let Some(c) = conns.pop() {
            let t = Instant::now();
            result = match tokio::time::timeout(Duration::from_secs(3), client::status(&c)).await {
                Ok(Ok(st)) => Ok((c, st, t.elapsed().as_millis() as u32)),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(anyhow!("no answer to the status request")),
            };
        }
        let mut g = r.inner.lock();
        let Some(n) = g.nodes.get_mut(i) else { return };
        n.live.checked_at = Some(Instant::now());
        match result {
            Ok((c, st, rtt)) => {
                if st.proto != marginalia_proto::PROTO_VERSION {
                    n.live.last_error = "needs a newer version of the app or the node".into();
                    n.live.conn = None;
                    n.live.status = None;
                } else {
                    n.live.conn = Some(c);
                    n.live.status = Some(st);
                    n.live.rtt_ms = rtt;
                    n.live.failed_at = None;
                    n.live.last_error.clear();
                }
            }
            Err(e) => {
                n.live.conn = None;
                n.live.status = None;
                n.live.failed_at = Some(Instant::now());
                n.live.last_error = short_error(&e);
            }
        }
    }
}

fn short_error(e: &anyhow::Error) -> String {
    let s = format!("{e:#}");
    if s.contains("blocked") {
        "This computer has blocked this phone".into()
    } else if s.contains("not the paired computer") {
        "A different computer answered at that address".into()
    } else if s.contains("not paired") || s.contains("closed") {
        "This phone is no longer paired with it".into()
    } else if s.contains("no answer") || s.contains("timed out") {
        "Not reachable on this network".into()
    } else {
        s
    }
}

/// Keeps an eye on the paired computers while the app is running: quickly while one is waiting to be
/// allowed, and every so often otherwise so the settings screen and the next job start from fresh facts.
async fn monitor(r: Arc<Remote>) {
    loop {
        let (count, waiting, enabled) = {
            let g = r.inner.lock();
            (g.nodes.len(), g.nodes.iter().any(|n| !n.saved.approved && !n.live.refused), g.enabled)
        };
        tokio::time::sleep(Duration::from_secs(if waiting { 4 } else { 20 })).await;
        if count == 0 || !(enabled || waiting) {
            continue;
        }
        for i in 0..count {
            let rr = r.clone();
            tokio::spawn(async move { rr.ensure_idx(i, true).await });
        }
    }
}
