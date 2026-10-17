//! Home computers: other machines on the same network that run `marginalia-node` and can do the AI
//! work faster than the phone. This is only ever a shortcut. Every call here answers "not handled"
//! when no computer is paired, switched on and reachable, and the phone then does the work itself
//! exactly as it did before.
//!
//! The nodes keep nothing: a request is sent, answered, and forgotten on their side.

use crate::engine::llm::{GenEvent, GenRequest, GenStats, Role};
use anyhow::{anyhow, bail, Result};
use marginalia_proto::tls::Identity;
use marginalia_proto::{client, wire, AudioFrame, ChatEvent, ChatMsg, ChatRequest, EmbedReply, EmbedRequest, KnockReply, Priority, Request, StatusReply, TranscribeEvent, TranscribeRequest};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Counts a job as out on a computer for as long as it is held.
struct Out<'a> {
    r: &'a Remote,
    fp: String,
}

impl<'a> Out<'a> {
    fn new(r: &'a Remote, fp: &str) -> Out<'a> {
        *r.inflight.lock().entry(fp.to_string()).or_insert(0) += 1;
        Out { r, fp: fp.to_string() }
    }
}

impl Drop for Out<'_> {
    fn drop(&mut self) {
        if let Some(n) = self.r.inflight.lock().get_mut(&self.fp) {
            *n = n.saturating_sub(1);
        }
    }
}

/// What became of a chat job that was offered to the home computers.
pub enum ChatOutcome {
    /// A computer finished it (or it was cancelled), or it failed in a way that cannot be recovered.
    Done(Result<GenStats>),
    /// Every computer failed partway. These words were already shown, the phone carries on from them.
    Continue { partial: String },
}

/// The same chat with the answer so far added, asking for the rest. Used when a computer drops out
/// halfway and another one, or the phone, has to finish the answer.
pub fn continuation(messages: &[crate::engine::llm::ChatMessage], partial: &str) -> Vec<crate::engine::llm::ChatMessage> {
    let mut m = messages.to_vec();
    m.push(crate::engine::llm::ChatMessage::assistant(partial.to_string()));
    m.push(crate::engine::llm::ChatMessage::user(
        "Your answer was cut off. Carry on from exactly where it stopped, without repeating anything and without a new introduction.",
    ));
    m
}

/// Joins a resumed answer onto the words already shown. A model asked to carry on tends to say the last
/// sentence again, so the first words of the new text are held back until it is clear whether they repeat
/// the end of what was shown, and any repeat is dropped.
pub struct SeamFilter {
    tail: String,
    buf: String,
    released: bool,
}

impl SeamFilter {
    const HOLD: usize = 100;
    const MIN_OVERLAP: usize = 12;

    pub fn new(shown: &str) -> SeamFilter {
        let start = shown.char_indices().rev().nth(239).map(|(i, _)| i).unwrap_or(0);
        SeamFilter { tail: shown[start..].to_string(), buf: String::new(), released: false }
    }

    pub fn push(&mut self, text: &str, emit: &mut dyn FnMut(&str)) {
        if self.released {
            emit(text);
            return;
        }
        self.buf.push_str(text);
        if self.buf.chars().count() >= Self::HOLD {
            self.release(emit);
        }
    }

    pub fn finish(&mut self, emit: &mut dyn FnMut(&str)) {
        if !self.released {
            self.release(emit);
        }
    }

    fn release(&mut self, emit: &mut dyn FnMut(&str)) {
        self.released = true;
        let buf = std::mem::take(&mut self.buf);
        let mut rest = buf.as_str();
        // The longest end of what was shown that the new text starts with.
        let starts: Vec<usize> = self.tail.char_indices().map(|(i, _)| i).collect();
        for &i in &starts {
            let suffix = &self.tail[i..];
            if suffix.chars().count() < Self::MIN_OVERLAP {
                break;
            }
            if rest.starts_with(suffix) {
                rest = &rest[suffix.len()..];
                break;
            }
        }
        if rest.is_empty() {
            return;
        }
        // The shown text may stop in the middle of a word or right after a sentence: keep words apart.
        let last = self.tail.chars().last();
        let first = rest.chars().next();
        if let (Some(l), Some(f)) = (last, first) {
            let needs_space = !l.is_whitespace() && !f.is_whitespace() && f.is_alphanumeric() && (".!?:;,)\"".contains(l) || (l.is_alphanumeric() && f.is_uppercase()));
            if needs_space {
                emit(" ");
            }
        }
        emit(rest);
    }
}

/// The node sends something every two seconds while it works. Silence for this long means the link is gone.
const STALL: Duration = Duration::from_secs(6);

/// A node that gives fewer words a second than this over a 5 second stretch is replaced mid-answer.
/// The phone's own model does about 10 to 15, so below this a switch is a gain.
const SLOW_TOK_S: f32 = 4.0;
const SPEED_WINDOW: Duration = Duration::from_secs(5);
/// How long a slow node is left alone.
const SLOW_PAUSE: Duration = Duration::from_secs(120);

/// A live node that produces no words for this long is stuck.
const NO_PROGRESS: Duration = Duration::from_secs(120);

/// A computer is only used for chat when it is clearly faster than a phone (about 10 tokens a second).
const MIN_CHAT_TOK_S: f32 = 15.0;
/// And for search when it beats the phone's one passage a second by a wide margin.
const MIN_EMBED_PER_S: f32 = 4.0;
/// A node is only given recorded audio when it listens at least this many times faster than real time.
/// (0 means it has not measured itself yet, which is let through.)
const MIN_TRANSCRIBE_X: f32 = 1.0;
/// Used to rank a node that has not measured how fast it listens.
const UNMEASURED_TRANSCRIBE_X: f32 = 2.0;
/// A live node that sends only heartbeats for this long, with no new text, is stuck.
const TRANSCRIBE_NO_PROGRESS: Duration = Duration::from_secs(600);
/// When a node drops out and less than this much audio is left, the rest is not worth another upload.
const TRANSCRIBE_TAIL_MS: u64 = 4000;
/// Biggest piece of audio put in one frame (the node accepts about 1 MB).
const AUDIO_FRAME_MAX: usize = 1 << 20;
const DIAL: Duration = Duration::from_millis(2500);
/// After a failed attempt a computer is left alone for this long.
const RETRY_AFTER: Duration = Duration::from_secs(20);

