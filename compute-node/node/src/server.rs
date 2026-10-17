//! Answers phones. One task per connection, one per request. A job's text and answer exist only in
//! this task's memory and go away when it ends.

use crate::engine::{self, GenEvent, GenParams, GenRequest};
use crate::cluster::Cluster;
use crate::host::Host;
use crate::metrics::Metrics;
use crate::place::{Job, Node};
use crate::state::Store;
use anyhow::Result;
use marginalia_proto::tls::{self, Identity};
use marginalia_proto::{pair, wire, AudioFrame, ChatEvent, DecideReply, EmbedReply, KnockReply, PairReply, PeerReply, PendingPhone, Priority, RecordsReply, Request, StatusReply, TranscribeEvent};
use std::collections::HashMap;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Pairing {
    pub code: String,
    pub until: Instant,
    pub tries: u8,
}

pub struct Waiting {
    pub name: String,
    pub since: Instant,
}

pub struct Shared {
    pub store: Arc<Store>,
    /// The other compute nodes this one has joined.
    pub cluster: Arc<Cluster>,
    pub host: Arc<Host>,
    pub identity: Identity,
    pub name: String,
    pub pairing: Mutex<Option<Pairing>>,
    pub metrics: Metrics,
    /// Phones that knocked and have not been answered, by certificate fingerprint.
    pub waiting: Mutex<HashMap<String, Waiting>>,
    /// Phones that were turned down, for a while.
    pub denied: Mutex<HashMap<String, Instant>>,
    /// Locked: a phone that asks has to be allowed (from a paired phone or `marginalia-node approve`)
    /// instead of being let in. By default any phone on the network that its owner trusts this
    /// compute node from may use it. The node keeps nothing, so a phone that gets in can only borrow its speed.
    pub locked: std::sync::atomic::AtomicBool,
    /// Paused: models stay loaded, new chat and search jobs are turned away, running jobs finish.
    /// Kept in memory only, so a restart is never paused.
    pub paused: AtomicBool,
    /// Woken by the `shutdown` command to make the node stop.
    pub stop: tokio::sync::Notify,
}

impl Shared {
    /// Opens the pairing window and returns the code to type on the phone.
    pub fn open_pairing(&self, minutes: u64) -> String {
        let code = pair::new_code();
        *self.pairing.lock() = Some(Pairing { code: code.clone(), until: Instant::now() + Duration::from_secs(minutes * 60), tries: 5 });
        code
    }

    pub fn pending_list(&self) -> Vec<PendingPhone> {
        let mut w = self.waiting.lock();
        w.retain(|_, p| p.since.elapsed() < Duration::from_secs(15 * 60));
        w.iter().map(|(fp, p)| PendingPhone { id: fp[..16].to_string(), name: p.name.clone(), waited_s: p.since.elapsed().as_secs() }).collect()
    }

    /// Lets a waiting phone in (or turns it down). `who` starts with its id.
    pub fn decide(&self, who: &str, allow: bool) -> Result<String, String> {
        if who.len() < 4 {
            return Err("Give at least 4 characters of the phone's id.".into());
        }
        let mut w = self.waiting.lock();
        let hits: Vec<String> = w.keys().filter(|fp| fp.starts_with(who)).cloned().collect();
        let fp = match hits.as_slice() {
            [one] => one.clone(),
            [] => return Err("No phone is waiting with that id.".into()),
            _ => return Err("More than one phone matches, give more of the id.".into()),
        };
        let entry = w.remove(&fp).expect("present");
        drop(w);
        if allow {
            self.store.add(fp, entry.name.clone()).map_err(|e| e.to_string())?;
            tracing::info!("{} was allowed", entry.name);
        } else {
            self.denied.lock().insert(fp, Instant::now());
            tracing::info!("{} was turned down", entry.name);
        }
        Ok(entry.name)
    }

