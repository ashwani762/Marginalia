//! Compute nodes that work together. Two nodes become peers when one joins the other with the one time code
//! (`marginalia-node join`). Peers trust each other's certificate both ways, copy the phone records to each other
//! (see [`crate::records`]), keep track of each other's load and hand jobs to the node that can answer soonest.
//! Peers never share job content: a forwarded job lives in memory for as long as the request, like any other.

use crate::peers::{Peer, Peers};
use crate::place::{self, Job, Node};
use crate::state::Store;
use anyhow::{anyhow, bail, Result};
use marginalia_proto::tls::Identity;
use marginalia_proto::{client, pair, wire, ChatEvent, ChatRequest, EmbedReply, EmbedRequest, PeerReply, Request, StatusReply};
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often peers are asked how busy they are.
const STATUS_EVERY: Duration = Duration::from_secs(2);
/// How often peers trade phone records, besides at once when one changes.
const RECORDS_EVERY: Duration = Duration::from_secs(3);
const CONNECT_WAIT: Duration = Duration::from_secs(3);
const CALL_WAIT: Duration = Duration::from_secs(3);
/// A peer whose last status is older than this is not counted on.
const FRESH: Duration = Duration::from_secs(8);
/// A peer that works on a forwarded job sends something every 2 seconds. Silence for this long means it is lost.
const SILENCE: Duration = Duration::from_secs(10);
/// How long after a pairing with the code the other node may still turn it into a peer.
const JOIN_WINDOW: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Live {
    status: Option<StatusReply>,
    seen: Option<Instant>,
    up: bool,
    rtt_ms: u32,
}

struct RecentPair {
    fp: String,
    code: String,
    until: Instant,
}

pub struct Cluster {
    pub peers: Peers,
    store: Arc<Store>,
    identity: Identity,
    name: String,
    /// The port this node listens on, told to peers so they can find it.
    port: u16,
    ep: quinn::Endpoint,
    conns: Mutex<HashMap<String, quinn::Connection>>,
    live: Mutex<HashMap<String, Live>>,
    /// Jobs this node has sent to each peer that are not finished. A peer's status is two seconds old, this
    /// keeps a burst of jobs from all going to the same peer.
    sent: Mutex<HashMap<String, u32>>,
    recent_pair: Mutex<Option<RecentPair>>,
    /// Jobs answered by a peer for this node since it started.
    pub forwarded: AtomicU64,
}

/// Counts a job sent to a peer until it ends.
struct Sent<'a> {
    c: &'a Cluster,
    fp: String,
}

impl<'a> Sent<'a> {
    fn new(c: &'a Cluster, fp: &str) -> Sent<'a> {
        *c.sent.lock().entry(fp.to_string()).or_insert(0) += 1;
        Sent { c, fp: fp.to_string() }
    }
}

impl Drop for Sent<'_> {
    fn drop(&mut self) {
        let mut m = self.c.sent.lock();
        if let Some(n) = m.get_mut(&self.fp) {
            *n = n.saturating_sub(1);
        }
    }
}

/// What became of a chat job offered to peers.
pub enum Forward {
    /// A peer answered it, or the phone went away. Nothing is left to do.
    Done,
    /// No peer began, so nothing reached the phone.
    No,
    /// A peer began and then stopped. These words were sent on to the phone already.
    Broke(String),
}

enum Next<T> {
    Got(T),
    /// The peer closed, failed or said nothing for [`SILENCE`].
    Lost,
    PhoneGone,
}