/// Audio for a node: 16 kHz mono 16 bit little endian, in pieces.
pub type PcmFrames = Box<dyn Iterator<Item = Vec<u8>> + Send>;

pub struct TranscribeJob<'a> {
    /// Length of all the audio.
    pub total_ms: u64,
    /// ISO code if known, else the node decides.
    pub language: Option<String>,
    pub cancel: &'a std::sync::atomic::AtomicBool,
    /// `Background` for a whole recording, `Interactive` for a short window that a person is waiting on.
    pub priority: Priority,
    /// What was said just before this audio (a short window only), so the node starts with the right names.
    pub prompt: Option<String>,
}

/// What came back from the compute nodes. Times are milliseconds from the start of the audio.
#[derive(Debug, Default)]
pub struct TranscribeOutcome {
    pub segments: Vec<(u64, u64, String)>,
    pub language: Option<String>,
    /// A node told us it had listened to everything. When false, `segments` stop early.
    pub complete: bool,
    /// The node that did the last part.
    pub node: String,
}

impl TranscribeOutcome {
    /// Where the text stops, in milliseconds.
    pub fn covered_ms(&self) -> u64 {
        resume_point_ms(&self.segments)
    }
}

enum Attempt {
    Finished,
    Cancelled,
}

/// Where the next node has to pick up: right after the last finished stretch of speech.
pub fn resume_point_ms(segments: &[(u64, u64, String)]) -> u64 {
    segments.iter().map(|s| s.1).max().unwrap_or(0)
}

/// Not worth another upload: only a few seconds of audio are left.
pub fn tail_is_negligible(total_ms: u64, from_ms: u64) -> bool {
    total_ms.saturating_sub(from_ms) < TRANSCRIBE_TAIL_MS
}

/// A segment the node timed from the start of the audio it was sent, moved onto the time line of the whole recording.
pub fn shift_segment(start_ms: u64, t0_ms: u64, t1_ms: u64) -> (u64, u64) {
    let t0 = start_ms + t0_ms;
    (t0, (start_ms + t1_ms).max(t0))
}

/// The node sends something every two seconds while it works. Nothing from it for the STALL time, while nothing is
/// going up to it either, means the link is gone.
pub fn link_is_lost(since_event: Duration, since_upload: Duration) -> bool {
    since_event > STALL && since_upload > STALL
}

/// The longest window, in seconds, a node treats as a quick job. A node that does not say (0) gets 120.
pub fn node_window_s(stated: u32) -> u32 {
    if stated == 0 { 120 } else { stated }
}

/// Whether a node's last status says it can take recorded audio.
pub fn can_transcribe(st: &StatusReply) -> bool {
    !st.paused && st.caps.transcribe.is_some() && (st.caps.transcribe_x == 0.0 || st.caps.transcribe_x >= MIN_TRANSCRIBE_X)
}