    fn knock(&self, fp: &str, name: String) -> KnockReply {
        let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = if name.trim().is_empty() { "A phone".to_string() } else { name };
        if self.denied.lock().get(fp).map(|t| t.elapsed() < Duration::from_secs(10 * 60)).unwrap_or(false) {
            return KnockReply::Refused("This compute node did not allow that phone.".into());
        }
        if !self.locked.load(Ordering::Relaxed) {
            if self.store.add(fp.to_string(), name.clone()).is_ok() {
                tracing::info!("{name} connected");
                return KnockReply::Accepted { node_name: self.name.clone() };
            }
        }
        let mut w = self.waiting.lock();
        w.retain(|_, p| p.since.elapsed() < Duration::from_secs(15 * 60));
        if !w.contains_key(fp) && w.len() >= 8 {
            return KnockReply::Refused("Too many phones are waiting. Try again later.".into());
        }
        w.entry(fp.to_string()).or_insert_with(|| {
            tracing::info!("{name} is asking to use this compute node. Allow it from a phone that is already paired, or run `marginalia-node approve`.");
            Waiting { name, since: Instant::now() }
        });
        KnockReply::Pending { node_name: self.name.clone() }
    }

    /// Jobs accepted and not finished yet (the one being worked on plus those waiting).
    pub fn in_flight(&self) -> u32 {
        self.host.chat_jobs.load(Ordering::Relaxed) + self.host.embed_jobs.load(Ordering::Relaxed) + self.host.speech_jobs.load(Ordering::Relaxed) + self.host.window_jobs.load(Ordering::Relaxed)
    }

    /// Chat, search and transcription jobs finished since the node started.
    pub fn served(&self) -> u64 {
        self.metrics.chat_jobs.load(Ordering::Relaxed) + self.metrics.embed_jobs.load(Ordering::Relaxed) + self.metrics.speech_jobs.load(Ordering::Relaxed) + self.metrics.window_jobs.load(Ordering::Relaxed)
    }

    fn status(&self) -> StatusReply {
        let running = self.in_flight();
        StatusReply {
            proto: marginalia_proto::PROTO_VERSION,
            node_name: self.name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            caps: self.host.caps.lock().clone(),
            running: running.min(1),
            waiting: running.saturating_sub(1),
            clients: self.store.list().len() as u32,
            served: self.served(),
            uptime_s: self.metrics.uptime_s(),
            pending: self.pending_list(),
            paused: self.paused.load(Ordering::Relaxed),
        }
    }
}

pub fn endpoint(shared: &Shared, port: u16) -> Result<quinn::Endpoint> {
    let cfg = tls::server_config(&shared.identity)?;
    Ok(quinn::Endpoint::server(cfg, (std::net::Ipv4Addr::UNSPECIFIED, port).into())?)
}

pub async fn serve(shared: Arc<Shared>, ep: quinn::Endpoint) {
    while let Some(incoming) = ep.accept().await {
        let sh = shared.clone();
        tokio::spawn(async move {
            if let Ok(conn) = incoming.await {
                connection(sh, conn).await;
            }
        });
    }
}

async fn connection(sh: Arc<Shared>, conn: quinn::Connection) {
    let Some(fp) = tls::peer_fingerprint(&conn) else {
        conn.close(1u32.into(), b"no certificate");
        return;
    };
    if sh.store.is_blocked(&fp) {
        sh.metrics.rejected.fetch_add(1, Ordering::Relaxed);
        conn.close(3u32.into(), b"blocked");
        return;
    }
    loop {
        let Ok((send, recv)) = conn.accept_bi().await else { break };
        let (sh, conn, fp) = (sh.clone(), conn.clone(), fp.clone());
        tokio::spawn(async move {
            if let Err(e) = request(sh, conn, fp, send, recv).await {
                tracing::debug!("request ended: {e:#}");
            }
        });
    }
}