/// The next message from a peer that works on a forwarded job. While it is silent the phone is sent `beat` every
/// 2 seconds, so the phone does not take a wait for the peer as a lost node.
async fn next_frame<T: serde::de::DeserializeOwned, B: serde::Serialize>(from_peer: &mut quinn::RecvStream, to_phone: &mut quinn::SendStream, beat: &B) -> Next<T> {
    let mut read = Box::pin(wire::read_frame::<T>(from_peer));
    let every = Duration::from_secs(2);
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    let mut quiet = Duration::ZERO;
    loop {
        tokio::select! {
            r = &mut read => return match r {
                Ok(Some(ev)) => Next::Got(ev),
                _ => Next::Lost,
            },
            _ = tick.tick() => {
                quiet += every;
                if quiet >= SILENCE {
                    return Next::Lost;
                }
                if wire::write_frame(to_phone, beat).await.is_err() {
                    return Next::PhoneGone;
                }
            }
        }
    }
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Cluster {
    pub fn new(store: Arc<Store>, peers: Peers, identity: Identity, name: String, port: u16) -> Result<Arc<Cluster>> {
        Ok(Arc::new(Cluster {
            peers,
            store,
            identity,
            name,
            port,
            ep: client::new_endpoint()?,
            conns: Default::default(),
            live: Default::default(),
            sent: Default::default(),
            recent_pair: Default::default(),
            forwarded: AtomicU64::new(0),
        }))
    }

    /// Starts keeping track of the peers: their load every 2 seconds, the phone records every 3 seconds and at once
    /// when one changes.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(STATUS_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let mut round = tokio::task::JoinSet::new();
                for p in me.peers.list() {
                    round.spawn(me.clone().poll(p));
                }
                while round.join_next().await.is_some() {}
            }
        });
        let me = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(RECORDS_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = me.store.changed.notified() => tokio::time::sleep(Duration::from_millis(50)).await,
                }
                let mut round = tokio::task::JoinSet::new();
                for p in me.peers.list() {
                    round.spawn(me.clone().trade_records(p));
                }
                while round.join_next().await.is_some() {}
            }
        });
    }

    /// Closes the connections to peers when the node stops.
    pub fn shutdown(&self) {
        self.ep.close(0u32.into(), b"bye");
    }

    async fn connection(&self, p: &Peer) -> Result<quinn::Connection> {
        let cached = self.conns.lock().get(&p.fp).filter(|c| c.close_reason().is_none()).cloned();
        if let Some(c) = cached {
            return Ok(c);
        }
        let addr: SocketAddr = p.addr.parse().map_err(|_| anyhow!("{} is not an address", p.addr))?;
        let (conn, _) = client::connect(&self.ep, &self.identity, addr, Some(p.fp.clone()), CONNECT_WAIT).await?;
        self.conns.lock().insert(p.fp.clone(), conn.clone());
        Ok(conn)
    }

    fn drop_connection(&self, fp: &str) {
        if let Some(c) = self.conns.lock().remove(fp) {
            c.close(0u32.into(), b"bye");
        }
    }

    async fn poll(self: Arc<Self>, p: Peer) {
        let t = Instant::now();
        let res = async {
            let c = self.connection(&p).await?;
            tokio::time::timeout(CALL_WAIT, client::status(&c)).await.map_err(|_| anyhow!("no answer"))?
        }
        .await;
        let mut live = self.live.lock();
        let l = live.entry(p.fp.clone()).or_default();
        match res {
            Ok(st) => {
                if !l.up {
                    tracing::info!("peer {} is reachable", p.name);
                }
                *l = Live { status: Some(st), seen: Some(Instant::now()), up: true, rtt_ms: t.elapsed().as_millis().min(u32::MAX as u128) as u32 };
            }
            Err(e) => {
                if l.up {
                    tracing::info!("peer {} is not answering", p.name);
                }
                l.up = false;
                drop(live);
                tracing::debug!("peer {}: {e:#}", p.name);
                self.drop_connection(&p.fp);
            }
        }
    }

    async fn trade_records(self: Arc<Self>, p: Peer) {
        let res = async {
            let c = self.connection(&p).await?;
            tokio::time::timeout(CALL_WAIT, client::records(&c, self.port, self.store.records())).await.map_err(|_| anyhow!("no answer"))?
        }
        .await;
        match res {
            Ok(theirs) => match self.store.merge(&theirs) {
                Ok(0) => {}
                Ok(n) => tracing::info!("took {n} phone record(s) from {}", p.name),
                Err(e) => tracing::warn!("could not save phone records from {}: {e:#}", p.name),
            },
            Err(e) => {
                tracing::debug!("records with {}: {e:#}", p.name);
                self.drop_connection(&p.fp);
            }
        }
    }

    /// `marginalia-node join`: pairs with the node at `addr` the way a phone does, then says it is a node.
    pub async fn join(&self, addr: &str, code: &str) -> Result<Peer> {
        let addr = tokio::net::lookup_host(addr)
            .await
            .ok()
            .and_then(|mut a| a.find(|a| a.is_ipv4()))
            .ok_or_else(|| anyhow!("give the address of the other compute node as ADDRESS:PORT, for example 192.168.1.20:47615"))?;
        let (conn, node_fp) = client::connect(&self.ep, &self.identity, addr, None, Duration::from_secs(5)).await.map_err(|e| anyhow!("could not reach a compute node at {addr}: {e}"))?;
        if node_fp == self.identity.fingerprint() {
            bail!("that is this compute node");
        }
        if self.peers.find(&node_fp).is_some() {
            bail!("that compute node is already a peer");
        }
        let node_name = client::pair(&conn, &self.identity, &node_fp, &self.name, code).await?;
        client::join(&conn, &self.identity, &node_fp, &self.name, self.port, code)
            .await
            .map_err(|e| anyhow!("paired with {node_name}, but it did not take this compute node as a peer: {e}. Run `marginalia-node revoke` there for this node and try again"))?;
        let peer = Peer { fp: node_fp, name: node_name, addr: addr.to_string(), added_at: now_s() };
        self.peers.add(peer.clone())?;
        conn.close(0u32.into(), b"done");
        // Trade phone records and load now instead of at the next tick.
        self.store.changed.notify_one();
        tracing::info!("joined {} as a peer", peer.name);
        Ok(peer)
    }

    /// `marginalia-node unjoin`: forgets the peers matching `who` and asks them to forget this node too.
    pub async fn unjoin(&self, who: &str) -> Result<Vec<Peer>> {
        let gone = self.peers.remove(who)?;
        if gone.is_empty() {
            bail!("no peer matches that id or name");
        }
        for p in &gone {
            // The other node may be off. Then it forgets this one when it is next told, or it stays a stranger here.
            if let Ok(c) = self.connection(p).await {
                let _ = tokio::time::timeout(CALL_WAIT, client::leave(&c)).await;
            }
            self.drop_connection(&p.fp);
            self.live.lock().remove(&p.fp);
            tracing::info!("{} is no longer a peer", p.name);
        }
        Ok(gone)
    }

    /// A phone just paired with the code. If it is a compute node it follows with `Join` within a minute.
    pub fn note_pair(&self, fp: &str, code: &str) {
        *self.recent_pair.lock() = Some(RecentPair { fp: fp.to_string(), code: code.to_string(), until: Instant::now() + JOIN_WINDOW });
    }

    /// Answers `Join`: only the device that has just paired with the code can become a peer, and it must prove it still knows it.
    pub fn accept_join(&self, conn: &quinn::Connection, fp: &str, name: String, port: u16, proof: [u8; 32]) -> PeerReply {
        let code = {
            let mut g = self.recent_pair.lock();
            match g.as_ref() {
                Some(r) if r.fp == fp && Instant::now() < r.until => g.take().map(|r| r.code),
                _ => None,
            }
        };
        let Some(code) = code else {
            return PeerReply::Refused("This compute node did not just pair with you using its code. Run `marginalia-node pair` there and join again.".into());
        };
        let Some(exp) = pair::exporter(conn) else {
            return PeerReply::Refused("No session secret.".into());
        };
        if !pair::proofs_equal(&pair::proof(&code, pair::Side::Peer, &exp, fp, &self.identity.fingerprint()), &proof) || port == 0 {
            return PeerReply::Refused("That code is not right.".into());
        }
        let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
        let name = if name.trim().is_empty() { "A compute node".to_string() } else { name };
        let peer = Peer { fp: fp.to_string(), name: name.clone(), addr: SocketAddr::new(conn.remote_address().ip(), port).to_string(), added_at: now_s() };
        // It paired as a phone a moment ago. It is a peer now, so the phone record is withdrawn, and that spreads too.
        if let Err(e) = self.peers.add(peer).and_then(|_| self.store.revoke(fp).map(|_| ())) {
            return PeerReply::Refused(format!("Could not save the peer: {e}"));
        }
        tracing::info!("{name} joined as a peer");
        PeerReply::Ok
    }

    /// A peer connected and told its port: remember where it is.
    pub fn peer_seen(&self, fp: &str, ip: IpAddr, port: u16) {
        if port != 0 {
            let _ = self.peers.set_addr(fp, &SocketAddr::new(ip, port).to_string());
        }
    }

    /// A peer asked to be forgotten.
    pub fn peer_left(&self, fp: &str) {
        if let Ok(gone) = self.peers.remove(fp) {
            for p in gone {
                tracing::info!("{} left", p.name);
                self.drop_connection(&p.fp);
                self.live.lock().remove(&p.fp);
            }
        }
    }

    fn fresh(l: &Live) -> Option<&StatusReply> {
        l.status.as_ref().filter(|_| l.up && l.seen.map(|t| t.elapsed() < FRESH).unwrap_or(false))
    }

    /// The peers to try for a job, best first. Empty when this node should run it.
    pub fn plan(&self, job: Job, local: &Node) -> Vec<Peer> {
        let peers = self.peers.list();
        if peers.is_empty() {
            return vec![];
        }
        let sent = self.sent.lock().clone();
        let live = self.live.lock();
        let seen: Vec<(String, Node)> = peers
            .iter()
            .filter_map(|p| {
                let st = Self::fresh(live.get(&p.fp)?)?;
                let (serves, speed) = match job {
                    Job::Chat { .. } => (st.caps.chat.is_some(), st.caps.chat_tok_s),
                    Job::Embed { .. } => (st.caps.embed.is_some(), st.caps.embed_per_s),
                };
                Some((p.fp.clone(), Node { load: st.running + st.waiting + sent.get(&p.fp).copied().unwrap_or(0), paused: st.paused, speed, serves }))
            })
            .collect();
        drop(live);
        place::choose(job, local, &seen).into_iter().filter_map(|fp| peers.iter().find(|p| p.fp == fp).cloned()).collect()
    }

    /// Runs a chat job on the first peer that takes it and relays the answer to the phone. [`Forward::No`] means no peer
    /// produced a first token, so nothing has reached the phone and the caller runs the job itself. [`Forward::Broke`] means
    /// the peer stopped after some words were sent on, and the caller carries on from those words.
    pub async fn forward_chat(&self, peers: Vec<Peer>, r: &ChatRequest, send: &mut quinn::SendStream) -> Result<Forward> {
        for p in peers {
            let _count = Sent::new(self, &p.fp);
            let opened = async {
                let c = self.connection(&p).await?;
                tokio::time::timeout(CALL_WAIT, client::open(&c, &Request::Chat(r.clone()))).await.map_err(|_| anyhow!("no answer"))?
            }
            .await;
            let (mut psend, mut precv) = match opened {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("chat to {}: {e:#}", p.name);
                    self.drop_connection(&p.fp);
                    continue;
                }
            };
            let _ = psend.finish();
            // The words that went on to the phone, so an answer that breaks off can be carried on from them.
            let mut sent_on = String::new();
            loop {
                let began = !sent_on.is_empty();
                let ev = match next_frame::<ChatEvent, _>(&mut precv, send, &ChatEvent::Working).await {
                    Next::Got(ev) => ev,
                    Next::PhoneGone => {
                        let _ = precv.stop(0u32.into());
                        return Ok(Forward::Done);
                    }
                    Next::Lost if !began => {
                        tracing::info!("{} went quiet before it answered, trying another node", p.name);
                        self.drop_connection(&p.fp);
                        break;
                    }
                    Next::Lost => {
                        tracing::info!("{} stopped answering after {} bytes, carrying on here", p.name, sent_on.len());
                        self.drop_connection(&p.fp);
                        let _ = precv.stop(0u32.into());
                        return Ok(Forward::Broke(sent_on));
                    }
                };
                if let ChatEvent::Error(why) = &ev {
                    if !began {
                        tracing::info!("{} turned a chat job away ({why}), trying another node", p.name);
                        break;
                    }
                    tracing::info!("{} failed after {} bytes ({why}), carrying on here", p.name, sent_on.len());
                    let _ = precv.stop(0u32.into());
                    return Ok(Forward::Broke(sent_on));
                }
                if let ChatEvent::Token(t) = &ev {
                    sent_on.push_str(t);
                }
                let last = matches!(ev, ChatEvent::Done { .. });
                if wire::write_frame(send, &ev).await.is_err() {
                    // The phone went away: stop the peer from writing for nobody.
                    let _ = precv.stop(0u32.into());
                    return Ok(Forward::Done);
                }
                if last {
                    self.forwarded.fetch_add(1, Ordering::Relaxed);
                    return Ok(Forward::Done);
                }
            }
            let _ = precv.stop(0u32.into());
        }
        Ok(Forward::No)
    }

    /// Like [`Cluster::forward_chat`] for search vectors.
    pub async fn forward_embed(&self, peers: Vec<Peer>, r: &EmbedRequest, send: &mut quinn::SendStream) -> Result<bool> {
        for p in peers {
            let _count = Sent::new(self, &p.fp);
            let opened = async {
                let c = self.connection(&p).await?;
                tokio::time::timeout(CALL_WAIT, client::open(&c, &Request::Embed(r.clone()))).await.map_err(|_| anyhow!("no answer"))?
            }
            .await;
            let (mut psend, mut precv) = match opened {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("search to {}: {e:#}", p.name);
                    self.drop_connection(&p.fp);
                    continue;
                }
            };
            let _ = psend.finish();
            loop {
                match next_frame::<EmbedReply, _>(&mut precv, send, &EmbedReply::Queued { ahead: 0 }).await {
                    Next::Got(EmbedReply::Error(why)) => {
                        tracing::info!("{} turned a search job away ({why}), trying another node", p.name);
                        break;
                    }
                    Next::Got(ev) => {
                        let last = matches!(ev, EmbedReply::Vectors(_));
                        if wire::write_frame(send, &ev).await.is_err() {
                            let _ = precv.stop(0u32.into());
                            return Ok(true);
                        }
                        if last {
                            self.forwarded.fetch_add(1, Ordering::Relaxed);
                            return Ok(true);
                        }
                    }
                    Next::PhoneGone => {
                        let _ = precv.stop(0u32.into());
                        return Ok(true);
                    }
                    Next::Lost => {
                        tracing::info!("{} went quiet before it answered, trying another node", p.name);
                        self.drop_connection(&p.fp);
                        break;
                    }
                }
            }
            let _ = precv.stop(0u32.into());
        }
        Ok(false)
    }

    /// The peers as the control channel shows them: names, addresses and load, nothing else.
    pub fn snapshot(&self) -> Vec<Value> {
        let live = self.live.lock();
        self.peers
            .list()
            .iter()
            .map(|p| {
                let l = live.get(&p.fp);
                let up = l.and_then(Self::fresh).is_some();
                let st = l.and_then(|l| l.status.as_ref());
                json!({
                    "name": p.name, "id": &p.fp[..16], "addr": p.addr, "reachable": up,
                    "last_seen_s": l.and_then(|l| l.seen).map(|t| t.elapsed().as_secs()),
                    "rtt_ms": l.filter(|_| up).map(|l| l.rtt_ms),
                    "load": st.map(|s| s.running + s.waiting), "paused": st.map(|s| s.paused),
                    "chat": st.map(|s| s.caps.chat.is_some()), "embed": st.map(|s| s.caps.embed.is_some()), "speech": st.map(|s| s.caps.transcribe.is_some()),
                })
            })
            .collect()
    }
}
