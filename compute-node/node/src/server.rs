//! Answers phones. One task per connection, one per request. A job's text and answer exist only in
//! this task's memory and go away when it ends.

use crate::engine::{self, GenEvent, GenParams, GenRequest};
use crate::host::Host;
use crate::metrics::Metrics;
use crate::state::Store;
use anyhow::Result;
use marginalia_proto::tls::{self, Identity};
use marginalia_proto::{pair, wire, ChatEvent, DecideReply, EmbedReply, KnockReply, PairReply, PendingPhone, Priority, Request, StatusReply};
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
        self.host.chat_jobs.load(Ordering::Relaxed) + self.host.embed_jobs.load(Ordering::Relaxed)
    }

    /// Chat and search jobs finished since the node started.
    pub fn served(&self) -> u64 {
        self.metrics.chat_jobs.load(Ordering::Relaxed) + self.metrics.embed_jobs.load(Ordering::Relaxed)
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
    let known = sh.store.find(&fp);
    let limit = if known.is_some() { 64 * 1024 * 1024 } else { 4 * 1024 };
    let Some(req) = wire::read_frame_max::<Request>(&mut recv, limit).await? else { return Ok(()) };
    match (req, known) {
        (Request::Pair { name, proof }, None) => {
            let reply = try_pair(&sh, &conn, &fp, name, proof);
            wire::write_frame(&mut send, &reply).await?;
        }
        (Request::Pair { .. }, Some(_)) => {
            wire::write_frame(&mut send, &PairReply::Refused("This phone is already paired.".into())).await?;
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
        (Request::Chat(r), Some(c)) => chat(&sh, r, &mut send, c).await?,
        (Request::Embed(r), Some(c)) => embed(&sh, r, &mut send, c).await?,
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
    *guard = None;
    if let Err(e) = sh.store.add(fp.to_string(), name.clone()) {
        return PairReply::Refused(format!("Could not save the pairing: {e}"));
    }
    tracing::info!("paired with {name}");
    sh.metrics.pairings.fetch_add(1, Ordering::Relaxed);
    reply
}

async fn chat(sh: &Arc<Shared>, r: marginalia_proto::ChatRequest, send: &mut quinn::SendStream, client: crate::state::Client) -> Result<()> {
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

async fn embed(sh: &Arc<Shared>, r: marginalia_proto::EmbedRequest, send: &mut quinn::SendStream, client: crate::state::Client) -> Result<()> {
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
    sh.metrics.embed_done(&client.fp, &client.name, count, t0.elapsed().as_millis() as u64, matches!(out, Ok(Ok(_))));
    let reply = match out {
        Ok(Ok(v)) => EmbedReply::Vectors(v),
        Ok(Err(e)) => EmbedReply::Error(format!("{e:#}")),
        Err(e) => EmbedReply::Error(format!("{e}")),
    };
    wire::write_frame(send, &reply).await?;
    Ok(())
}