async fn request(sh: Arc<Shared>, conn: quinn::Connection, fp: String, mut send: quinn::SendStream, mut recv: quinn::RecvStream) -> Result<()> {
    // A block applies to connections that are already open as well.
    if sh.store.is_blocked(&fp) {
        sh.metrics.rejected.fetch_add(1, Ordering::Relaxed);
        conn.close(3u32.into(), b"blocked");
        return Ok(());
    }
    // Someone who is not paired may only ask to pair, so what they send is kept small: nothing is
    // allocated for them before they have shown they know the code.
    // A peer is known too, with the rights of a phone and a few more. What a phone sends may be forwarded to a peer,
    // what a peer sends never is.
    let peer = sh.cluster.peers.find(&fp);
    let known = sh.store.find(&fp).or_else(|| peer.as_ref().map(|p| crate::state::Client { fp: fp.clone(), name: p.name.clone(), paired_at: p.added_at }));
    let limit = if known.is_some() { 64 * 1024 * 1024 } else { 4 * 1024 };
    let Some(req) = wire::read_frame_max::<Request>(&mut recv, limit).await? else { return Ok(()) };
    match (req, known) {
        (Request::Pair { name, proof }, None) => {
            let reply = try_pair(&sh, &conn, &fp, name, proof);
            wire::write_frame(&mut send, &reply).await?;
        }
        (Request::Pair { .. }, Some(_)) => {
            let why = if peer.is_some() { "That compute node is already a peer." } else { "This phone is already paired." };
            wire::write_frame(&mut send, &PairReply::Refused(why.into())).await?;
        }
        (Request::Join { name, port, proof }, Some(_)) => {
            let reply = sh.cluster.accept_join(&conn, &fp, name, port, proof);
            wire::write_frame(&mut send, &reply).await?;
        }
        (Request::Records { port, records }, Some(_)) if peer.is_some() => {
            sh.cluster.peer_seen(&fp, conn.remote_address().ip(), port);
            if let Err(e) = sh.store.merge(&records) {
                tracing::warn!("could not save phone records from a peer: {e:#}");
            }
            wire::write_frame(&mut send, &RecordsReply { records: sh.store.records() }).await?;
        }
        (Request::Leave, Some(_)) if peer.is_some() => {
            wire::write_frame(&mut send, &PeerReply::Ok).await?;
            sh.cluster.peer_left(&fp);
        }
        (Request::Records { .. } | Request::Leave, Some(_)) => {
            sh.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            conn.close(2u32.into(), b"not a peer");
            return Ok(());
        }
        (Request::Knock { name }, None) => wire::write_frame(&mut send, &sh.knock(&fp, name)).await?,
        (Request::Knock { .. }, Some(_)) => wire::write_frame(&mut send, &KnockReply::Accepted { node_name: sh.name.clone() }).await?,
        (Request::KnockStatus, None) => {
            let reply = if sh.waiting.lock().contains_key(&fp) {
                KnockReply::Pending { node_name: sh.name.clone() }
            } else if sh.denied.lock().contains_key(&fp) {
                KnockReply::Refused("This compute node did not allow that phone.".into())
            } else {
                KnockReply::Refused("This compute node does not know that phone. Ask to connect again.".into())
            };
            wire::write_frame(&mut send, &reply).await?;
        }
        (Request::KnockStatus, Some(_)) => wire::write_frame(&mut send, &KnockReply::Accepted { node_name: sh.name.clone() }).await?,
        (Request::Decide { who, allow }, Some(_)) => {
            let reply = match sh.decide(&who, allow) {
                Ok(_) => DecideReply::Ok,
                Err(e) => DecideReply::Refused(e),
            };
            wire::write_frame(&mut send, &reply).await?;
        }
        (_, None) => {
            sh.metrics.rejected.fetch_add(1, Ordering::Relaxed);
            conn.close(2u32.into(), b"not paired");
            return Ok(());
        }
        (Request::Status, Some(_)) => wire::write_frame(&mut send, &sh.status()).await?,
        (Request::Chat(r), Some(c)) => chat(&sh, r, &mut send, c, peer.is_some()).await?,
        (Request::Embed(r), Some(c)) => embed(&sh, r, &mut send, c, peer.is_some()).await?,
        (Request::Transcribe(r), Some(c)) => transcribe(&sh, &conn, r, &mut send, recv, c).await?,
    }
    let _ = send.finish();
    // Let the other side read everything before the stream is dropped.
    let _ = tokio::time::timeout(Duration::from_secs(5), send.stopped()).await;
    Ok(())
}