/// How long a new listening job would wait on a node, in milliseconds: the jobs ahead plus this one,
/// divided by how many times faster than real time it listens (per unit of audio length).
pub fn transcribe_wait(transcribe_x: f32, ahead: u32) -> u32 {
    let x = if transcribe_x > 0.0 { transcribe_x } else { UNMEASURED_TRANSCRIBE_X };
    ((ahead + 1) as f32 / x * 1000.0) as u32
}

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
    /// The compute node turned this phone down.
    refused: bool,
    /// Tokens a second this phone actually got from the node on recent answers (0 when not known yet).
    observed_chat: f32,
    /// A node that turned slow is left alone until then.
    slow_until: Option<Instant>,
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
    started: Instant,
    last_kick: std::sync::atomic::AtomicU64,
    /// Jobs this phone has out on each computer right now, by fingerprint. A computer's own status can
    /// be a few seconds old, so the phone counts its own jobs too when it chooses where to send the next.
    inflight: Mutex<HashMap<String, u32>>,
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
            .map(|n| Node { saved: n, live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false, observed_chat: 0.0, slow_until: None } })
            .collect();
        let r = Arc::new(Remote { started: Instant::now(), last_kick: std::sync::atomic::AtomicU64::new(0), inflight: Mutex::new(HashMap::new()), rt, ep, id, path, inner: Mutex::new(Inner { enabled: saved.enabled, nodes, seen: HashMap::new() }) });
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
    pub fn seen(self: &Arc<Self>, name: &str, id: &str, port: u16, addrs: &[String]) {
        let addrs: Vec<SocketAddr> = addrs.iter().filter_map(|a| a.parse::<std::net::IpAddr>().ok()).filter(|a| a.is_ipv4()).map(|a| SocketAddr::new(a, port)).collect();
        if addrs.is_empty() {
            return;
        }
        let mut g = self.inner.lock();
        // A paired compute node that moved to a new address is followed.
        for n in g.nodes.iter_mut().filter(|n| id.len() >= 16 && n.saved.fp.starts_with(id)) {
            n.saved.addrs = addrs.iter().map(|a| a.to_string()).collect();
            n.live.failed_at = None;
        }
        g.seen.insert(id.to_string(), Seen { name: name.to_string(), id: id.to_string(), addrs, at: Instant::now() });
        // A paired node that is announcing itself again is looked at right away, so work waiting for one starts sooner.
        let known_offline = g.nodes.iter().any(|n| id.len() >= 16 && n.saved.fp.starts_with(id) && n.live.conn.as_ref().map(|c| c.close_reason().is_some()).unwrap_or(true));
        drop(g);
        if known_offline {
            self.kick();
        }
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
            live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false, observed_chat: 0.0, slow_until: None },
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
                            return Err(anyhow!("a different device answered than the compute node that was announced"));
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
            live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false, observed_chat: 0.0, slow_until: None },
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
                    "chat": caps.chat, "embed": caps.embed, "transcribe": caps.transcribe, "transcribe_x": caps.transcribe_x, "window_max_s": node_window_s(caps.window_max_s), "chat_tok_s": caps.chat_tok_s, "embed_per_s": caps.embed_per_s,
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

    /// Looks at every paired compute node now (answers when they have all replied or timed out).
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

    /// Chat on a home computer, or None when the phone should do it from the start.
    ///
    /// Computers are tried best first. If one fails before the first word, the next one is tried. If
    /// one fails halfway through an answer, the next one is asked to carry on from the words already
    /// sent (they are shown to the person, so they cannot be taken back). When every computer has
    /// failed, [`ChatOutcome::Continue`] hands the words so far back so the phone can carry on itself.
    pub fn chat(self: &Arc<Self>, req: &GenRequest) -> Option<ChatOutcome> {
        self.chat_from(req, "")
    }

    /// Like [`Self::chat`], for an answer that is already partly written (by the phone, or by a node that
    /// dropped out): the first node asked carries on from `start`.
    pub fn chat_from(self: &Arc<Self>, req: &GenRequest, start: &str) -> Option<ChatOutcome> {
        if !req.images.is_empty() || !self.enabled() {
            return None;
        }
        let candidates = self.rt.block_on(self.pick_all(Kind::Chat));
        if candidates.is_empty() {
            return None;
        }
        let mut partial = start.to_string();
        for cand in candidates {
            if req.cancel.load(Ordering::Relaxed) {
                return Some(ChatOutcome::Done(Ok(GenStats::default())));
            }
            let messages = if partial.is_empty() { req.messages.clone() } else { continuation(&req.messages, &partial) };
            let _out = Out::new(self, &cand.fp);
            let started = Instant::now();
            let before = partial.len();
            match self.rt.block_on(self.chat_on(&cand, req, &messages, &mut partial)) {
                Ok(stats) => {
                    tracing::info!("chat done on {}: {:.2}s ({} tokens)", cand.name, started.elapsed().as_secs_f32(), stats.generated_tokens);
                    return Some(ChatOutcome::Done(Ok(stats)));
                }
                Err(e) => {
                    let kept = partial.len() - before;
                    tracing::warn!("{} stopped the chat ({e:#}), {kept} bytes had arrived", cand.name);
                    self.mark_failed(&cand.fp, &e);
                }
            }
        }
        if partial.is_empty() { None } else { Some(ChatOutcome::Continue { partial }) }
    }

    /// True when a compute node could take a chat job right now. While none is up this also looks for
    /// one, at most every 5 seconds, so a node that comes back is noticed while the phone is working.
    pub fn chat_ready(self: &Arc<Self>) -> bool {
        if !self.enabled() {
            return false;
        }
        if self.chat_parallelism() > 0 {
            return true;
        }
        self.kick();
        false
    }

    /// Looks at every paired node in the background (rate limited).
    fn kick(self: &Arc<Self>) {
        let now = self.started.elapsed().as_millis() as u64;
        let last = self.last_kick.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 5000 || self.inner.lock().nodes.is_empty() {
            return;
        }
        self.last_kick.store(now.max(1), Ordering::Relaxed);
        let count = self.inner.lock().nodes.len();
        for i in 0..count {
            let r = self.clone();
            self.rt.spawn(async move { r.ensure_idx(i, true).await });
        }
    }

    /// One attempt on one computer. Every word that arrives is passed on and added to `partial`.
    async fn chat_on(&self, node: &Picked, req: &GenRequest, messages: &[crate::engine::llm::ChatMessage], partial: &mut String) -> Result<GenStats> {
        let msgs = messages
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
        let (mut send, mut recv) = client::open(&node.conn, &Request::Chat(creq)).await?;
        let _ = send.finish();
        let mut seam = if partial.is_empty() { None } else { Some(SeamFilter::new(partial)) };
        let result = self.read_chat(node, req, &mut recv, partial, &mut seam).await;
        // Whatever the filter was still holding is part of the answer so far.
        if let Some(f) = seam.as_mut() {
            f.finish(&mut |t| {
                partial.push_str(t);
                (req.on_event)(GenEvent::Token(t.to_string()));
            });
        }
        result
    }

    async fn read_chat(&self, node: &Picked, req: &GenRequest, recv: &mut quinn::RecvStream, partial: &mut String, seam: &mut Option<SeamFilter>) -> Result<GenStats> {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut last_progress = Instant::now();
        // Speed watch: words counted over 5 second windows once they are flowing.
        let mut window = (Instant::now(), 0u32);
        let mut first_token: Option<Instant> = None;
        let mut tokens = 0u32;
        loop {
            // While words are flowing a pause of more than a few seconds means the link is gone. Before
            // the first word the computer may be reading a long prompt, so it gets longer.
            // The node sends something at least every two seconds while it is alive, so a few seconds of
            // silence means it is gone. A long wait with a live node is judged by `last_progress` below.
            let wait = STALL;
            let ev = tokio::select! {
                ev = tokio::time::timeout(wait, wire::read_frame::<ChatEvent>(recv)) => match ev {
                    Ok(Ok(Some(e))) => e,
                    Ok(Ok(None)) => bail!("the computer stopped answering"),
                    Ok(Err(e)) => return Err(e),
                    Err(_) => bail!("the computer took too long"),
                },
                _ = async { loop { tick.tick().await; if req.cancel.load(Ordering::Relaxed) { break; } } } => {
                    let _ = recv.stop(0u32.into());
                    return Ok(GenStats::default());
                }
            };
            match ev {
                ChatEvent::Queued { .. } | ChatEvent::Working => {
                    // Alive but not producing. A node that has been alive and silent of words for this
                    // long is stuck, so it is given up on.
                    if last_progress.elapsed() > NO_PROGRESS {
                        bail!("the computer is not making progress");
                    }
                }
                ChatEvent::Token(t) => {
                    last_progress = Instant::now();
                    tokens += 1;
                    if first_token.is_none() {
                        first_token = Some(Instant::now());
                        window = (Instant::now(), 0);
                    }
                    window.1 += 1;
                    if window.0.elapsed() >= SPEED_WINDOW {
                        let rate = window.1 as f32 / window.0.elapsed().as_secs_f32();
                        if rate < SLOW_TOK_S {
                            self.mark_slow(&node.fp, rate);
                            bail!("the compute node slowed down to {rate:.1} words a second");
                        }
                        window = (Instant::now(), 0);
                    }
                    match seam.as_mut() {
                        Some(f) => f.push(&t, &mut |x| {
                            partial.push_str(x);
                            (req.on_event)(GenEvent::Token(x.to_string()));
                        }),
                        None => {
                            partial.push_str(&t);
                            (req.on_event)(GenEvent::Token(t));
                        }
                    }
                }
                ChatEvent::Done { prompt_tokens, generated_tokens, elapsed_ms } => {
                    if let Some(f) = seam.as_mut() {
                        f.finish(&mut |x| {
                            partial.push_str(x);
                            (req.on_event)(GenEvent::Token(x.to_string()));
                        });
                    }
                    // What the phone really got, from the first word to the last.
                    if let Some(t0) = first_token {
                        if tokens >= 30 {
                            self.record_speed(&node.fp, tokens as f32 / t0.elapsed().as_secs_f32().max(0.05));
                        }
                    }
                    return Ok(GenStats { prompt_tokens: prompt_tokens as usize, generated_tokens: generated_tokens as usize, served_by: Some(node.name.clone()), remote_ms: elapsed_ms as u64, ..Default::default() });
                }
                ChatEvent::Error(e) => bail!(e),
            }
        }
    }

    /// Embeddings on a home computer, or None when the phone should do them. Computers are tried best
    /// first. Embedding a batch again is harmless, so a failure simply moves on.
    pub fn embed(self: &Arc<Self>, texts: &[String]) -> Option<Vec<Vec<f32>>> {
        if !self.enabled() || texts.is_empty() {
            return None;
        }
        for cand in self.rt.block_on(self.pick_all(Kind::Embed)) {
            let _out = Out::new(self, &cand.fp);
            let started = Instant::now();
            let r: Result<Vec<Vec<f32>>> = self.rt.block_on(async {
                let mut all = Vec::with_capacity(texts.len());
                for chunk in texts.chunks(64) {
                    let (mut send, mut recv) = client::open(&cand.conn, &Request::Embed(EmbedRequest { texts: chunk.to_vec(), priority: Priority::Bulk })).await?;
                    let _ = send.finish();
                    loop {
                        match tokio::time::timeout(STALL, wire::read_frame::<EmbedReply>(&mut recv)).await {
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
                    tracing::debug!("{} passages embedded on {} in {:.2}s", v.len(), cand.name, started.elapsed().as_secs_f32());
                    return Some(v);
                }
                Err(e) => {
                    tracing::warn!("{} could not embed ({e:#}), trying the next one", cand.name);
                    self.mark_failed(&cand.fp, &e);
                }
            }
        }
        None
    }

    /// The name of a compute node that could transcribe recorded audio right now, from what was last seen
    /// (the fastest one when several can).
    pub fn transcribe_node_available(&self) -> Option<String> {
        self.best_transcribe().map(|(name, _)| name)
    }

    /// Like [`Remote::transcribe_node_available`], with the longest window (seconds) that node treats as a quick job.
    /// A node that does not say gets 120.
    pub fn transcribe_node_window(&self) -> Option<(String, u32)> {
        self.best_transcribe()
    }

    fn best_transcribe(&self) -> Option<(String, u32)> {
        let g = self.inner.lock();
        if !g.enabled {
            return None;
        }
        g.nodes
            .iter()
            .filter(|n| n.saved.approved)
            .filter(|n| n.live.conn.as_ref().map(|c| c.close_reason().is_none()).unwrap_or(false))
            .filter_map(|n| n.live.status.as_ref().filter(|s| can_transcribe(s)).map(|s| (s.caps.transcribe_x, n.saved.name.clone(), node_window_s(s.caps.window_max_s))))
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, name, w)| (name, w))
    }

    /// Speech to text on recorded audio, on a compute node with a large speech model.
    ///
    /// `open_audio(start_ms)` gives the audio (16 kHz mono 16 bit little endian, in pieces of up to 1 MB)
    /// from `start_ms` on. Times everywhere are milliseconds from the start of the whole audio.
    ///
    /// Nodes are tried best first (fastest, then least busy). When one drops out partway, the segments it
    /// finished are kept and the next node gets only the audio after the last of them. `None` means no node
    /// qualifies and nothing was sent. `Some(Err)` means every node failed before finishing anything. Otherwise the
    /// outcome says whether the whole audio was covered (`complete`) or where it stopped, so the caller can keep
    /// the live transcript for the rest. `on_progress(done_ms, total_ms, node)` reports how far it is and
    /// `on_segment` gets each finished stretch.
    pub fn transcribe(
        self: &Arc<Self>,
        job: &TranscribeJob<'_>,
        open_audio: impl Fn(u64) -> Result<PcmFrames>,
        on_progress: impl Fn(u64, u64, &str),
        mut on_segment: impl FnMut(u64, u64, &str),
    ) -> Option<Result<TranscribeOutcome>> {
        if !self.enabled() || job.total_ms == 0 {
            return None;
        }
        let candidates = self.rt.block_on(self.pick_all(Kind::Transcribe));
        if candidates.is_empty() {
            return None;
        }
        let mut out = TranscribeOutcome { segments: Vec::new(), language: None, complete: false, node: String::new() };
        let mut last_err: Option<anyhow::Error> = None;
        for cand in candidates {
            if job.cancel.load(Ordering::Relaxed) {
                break;
            }
            let start = resume_point_ms(&out.segments);
            if !out.segments.is_empty() && tail_is_negligible(job.total_ms, start) {
                out.complete = true;
                break;
            }
            let frames = match open_audio(start) {
                Ok(f) => f,
                // The recording itself cannot be read: another node would not help.
                Err(e) => return Some(Err(e)),
            };
            let _out = Out::new(self, &cand.fp);
            let started = Instant::now();
            let before = out.segments.len();
            match self.rt.block_on(self.transcribe_on(&cand, job, start, frames, &mut out, &on_progress, &mut on_segment)) {
                Ok(Attempt::Finished) => {
                    tracing::info!("transcript from {}: {} segments in {:.1}s", cand.name, out.segments.len() - before, started.elapsed().as_secs_f32());
                    out.complete = true;
                    out.node = cand.name.clone();
                    break;
                }
                Ok(Attempt::Cancelled) => break,
                Err(e) => {
                    tracing::warn!("{} stopped the transcript ({e:#}), {} segments had arrived", cand.name, out.segments.len() - before);
                    if out.segments.len() > before {
                        out.node = cand.name.clone();
                    }
                    self.mark_failed(&cand.fp, &e);
                    last_err = Some(e);
                }
            }
        }
        if out.segments.is_empty() && !out.complete && !job.cancel.load(Ordering::Relaxed) {
            return Some(Err(last_err.unwrap_or_else(|| anyhow!("no compute node could listen to the recording"))));
        }
        Some(Ok(out))
    }

    /// One attempt on one node: the audio from `start_ms` goes up while the answers come down.
    #[allow(clippy::too_many_arguments)]
    async fn transcribe_on(
        &self,
        node: &Picked,
        job: &TranscribeJob<'_>,
        start_ms: u64,
        frames: PcmFrames,
        out: &mut TranscribeOutcome,
        on_progress: &impl Fn(u64, u64, &str),
        on_segment: &mut impl FnMut(u64, u64, &str),
    ) -> Result<Attempt> {
        let req = TranscribeRequest { language: job.language.clone(), total_ms: job.total_ms.saturating_sub(start_ms), priority: job.priority, prompt: job.prompt.clone() };
        let (mut send, recv) = client::open(&node.conn, &Request::Transcribe(req)).await?;
        // Decoding is slow work: it runs on its own thread and hands over a few pieces ahead of the upload.
        let (ftx, mut frx) = tokio::sync::mpsc::channel::<Vec<u8>>(3);
        let producer = tokio::task::spawn_blocking(move || {
            for f in frames {
                if ftx.blocking_send(f).is_err() {
                    break;
                }
            }
        });
        // The answers are read on a task of their own: reading half a frame must never be cut off by another branch.
        let (etx, mut erx) = tokio::sync::mpsc::channel::<Result<Option<TranscribeEvent>>>(64);
        let reader = tokio::spawn(async move {
            let mut recv = recv;
            loop {
                let r = wire::read_frame::<TranscribeEvent>(&mut recv).await;
                let last = !matches!(r, Ok(Some(_)));
                if etx.send(r).await.is_err() || last {
                    break;
                }
            }
        });
        let last_up = std::cell::Cell::new(Instant::now());
        let result: Result<Attempt> = {
        let upload = async {
            while let Some(f) = frx.recv().await {
                for part in f.chunks(AUDIO_FRAME_MAX) {
                    wire::write_frame(&mut send, &AudioFrame::Pcm(part.to_vec())).await?;
                    last_up.set(Instant::now());
                }
            }
            wire::write_frame(&mut send, &AudioFrame::End).await?;
            let _ = send.finish();
            last_up.set(Instant::now());
            Ok::<(), anyhow::Error>(())
        };
        tokio::pin!(upload);
        let mut uploaded = false;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut last_event = Instant::now();
        let mut last_progress = Instant::now();
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if job.cancel.load(Ordering::Relaxed) {
                        break Ok(Attempt::Cancelled);
                    }
                    if link_is_lost(last_event.elapsed(), last_up.get().elapsed()) {
                        break Err(anyhow!("the computer took too long"));
                    }
                    if last_progress.elapsed() > TRANSCRIBE_NO_PROGRESS {
                        break Err(anyhow!("the computer is not making progress"));
                    }
                }
                r = &mut upload, if !uploaded => match r {
                    Ok(()) => uploaded = true,
                    Err(e) => break Err(e),
                },
                ev = erx.recv() => {
                    let ev = match ev {
                        Some(Ok(Some(e))) => e,
                        Some(Ok(None)) | None => break Err(anyhow!("the computer stopped answering")),
                        Some(Err(e)) => break Err(e),
                    };
                    last_event = Instant::now();
                    match ev {
                        TranscribeEvent::Queued { .. } => last_progress = Instant::now(),
                        TranscribeEvent::Working => {}
                        TranscribeEvent::Progress { done_ms, .. } => {
                            last_progress = Instant::now();
                            on_progress((start_ms + done_ms).min(job.total_ms), job.total_ms, &node.name);
                        }
                        TranscribeEvent::Segment { t0_ms, t1_ms, text } => {
                            last_progress = Instant::now();
                            let (t0, t1) = shift_segment(start_ms, t0_ms, t1_ms);
                            if !text.trim().is_empty() {
                                on_segment(t0, t1, &text);
                                out.segments.push((t0, t1, text));
                            }
                        }
                        TranscribeEvent::Done { language, .. } => {
                            if !language.trim().is_empty() {
                                out.language = Some(language);
                            }
                            break Ok(Attempt::Finished);
                        }
                        TranscribeEvent::Error(e) => break Err(anyhow!(e)),
                    }
                }
            }
        }
        };
        reader.abort();
        let _ = producer; // ends by itself at its next piece
        if !matches!(result, Ok(Attempt::Finished)) {
            let _ = send.reset(0u32.into());
        }
        result
    }

    /// How many home computers could take a chat job right now, from what was last seen. The write-up
    /// uses it to decide how many parts to have written at the same time.
    pub fn chat_parallelism(&self) -> usize {
        let g = self.inner.lock();
        if !g.enabled {
            return 0;
        }
        g.nodes
            .iter()
            .filter(|n| n.saved.approved)
            .filter(|n| n.live.conn.as_ref().map(|c| c.close_reason().is_none()).unwrap_or(false))
            .filter(|n| n.live.status.as_ref().map(|s| !s.paused && s.caps.chat.is_some() && (s.caps.chat_tok_s == 0.0 || s.caps.chat_tok_s >= MIN_CHAT_TOK_S)).unwrap_or(false))
            .count()
    }

    /// The network under the phone changed (Wi-Fi dropped or came back). Every connection is dropped
    /// at once, so work in flight fails over now instead of after a timeout, and the computers are
    /// looked for again a moment after the network returns.
    pub fn network(self: &Arc<Self>, up: bool) {
        let count = {
            let mut g = self.inner.lock();
            for n in g.nodes.iter_mut() {
                if let Some(c) = n.live.conn.take() {
                    c.close(0u32.into(), b"network changed");
                }
                n.live.status = None;
                n.live.failed_at = None;
                n.live.checked_at = None;
                if !up {
                    n.live.last_error = "Not reachable on this network".into();
                }
            }
            g.nodes.len()
        };
        if up && count > 0 {
            let r = self.clone();
            self.rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                for i in 0..count {
                    r.clone().ensure_idx(i, true).await;
                }
            });
        }
    }

    /// Remembers how fast a compute node really answered, as a moving average.
    fn record_speed(&self, fp: &str, tok_s: f32) {
        let mut g = self.inner.lock();
        if let Some(n) = g.nodes.iter_mut().find(|n| n.saved.fp == fp) {
            n.live.observed_chat = if n.live.observed_chat > 0.0 { n.live.observed_chat * 0.6 + tok_s * 0.4 } else { tok_s };
            n.live.slow_until = None;
        }
    }

    /// A compute node that turned slow is not given chat jobs for a while.
    fn mark_slow(&self, fp: &str, tok_s: f32) {
        let mut g = self.inner.lock();
        if let Some(n) = g.nodes.iter_mut().find(|n| n.saved.fp == fp) {
            n.live.observed_chat = tok_s;
            n.live.slow_until = Some(Instant::now() + SLOW_PAUSE);
            tracing::warn!("{} is slow ({tok_s:.1} words a second), leaving it alone for {} s", n.saved.name, SLOW_PAUSE.as_secs());
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
    async fn pick_all(self: &Arc<Self>, kind: Kind) -> Vec<Picked> {
        let count = self.inner.lock().nodes.len();
        if count == 0 {
            return vec![];
        }
        let mut set = tokio::task::JoinSet::new();
        for i in 0..count {
            let this = self.clone();
            set.spawn(async move { this.ensure_idx(i, false).await });
        }
        while set.join_next().await.is_some() {}
        let g = self.inner.lock();
        let mut ranked: Vec<(u32, u32, Picked)> = Vec::new();
        for n in g.nodes.iter().filter(|n| n.saved.approved) {
            let (Some(conn), Some(st)) = (n.live.conn.clone(), n.live.status.as_ref()) else { continue };
            if conn.close_reason().is_some() || st.paused {
                continue;
            }
            if matches!(kind, Kind::Chat) && n.live.slow_until.map(|t| t > Instant::now()).unwrap_or(false) {
                continue;
            }
            let ok = match kind {
                Kind::Chat => st.caps.chat.is_some() && (st.caps.chat_tok_s == 0.0 || st.caps.chat_tok_s >= MIN_CHAT_TOK_S),
                Kind::Embed => st.caps.embed.is_some() && st.caps.embed_dim == 768 && (st.caps.embed_per_s == 0.0 || st.caps.embed_per_s >= MIN_EMBED_PER_S),
                Kind::Transcribe => can_transcribe(st),
            };
            if !ok {
                continue;
            }
            // Expected wait: the jobs ahead plus this one, divided by how fast the computer works, so a
            // quick computer with a short queue beats an idle slow one. The round trip breaks ties.
            let speed = match kind {
                // What the node says it does now, or what this phone really got from it, whichever is lower.
                Kind::Chat if n.live.observed_chat > 0.0 && st.caps.chat_tok_s > 0.0 => st.caps.chat_tok_s.min(n.live.observed_chat),
                Kind::Chat if n.live.observed_chat > 0.0 => n.live.observed_chat,
                Kind::Chat => st.caps.chat_tok_s,
                Kind::Embed => st.caps.embed_per_s,
                Kind::Transcribe => st.caps.transcribe_x,
            };
            let speed = if speed > 0.0 { speed } else if matches!(kind, Kind::Transcribe) { UNMEASURED_TRANSCRIBE_X } else { 10.0 };
            let ours = self.inflight.lock().get(&n.saved.fp).copied().unwrap_or(0);
            let ahead = (st.running + st.waiting).max(ours);
            let wait = if matches!(kind, Kind::Transcribe) { transcribe_wait(st.caps.transcribe_x, ahead) } else { ((ahead + 1) as f32 / speed * 1000.0) as u32 };
            ranked.push((wait, n.live.rtt_ms, Picked { conn, fp: n.saved.fp.clone(), name: n.saved.name.clone() }));
        }
        ranked.sort_by_key(|r| (r.0, r.1));
        ranked.into_iter().map(|r| r.2).collect()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Chat,
    Embed,
    Transcribe,
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
        "This compute node has blocked this phone".into()
    } else if s.contains("not the paired compute node") {
        "A different device answered at that address".into()
    } else if s.contains("not paired") || s.contains("closed") {
        "This phone is no longer paired with it".into()
    } else if s.contains("no answer") || s.contains("timed out") {
        "Not reachable on this network".into()
    } else {
        s
    }
}

/// Keeps an eye on the paired compute nodes while the app is running: quickly while one is waiting to be
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use marginalia_proto::AudioFrame;

    fn run(shown: &str, pieces: &[&str]) -> String {
        let mut f = SeamFilter::new(shown);
        let mut out = String::new();
        for p in pieces {
            f.push(p, &mut |t| out.push_str(t));
        }
        f.finish(&mut |t| out.push_str(t));
        out
    }

    #[test]
    fn a_repeated_sentence_is_dropped() {
        let shown = "Notes are shared. Membership within the cluster is handled via gossip";
        let out = run(shown, &["Membership within the cluster ", "is handled via gossip, with last ", "writer wins."]);
        assert_eq!(out, ", with last writer wins.");
    }

    #[test]
    fn new_text_that_does_not_repeat_is_kept_with_a_space() {
        let out = run("Everything is shared via gossip", &["Revoking a phone writes a record."]);
        assert_eq!(out, " Revoking a phone writes a record.");
    }

    #[test]
    fn a_word_cut_in_half_is_finished_without_a_space() {
        let out = run("The notes are handl", &["ed by the node."]);
        assert_eq!(out, "ed by the node.");
    }

    #[test]
    fn short_answers_are_flushed_at_the_end() {
        assert_eq!(run("one two three four", &["five"]), "five");
    }

    // ------------------------------------------------------------------ listening to recorded audio

    fn seg(t0: u64, t1: u64) -> (u64, u64, String) {
        (t0, t1, "x".into())
    }

    #[test]
    fn the_next_node_picks_up_after_the_last_finished_segment() {
        assert_eq!(resume_point_ms(&[]), 0);
        assert_eq!(resume_point_ms(&[seg(0, 4000), seg(4500, 9000), seg(9000, 8000)]), 9000);
        assert_eq!(shift_segment(9000, 0, 2500), (9000, 11500));
        assert_eq!(shift_segment(9000, 300, 100), (9300, 9300), "an end before the start is not allowed");
        assert!(tail_is_negligible(100_000, 97_000));
        assert!(!tail_is_negligible(100_000, 90_000));
    }

    #[test]
    fn silence_only_counts_when_nothing_goes_up_either() {
        let s = |n| Duration::from_secs(n);
        assert!(!link_is_lost(s(2), s(2)));
        assert!(!link_is_lost(s(10), s(1)), "audio is still going up");
        assert!(!link_is_lost(s(1), s(10)), "the node is still answering");
        assert!(link_is_lost(s(7), s(7)));
    }

    #[test]
    fn faster_and_less_busy_nodes_rank_first() {
        assert!(transcribe_wait(8.0, 0) < transcribe_wait(2.0, 0));
        assert!(transcribe_wait(4.0, 0) < transcribe_wait(4.0, 2));
        assert!(transcribe_wait(8.0, 3) > transcribe_wait(8.0, 1));
        assert_eq!(transcribe_wait(0.0, 0), transcribe_wait(UNMEASURED_TRANSCRIBE_X, 0));
    }

    pub(crate) fn status(transcribe: Option<&str>, x: f32, paused: bool) -> StatusReply {
        let mut st = StatusReply { proto: marginalia_proto::PROTO_VERSION, paused, ..Default::default() };
        st.caps.transcribe = transcribe.map(String::from);
        st.caps.transcribe_x = x;
        st
    }

    #[test]
    fn only_a_node_that_listens_quickly_and_is_not_paused_qualifies() {
        assert!(can_transcribe(&status(Some("large"), 6.0, false)));
        assert!(can_transcribe(&status(Some("large"), 0.0, false)), "not measured yet is let through");
        assert!(!can_transcribe(&status(Some("large"), 0.4, false)), "slower than real time");
        assert!(!can_transcribe(&status(None, 6.0, false)));
        assert!(!can_transcribe(&status(Some("large"), 6.0, true)));
    }

    // A small node that speaks the protocol, to try the phone side against.

    #[derive(Clone, Copy)]
    pub(crate) enum Mode {
        /// Listens to everything and answers.
        Good,
        /// Answers this many segments, then the connection dies.
        DropAfter(usize),
        /// Says it failed.
        Broken,
    }

    pub(crate) struct Fake {
        pub(crate) fp: String,
        pub(crate) addr: SocketAddr,
        /// For every listening job: (total_ms announced, bytes of audio received).
        pub(crate) jobs: Arc<std::sync::Mutex<Vec<(u64, usize)>>>,
        /// For every listening job: its priority and the prompt it came with.
        pub(crate) metas: Arc<std::sync::Mutex<Vec<(Priority, Option<String>)>>>,
    }

    pub(crate) fn start_fake(rt: &tokio::runtime::Runtime, x: f32, mode: Mode) -> Fake {
        use marginalia_proto::tls;
        let id = tls::Identity::generate().unwrap();
        let _g = rt.enter();
        let ep = quinn::Endpoint::server(tls::server_config(&id).unwrap(), (std::net::Ipv4Addr::LOCALHOST, 0).into()).unwrap();
        let addr = ep.local_addr().unwrap();
        let jobs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let jobs2 = jobs.clone();
        let metas = Arc::new(std::sync::Mutex::new(Vec::new()));
        let metas2 = metas.clone();
        rt.spawn(async move {
            while let Some(incoming) = ep.accept().await {
                let jobs = jobs2.clone();
                let metas = metas2.clone();
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                        let (jobs, conn, metas) = (jobs.clone(), conn.clone(), metas.clone());
                        tokio::spawn(async move {
                            let Ok(Some(req)) = wire::read_frame::<Request>(&mut recv).await else { return };
                            match req {
                                Request::Status => {
                                    let mut st = status(Some("large-test"), x, false);
                                    st.caps.window_max_s = 90;
                                    st.node_name = "fake".into();
                                    let _ = wire::write_frame(&mut send, &st).await;
                                    let _ = send.finish();
                                    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
                                }
                                Request::Transcribe(t) => {
                                    let mut bytes = 0usize;
                                    loop {
                                        match wire::read_frame::<AudioFrame>(&mut recv).await {
                                            Ok(Some(AudioFrame::Pcm(p))) => bytes += p.len(),
                                            _ => break,
                                        }
                                    }
                                    jobs.lock().unwrap().push((t.total_ms, bytes));
                                    metas.lock().unwrap().push((t.priority, t.prompt.clone()));
                                    if matches!(mode, Mode::Broken) {
                                        let _ = wire::write_frame(&mut send, &TranscribeEvent::Error("no model".into())).await;
                                        let _ = send.finish();
                                        let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
                                        return;
                                    }
                                    let _ = wire::write_frame(&mut send, &TranscribeEvent::Queued { ahead: 0 }).await;
                                    let secs = bytes / 32_000;
                                    for i in 0..secs {
                                        if let Mode::DropAfter(n) = mode {
                                            if i == n {
                                                tokio::time::sleep(Duration::from_millis(300)).await; // let what was sent arrive
                                                conn.close(9u32.into(), b"boom");
                                                return;
                                            }
                                        }
                                        let (a, b) = (i as u64 * 1000, (i as u64 + 1) * 1000);
                                        let _ = wire::write_frame(&mut send, &TranscribeEvent::Segment { t0_ms: a, t1_ms: b, text: format!("w{i}") }).await;
                                        let _ = wire::write_frame(&mut send, &TranscribeEvent::Progress { done_ms: b, total_ms: t.total_ms }).await;
                                    }
                                    let _ = wire::write_frame(&mut send, &TranscribeEvent::Done { language: "en".into(), segments: secs as u32, elapsed_ms: 5 }).await;
                                    let _ = send.finish();
                                    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
                                }
                                _ => {}
                            }
                        });
                    }
                });
            }
        });
        Fake { fp: id.fingerprint(), addr, jobs, metas }
    }

    pub(crate) fn remote_with(fakes: &[(&str, &Fake)], tag: &str) -> Arc<Remote> {
        let dir = std::env::temp_dir().join(format!("marginalia-remote-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Remote::open(dir.join("remote.json")).unwrap();
        {
            let mut g = r.inner.lock();
            for (name, f) in fakes {
                g.nodes.push(Node {
                    saved: SavedNode { fp: f.fp.clone(), name: name.to_string(), addrs: vec![f.addr.to_string()], approved: true },
                    live: Live { conn: None, status: None, rtt_ms: 0, failed_at: None, checked_at: None, last_error: String::new(), refused: false, observed_chat: 0.0, slow_until: None },
                });
            }
        }
        r
    }

    /// `total_secs` seconds of silence, one second per piece, starting at `start_ms`.
    pub(crate) fn silence(total_secs: u64) -> impl Fn(u64) -> Result<PcmFrames> {
        move |start_ms| {
            let left = total_secs.saturating_sub(start_ms / 1000);
            Ok(Box::new((0..left).map(|_| vec![0u8; 32_000])) as PcmFrames)
        }
    }

    fn run_job(r: &Arc<Remote>, secs: u64) -> (Option<Result<TranscribeOutcome>>, Vec<(u64, u64)>) {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let job = TranscribeJob { total_ms: secs * 1000, language: Some("en".into()), cancel: &cancel, priority: Priority::Background, prompt: None };
        let progress = std::sync::Mutex::new(Vec::new());
        let out = r.transcribe(&job, silence(secs), |d, t, _| progress.lock().unwrap().push((d, t)), |_, _, _| {});
        (out, progress.into_inner().unwrap())
    }

    #[test]
    fn a_node_that_listens_returns_the_whole_transcript() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = start_fake(&rt, 8.0, Mode::Good);
        let r = remote_with(&[("a", &a)], "good");
        assert_eq!(r.transcribe_node_available(), None, "nothing is known before the node has been looked at");
        r.refresh();
        assert_eq!(r.transcribe_node_available().as_deref(), Some("a"));
        let (out, progress) = run_job(&r, 6);
        let out = out.expect("a node qualifies").unwrap();
        assert!(out.complete);
        assert_eq!(out.language.as_deref(), Some("en"));
        assert_eq!(out.segments.len(), 6);
        assert_eq!(out.segments[5], (5000, 6000, "w5".to_string()));
        assert_eq!(progress.last(), Some(&(6000, 6000)));
        assert_eq!(a.jobs.lock().unwrap().as_slice(), &[(6000, 6 * 32_000)]);
        assert_eq!(r.inflight.lock().values().sum::<u32>(), 0, "the job is no longer counted as out");
    }

    #[test]
    fn nothing_is_sent_when_no_node_can_listen() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let r = remote_with(&[], "none");
        assert!(run_job(&r, 5).0.is_none());
        r.set_enabled(false);
        let a = start_fake(&rt, 8.0, Mode::Good);
        let r = remote_with(&[("a", &a)], "off");
        r.set_enabled(false);
        assert!(run_job(&r, 5).0.is_none());
        assert!(a.jobs.lock().unwrap().is_empty());
    }

    #[test]
    fn when_a_node_drops_out_the_next_one_gets_only_the_audio_after_it() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        // The first is faster, so it is asked first, and dies after three segments.
        let a = start_fake(&rt, 9.0, Mode::DropAfter(3));
        let b = start_fake(&rt, 4.0, Mode::Good);
        let r = remote_with(&[("a", &a), ("b", &b)], "failover");
        let (out, progress) = run_job(&r, 10);
        let out = out.unwrap().unwrap();
        assert!(out.complete);
        assert_eq!(out.node, "b");
        let times: Vec<(u64, u64)> = out.segments.iter().map(|s| (s.0, s.1)).collect();
        assert_eq!(times, (0..10).map(|i| (i * 1000, (i + 1) * 1000)).collect::<Vec<_>>(), "no gap and no repeat");
        assert_eq!(a.jobs.lock().unwrap().as_slice(), &[(10_000, 10 * 32_000)]);
        assert_eq!(b.jobs.lock().unwrap().as_slice(), &[(7_000, 7 * 32_000)], "only the audio after the last finished segment was sent");
        assert_eq!(out.segments[3].2, "w0", "the second node's text starts where the first stopped");
        assert!(progress.windows(2).all(|w| w[0].0 <= w[1].0 || w[1].0 <= 3000), "progress never runs backwards past what was kept");
    }

    #[test]
    fn when_every_node_drops_out_the_partial_transcript_comes_back() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = start_fake(&rt, 9.0, Mode::DropAfter(4));
        let b = start_fake(&rt, 4.0, Mode::DropAfter(2));
        let r = remote_with(&[("a", &a), ("b", &b)], "partial");
        let out = run_job(&r, 20).0.unwrap().unwrap();
        assert!(!out.complete);
        assert_eq!(out.segments.len(), 6);
        assert_eq!(out.covered_ms(), 6000);
    }

    #[test]
    fn when_every_node_fails_at_once_it_is_an_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = start_fake(&rt, 9.0, Mode::Broken);
        let r = remote_with(&[("a", &a)], "broken");
        let out = run_job(&r, 5).0.unwrap();
        assert!(out.is_err());
    }

    #[test]
    fn a_cancelled_job_stops_without_an_error() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let a = start_fake(&rt, 9.0, Mode::Good);
        let r = remote_with(&[("a", &a)], "cancel");
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let job = TranscribeJob { total_ms: 5000, language: None, cancel: &cancel, priority: Priority::Background, prompt: None };
        let out = r.transcribe(&job, silence(5), |_, _, _| {}, |_, _, _| {}).unwrap().unwrap();
        assert!(!out.complete && out.segments.is_empty());
    }
}