fn try_pair(sh: &Shared, conn: &quinn::Connection, fp: &str, name: String, proof: [u8; 32]) -> PairReply {
    let mut guard = sh.pairing.lock();
    let Some(p) = guard.as_mut() else {
        return PairReply::Refused("Pairing is not open on this compute node. Run `marginalia-node pair` first.".into());
    };
    if Instant::now() > p.until {
        *guard = None;
        return PairReply::Refused("The code has expired. Run `marginalia-node pair` again.".into());
    }
    let (Some(exp), node_fp) = (pair::exporter(conn), sh.identity.fingerprint()) else {
        return PairReply::Refused("No session secret.".into());
    };
    let want = pair::proof(&p.code, pair::Side::Phone, &exp, fp, &node_fp);
    if !pair::proofs_equal(&want, &proof) {
        p.tries = p.tries.saturating_sub(1);
        if p.tries == 0 {
            *guard = None;
            return PairReply::Refused("Too many wrong codes. Run `marginalia-node pair` again.".into());
        }
        return PairReply::Refused("That code is not right.".into());
    }
    let reply = PairReply::Ok { node_name: sh.name.clone(), proof: pair::proof(&p.code, pair::Side::Node, &exp, fp, &node_fp) };
    // A compute node joining follows the pairing with `Join`, which needs the code once more.
    sh.cluster.note_pair(fp, &p.code);
    *guard = None;
    if let Err(e) = sh.store.add(fp.to_string(), name.clone()) {
        return PairReply::Refused(format!("Could not save the pairing: {e}"));
    }
    tracing::info!("paired with {name}");
    sh.metrics.pairings.fetch_add(1, Ordering::Relaxed);
    reply
}

async fn chat(sh: &Arc<Shared>, r: marginalia_proto::ChatRequest, send: &mut quinn::SendStream, client: crate::state::Client, from_peer: bool) -> Result<()> {
    // A job from a phone goes to a peer when this node is clearly the worse choice. A job from a peer stays here,
    // so a job is never passed on twice.
    if !from_peer {
        let local = Node { load: sh.in_flight(), paused: sh.paused.load(Ordering::Relaxed), speed: sh.host.caps.lock().chat_tok_s, serves: sh.host.llm.is_some() };
        let order = sh.cluster.plan(Job::Chat { max_tokens: r.max_tokens }, &local);
        if !order.is_empty() && sh.cluster.forward_chat(order, &r, send).await? {
            return Ok(());
        }
    }
    if sh.paused.load(Ordering::Relaxed) {
        wire::write_frame(send, &ChatEvent::Error("paused".into())).await?;
        return Ok(());
    }
    let Some(llm) = sh.host.llm.clone() else {
        wire::write_frame(send, &ChatEvent::Error("This compute node does not serve chat.".into())).await?;
        return Ok(());
    };
    let jobs = sh.host.chat_jobs.clone();
    let ahead = jobs.fetch_add(1, Ordering::Relaxed);
    if ahead > 0 {
        wire::write_frame(send, &ChatEvent::Queued { ahead }).await?;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ChatEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let messages = r
        .messages
        .into_iter()
        .map(|m| engine::ChatMessage {
            role: match m.role {
                marginalia_proto::Role::System => engine::Role::System,
                marginalia_proto::Role::User => engine::Role::User,
                marginalia_proto::Role::Assistant => engine::Role::Assistant,
            },
            content: m.content,
        })
        .collect();
    let done_jobs = jobs.clone();
    let (sh2, cl) = (sh.clone(), client);
    llm.generate(GenRequest {
        messages,
        images: vec![],
        params: GenParams { max_tokens: r.max_tokens as usize, temperature: r.temperature, top_p: r.top_p, top_k: r.top_k, repeat_penalty: 1.05 },
        priority: if r.priority == Priority::Interactive { engine::Priority::High } else { engine::Priority::Low },
        cancel: cancel.clone(),
        on_event: Box::new(move |ev| {
            let out = match ev {
                GenEvent::Token(t) => ChatEvent::Token(t),
                GenEvent::Done(s) => {
                    done_jobs.fetch_sub(1, Ordering::Relaxed);
                    sh2.metrics.chat_done(&cl.fp, &cl.name, s.prompt_tokens as u64, s.generated_tokens as u64, s.elapsed_ms as u64);
                    if s.generated_tokens >= 30 && s.elapsed_ms > 0 {
                        sh2.host.note_chat_speed(s.generated_tokens as f32 * 1000.0 / s.elapsed_ms as f32);
                    }
                    ChatEvent::Done { prompt_tokens: s.prompt_tokens as u32, generated_tokens: s.generated_tokens as u32, elapsed_ms: s.elapsed_ms.min(u32::MAX as u128) as u32 }
                }
                GenEvent::Error(e) => {
                    done_jobs.fetch_sub(1, Ordering::Relaxed);
                    sh2.metrics.chat_failed(&cl.fp, &cl.name);
                    ChatEvent::Error(e)
                }
            };
            let _ = tx.send(out);
        }),
    });
    let mut beat = tokio::time::interval(Duration::from_secs(2));
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => match ev { Some(e) => e, None => break },
            _ = beat.tick() => ChatEvent::Working,
        };
        let last = matches!(ev, ChatEvent::Done { .. } | ChatEvent::Error(_));
        if wire::write_frame(send, &ev).await.is_err() {
            // The phone went away or gave up: stop generating for nobody.
            cancel.store(true, Ordering::Relaxed);
            break;
        }
        if last {
            break;
        }
    }
    Ok(())
}

async fn embed(sh: &Arc<Shared>, r: marginalia_proto::EmbedRequest, send: &mut quinn::SendStream, client: crate::state::Client, from_peer: bool) -> Result<()> {
    if !from_peer {
        let local = Node { load: sh.in_flight(), paused: sh.paused.load(Ordering::Relaxed), speed: sh.host.caps.lock().embed_per_s, serves: sh.host.embed.is_some() };
        let order = sh.cluster.plan(Job::Embed { texts: r.texts.len() as u32 }, &local);
        if !order.is_empty() && sh.cluster.forward_embed(order, &r, send).await? {
            return Ok(());
        }
    }
    if sh.paused.load(Ordering::Relaxed) {
        wire::write_frame(send, &EmbedReply::Error("paused".into())).await?;
        return Ok(());
    }
    let Some(handle) = sh.host.embed.clone() else {
        wire::write_frame(send, &EmbedReply::Error("This compute node does not serve search.".into())).await?;
        return Ok(());
    };
    let jobs = sh.host.embed_jobs.clone();
    let ahead = jobs.fetch_add(1, Ordering::Relaxed);
    if ahead > 0 {
        wire::write_frame(send, &EmbedReply::Queued { ahead }).await?;
    }
    let texts = r.texts;
    let (count, t0) = (texts.len() as u64, Instant::now());
    let mut work = tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(32) {
            all.extend(handle.embed(chunk.to_vec(), engine::EmbedKind::Raw)?);
        }
        Ok(all)
    });
    let mut beat = tokio::time::interval(Duration::from_secs(2));
    let out = loop {
        tokio::select! {
            r = &mut work => break r,
            // Tells the phone this node is still alive while a large batch runs.
            _ = beat.tick() => {
                if wire::write_frame(send, &EmbedReply::Queued { ahead: 0 }).await.is_err() {
                    // The phone is gone. The batch finishes in the background and its result is dropped.
                    jobs.fetch_sub(1, Ordering::Relaxed);
                    return Ok(());
                }
            }
        }
    };
    jobs.fetch_sub(1, Ordering::Relaxed);
    let took = t0.elapsed();
    let ok = matches!(out, Ok(Ok(_)));
    sh.metrics.embed_done(&client.fp, &client.name, count, took.as_millis() as u64, ok);
    if ok && count >= 8 && !took.is_zero() {
        sh.host.note_embed_speed(count as f32 / took.as_secs_f32());
    }
    let reply = match out {
        Ok(Ok(v)) => EmbedReply::Vectors(v),
        Ok(Err(e)) => EmbedReply::Error(format!("{e:#}")),
        Err(e) => EmbedReply::Error(format!("{e}")),
    };
    wire::write_frame(send, &reply).await?;
    Ok(())
}

/// The longest recording a node takes: 3 hours of 16 kHz audio is about 350 MB in memory.
const MAX_AUDIO_MS: u64 = 3 * 60 * 60 * 1000;
/// Recordings waiting behind the one being transcribed. More than this are turned away.
const MAX_WAITING: u32 = 3;
/// A little more audio than announced is let through on the window lane (a frame boundary, rounding), not hours.
const WINDOW_SLACK_MS: u64 = 5_000;
/// Windows waiting behind the one being transcribed. More than this are turned away.
const MAX_WINDOWS_WAITING: u32 = 4;
/// Windows one phone may have in flight. It should only ever send one at a time.
const MAX_WINDOWS_PER_PHONE: u32 = 2;
/// A window that has waited this long for its turn is turned away: the words it was for have gone stale.
const WINDOW_MAX_WAIT: Duration = Duration::from_secs(20);

/// Counts a transcription job while it exists, so `status` and a clean stop see it until the work has really ended.
struct SpeechJob {
    jobs: Arc<std::sync::atomic::AtomicU32>,
    /// For a window: the host and the phone it is counted against.
    window_of: Option<(Arc<Shared>, String)>,
}

impl Drop for SpeechJob {
    fn drop(&mut self) {
        self.jobs.fetch_sub(1, Ordering::Relaxed);
        if let Some((sh, fp)) = self.window_of.take() {
            let mut m = sh.host.window_by_phone.lock();
            if let Some(n) = m.get_mut(&fp) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    m.remove(&fp);
                }
            }
        }
    }
}

/// Reads the audio frames of a transcription request: raw 16 bit mono PCM until `AudioFrame::End`.
async fn read_audio(mut recv: quinn::RecvStream, expected_ms: u64, max_ms: u64) -> Result<Vec<i16>> {
    let max_samples = (max_ms * 16) as usize;
    let mut out: Vec<i16> = Vec::with_capacity((expected_ms * 16).min(max_ms * 16) as usize);
    let mut carry: Option<u8> = None;
    loop {
        match wire::read_frame_max::<AudioFrame>(&mut recv, 8 * 1024 * 1024).await? {
            Some(AudioFrame::Pcm(bytes)) => {
                let mut it = carry.take().into_iter().chain(bytes.iter().copied());
                while let Some(lo) = it.next() {
                    match it.next() {
                        Some(hi) => out.push(i16::from_le_bytes([lo, hi])),
                        None => carry = Some(lo),
                    }
                }
                if out.len() > max_samples {
                    anyhow::bail!(if max_ms == MAX_AUDIO_MS { "The recording is longer than 3 hours, which is the most a compute node takes." } else { "More audio came than was announced." });
                }
            }
            Some(AudioFrame::End) => return Ok(out),
            None => anyhow::bail!("The audio stopped before the end marker."),
        }
    }
}

/// A quick job for the short lane: someone is waiting on it and the audio is no longer than the node's window length.
/// Anything longer goes the long way, even when it is interactive. Shorter windows are fine.
fn is_window(priority: Priority, total_ms: u64, window_max_s: u32) -> bool {
    priority == Priority::Interactive && window_max_s > 0 && total_ms <= window_max_s as u64 * 1000
}

async fn transcribe(sh: &Arc<Shared>, conn: &quinn::Connection, r: marginalia_proto::TranscribeRequest, send: &mut quinn::SendStream, recv: quinn::RecvStream, client: crate::state::Client) -> Result<()> {
    if sh.paused.load(Ordering::Relaxed) {
        wire::write_frame(send, &TranscribeEvent::Error("paused".into())).await?;
        return Ok(());
    }
    let Some(whisper) = sh.host.whisper.clone() else {
        wire::write_frame(send, &TranscribeEvent::Error("This compute node does not transcribe recordings.".into())).await?;
        return Ok(());
    };
    if r.total_ms > MAX_AUDIO_MS {
        wire::write_frame(send, &TranscribeEvent::Error("The recording is longer than 3 hours, which is the most a compute node takes.".into())).await?;
        return Ok(());
    }
    let window = is_window(r.priority, r.total_ms, sh.host.caps.lock().window_max_s);
    // A window has its own count, gate and limits, so it never waits behind a long recording.
    let (jobs, gate, max_waiting) = if window {
        (sh.host.window_jobs.clone(), sh.host.window_gate.clone(), MAX_WINDOWS_WAITING)
    } else {
        (sh.host.speech_jobs.clone(), sh.host.speech_gate.clone(), MAX_WAITING)
    };
    let ahead = jobs.fetch_add(1, Ordering::Relaxed);
    let mut job = SpeechJob { jobs: jobs.clone(), window_of: None };
    if window {
        // One phone should send one window at a time. A second one in flight is allowed, a third is not.
        let allowed = {
            let mut by_phone = sh.host.window_by_phone.lock();
            let mine = by_phone.entry(client.fp.clone()).or_insert(0);
            if *mine >= MAX_WINDOWS_PER_PHONE {
                false
            } else {
                *mine += 1;
                true
            }
        };
        if !allowed {
            wire::write_frame(send, &TranscribeEvent::Error("busy".into())).await?;
            return Ok(());
        }
        job.window_of = Some((sh.clone(), client.fp.clone()));
    }
    if ahead > max_waiting {
        wire::write_frame(send, &TranscribeEvent::Error("busy".into())).await?;
        return Ok(());
    }
    if ahead > 0 {
        wire::write_frame(send, &TranscribeEvent::Queued { ahead }).await?;
    }
    let mut beat = tokio::time::interval(Duration::from_secs(2));

    // The audio arrives first, while the heartbeat tells the phone this node is alive.
    let max_audio_ms = if window { r.total_ms + WINDOW_SLACK_MS } else { MAX_AUDIO_MS };
    let mut reader = tokio::spawn(read_audio(recv, r.total_ms, max_audio_ms));
    let pcm = loop {
        tokio::select! {
            res = &mut reader => break res,
            _ = beat.tick() => {
                if wire::write_frame(send, &TranscribeEvent::Working).await.is_err() {
                    reader.abort();
                    return Ok(());
                }
            }
            _ = conn.closed() => {
                reader.abort();
                return Ok(());
            }
        }
    };
    let pcm = match pcm {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            sh.metrics.speech_failed(window, &client.fp, &client.name, 0);
            wire::write_frame(send, &TranscribeEvent::Error(format!("{e:#}"))).await?;
            return Ok(());
        }
        Err(_) => return Ok(()),
    };
    let audio_ms = pcm.len() as u64 / 16;

    // One at a time per lane, in the order they came (the semaphore is first come, first served, so a
    // window is never passed by a later one).
    let queued_at = Instant::now();
    let mut wait = Box::pin(gate.acquire_owned());
    let permit = loop {
        tokio::select! {
            p = &mut wait => match p {
                Ok(p) => break p,
                Err(_) => return Ok(()),
            },
            _ = beat.tick() => {
                if window && queued_at.elapsed() > WINDOW_MAX_WAIT {
                    wire::write_frame(send, &TranscribeEvent::Error("busy".into())).await?;
                    return Ok(());
                }
                let ahead = jobs.load(Ordering::Relaxed).saturating_sub(1);
                if wire::write_frame(send, &TranscribeEvent::Queued { ahead }).await.is_err() {
                    return Ok(());
                }
            }
            _ = conn.closed() => return Ok(()),
        }
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<TranscribeEvent>();
    let cancel = Arc::new(AtomicBool::new(false));
    let (cancel2, language, prompt) = (cancel.clone(), r.language.clone(), r.prompt.clone());
    let work = tokio::task::spawn_blocking(move || {
        // The permit and the job count are held until whisper has really stopped.
        let (_permit, _job) = (permit, job);
        let started = Instant::now();
        let audio: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        drop(pcm);
        let (tx_seg, tx_prog) = (tx.clone(), tx.clone());
        let mut last_pct = -1;
        let res = whisper.transcribe(
            &audio,
            engine::whisper::Job { language: language.as_deref(), prompt: prompt.as_deref(), window },
            &cancel2,
            &mut |seg| {
                let _ = tx_seg.send(TranscribeEvent::Segment { t0_ms: seg.t0_ms, t1_ms: seg.t1_ms, text: seg.text });
            },
            &mut |pct| {
                if pct != last_pct {
                    last_pct = pct;
                    let _ = tx_prog.send(TranscribeEvent::Progress { done_ms: audio_ms * pct.clamp(0, 100) as u64 / 100, total_ms: audio_ms });
                }
            },
        );
        drop(audio);
        let took = started.elapsed();
        // Handing memory back takes a moment, so only a long job does it.
        if !window {
            engine::whisper::give_back_memory();
        }
        (res, took)
    });

    let mut gone = false;
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => match ev { Some(e) => e, None => break },
            _ = beat.tick() => TranscribeEvent::Working,
            _ = conn.closed() => { gone = true; break }
        };
        if wire::write_frame(send, &ev).await.is_err() {
            gone = true;
            break;
        }
    }
    if gone {
        // The phone went away or gave up: stop working for nobody. The audio is freed when whisper stops.
        cancel.store(true, Ordering::Relaxed);
        let sh = sh.clone();
        tokio::spawn(async move {
            let _ = work.await;
            sh.metrics.speech_failed(window, &client.fp, &client.name, audio_ms);
        });
        return Ok(());
    }
    let (res, took) = match work.await {
        Ok(x) => x,
        Err(e) => {
            sh.metrics.speech_failed(window, &client.fp, &client.name, audio_ms);
            wire::write_frame(send, &TranscribeEvent::Error(format!("{e}"))).await?;
            return Ok(());
        }
    };
    let ev = match res {
        Ok(out) => {
            sh.metrics.speech_finished(window, &client.fp, &client.name, audio_ms, took.as_millis() as u64);
            // Windows are not folded into the speed phones see: it is the speed on whole recordings.
            if !window && audio_ms >= 20_000 && !took.is_zero() {
                sh.host.note_speech_speed(audio_ms as f32 / took.as_millis().max(1) as f32);
            }
            TranscribeEvent::Done { language: out.language, segments: out.segments, elapsed_ms: took.as_millis().min(u32::MAX as u128) as u32 }
        }
        Err(e) => {
            sh.metrics.speech_failed(window, &client.fp, &client.name, audio_ms);
            TranscribeEvent::Error(format!("{e:#}"))
        }
    };
    wire::write_frame(send, &ev).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_interactive_audio_is_a_window() {
        assert!(is_window(Priority::Interactive, 40_000, 120));
        assert!(is_window(Priority::Interactive, 3_000, 60));
        assert!(is_window(Priority::Interactive, 60_000, 60));
        assert!(!is_window(Priority::Interactive, 60_001, 60));
        assert!(!is_window(Priority::Interactive, 600_000, 120));
        assert!(!is_window(Priority::Background, 40_000, 120));
        assert!(!is_window(Priority::Bulk, 10_000, 120));
        // No short lane when the node has no speech model to announce a length for.
        assert!(!is_window(Priority::Interactive, 10_000, 0));
    }
}
