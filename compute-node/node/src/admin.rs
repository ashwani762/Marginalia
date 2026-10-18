//! The local control channel: `marginalia-node pair`, `status`, `pause`, `stop` and the tray icon talk
//! to the running daemon through a port that only this machine can reach. Replies hold counts, names
//! and timings, never any content.

use crate::server::Shared;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn serve(sh: Arc<Shared>, port: u16, node_port: u16) -> Result<()> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    loop {
        let (mut sock, _) = listener.accept().await?;
        let sh = sh.clone();
        tokio::spawn(async move {
            let (r, mut w) = sock.split();
            let mut line = String::new();
            if BufReader::new(r).read_line(&mut line).await.is_err() {
                return;
            }
            let req: Value = serde_json::from_str(&line).unwrap_or(json!({}));
            let mut stop = false;
            let cmd = req["cmd"].as_str().unwrap_or("").to_string();
            let reply = match cmd.as_str() {
                "pair" => {
                    let code = sh.open_pairing(5);
                    let addrs: Vec<String> = crate::discovery::lan_addresses().iter().map(|a| format!("{a}:{node_port}")).collect();
                    json!({"code": code, "minutes": 5, "name": sh.name, "addresses": addrs})
                }
                "status" => {
                    let caps = sh.host.caps.lock().clone();
                    let in_flight = sh.in_flight();
                    let addrs: Vec<String> = crate::discovery::lan_addresses().iter().map(|a| format!("{a}:{node_port}")).collect();
                    json!({
                        "paused": sh.paused.load(Ordering::Relaxed), "running_jobs": in_flight.min(1), "waiting_jobs": in_flight.saturating_sub(1),
                        "served": sh.served(), "uptime_s": sh.metrics.uptime_s(), "phones": sh.store.list().len(), "version": env!("CARGO_PKG_VERSION"),
                        "port": node_port, "addresses": addrs,
                        "name": sh.name, "fingerprint": sh.identity.fingerprint(), "locked": sh.locked.load(Ordering::Relaxed),
                        "chat": caps.chat, "embed": caps.embed, "chat_tok_s": caps.chat_tok_s, "embed_per_s": caps.embed_per_s,
                        "speech": caps.transcribe, "speech_x": caps.transcribe_x, "window_max_s": caps.window_max_s,
                        "blocked": sh.store.blocked_list().iter().map(|b| json!({"name": b.name, "id": &b.fp[..16], "at": b.at})).collect::<Vec<_>>(),
                        "waiting": sh.pending_list().iter().map(|p| json!({"id": p.id, "name": p.name, "waited_s": p.waited_s})).collect::<Vec<_>>(),
                        "peers": sh.cluster.snapshot(), "nearby": nearby_not_joined(&sh), "forwarded": sh.cluster.forwarded.load(Ordering::Relaxed),
                        "clients": sh.store.list().iter().map(|c| json!({"name": c.name, "fp": &c.fp[..16], "paired_at": c.paired_at})).collect::<Vec<_>>(),
                    })
                }
                "peers" => json!({"peers": sh.cluster.snapshot(), "nearby": nearby_not_joined(&sh)}),
                "join" => match sh.cluster.join(&resolve_nearby(&sh, req["addr"].as_str().unwrap_or("")), req["code"].as_str().unwrap_or("")).await {
                    Ok(p) => json!({"joined": p.name, "id": &p.fp[..16], "addr": p.addr}),
                    Err(e) => json!({"error": format!("{e:#}")}),
                },
                "unjoin" => match sh.cluster.unjoin(req["who"].as_str().unwrap_or("")).await {
                    Ok(gone) => json!({"unjoined": gone.iter().map(|p| p.name.clone()).collect::<Vec<_>>()}),
                    Err(e) => json!({"error": format!("{e:#}")}),
                },
                "pause" | "resume" => {
                    let on = cmd == "pause";
                    sh.paused.store(on, Ordering::Relaxed);
                    if let Some(a) = sh.announce.get() {
                        a.set_paused(on);
                    }
                    tracing::info!("{}", if on { "paused: new jobs are turned away" } else { "resumed" });
                    json!({"paused": on})
                }
                "shutdown" => {
                    // The reply goes out first, then main is told to wind down.
                    stop = true;
                    json!({"stopping": true})
                }
                "approve" | "deny" => {
                    let allow = req["cmd"] == "approve";
                    let pending = sh.pending_list();
                    let who = req["who"].as_str().map(String::from).or_else(|| (pending.len() == 1).then(|| pending[0].id.clone()));
                    match who {
                        None if pending.is_empty() => json!({"error": "no phone is waiting"}),
                        None => json!({"error": "more than one phone is waiting, name one", "waiting": pending.iter().map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>()}),
                        Some(w) => match sh.decide(&w, allow) {
                            Ok(name) => json!({"done": if allow { "allowed" } else { "turned down" }, "name": name}),
                            Err(e) => json!({"error": e}),
                        },
                    }
                }
                "block" => {
                    let who = req["who"].as_str().unwrap_or("");
                    if who.len() < 4 {
                        json!({"error": "give at least 4 characters of the phone's id, or its name"})
                    } else {
                        let waiting: Vec<(String, String)> = sh.waiting.lock().iter().map(|(fp, w)| (fp.clone(), w.name.clone())).collect();
                        match sh.store.block(who, &waiting) {
                            Ok(names) => {
                                sh.waiting.lock().retain(|fp, _| sh.store.is_blocked(fp) == false);
                                json!({"blocked": names})
                            }
                            Err(e) => json!({"error": e.to_string()}),
                        }
                    }
                }
                "unblock" => json!({"unblocked": sh.store.unblock(req["who"].as_str().unwrap_or("")).unwrap_or(0)}),
                "lock" | "unlock" => {
                    let on = req["cmd"] == "lock";
                    sh.locked.store(on, Ordering::Relaxed);
                    match sh.store.set_locked(on) {
                        Ok(()) => json!({"locked": on}),
                        Err(e) => json!({"error": e.to_string()}),
                    }
                }
                "stats" => {
                    let mut v = sh.metrics.snapshot();
                    v["name"] = json!(sh.name);
                    v["paused"] = json!(sh.paused.load(Ordering::Relaxed));
                    v["locked"] = json!(sh.locked.load(Ordering::Relaxed));
                    v["report"] = json!(crate::metrics::report(&v));
                    v
                }
                "revoke" => {
                    let who = req["who"].as_str().unwrap_or("");
                    if who.len() < 4 {
                        json!({"error": "give at least 4 characters of the phone's id, or its name"})
                    } else {
                        json!({"removed": sh.store.revoke(who).unwrap_or(0)})
                    }
                }
                _ => json!({"error": "unknown command"}),
            };
            let _ = w.write_all(format!("{reply}\n").as_bytes()).await;
            let _ = w.flush().await;
            if stop {
                sh.stop.notify_one();
            }
        });
    }
}

/// The command-line side: send one command, print the answer.
pub async fn ask(port: u16, cmd: Value) -> Result<Value> {
    let mut sock = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|_| anyhow::anyhow!("the node is not running (nothing is listening on 127.0.0.1:{port}). Start it with `marginalia-node run`."))?;
    sock.write_all(format!("{cmd}\n").as_bytes()).await?;
    let mut line = String::new();
    BufReader::new(sock).read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}

/// Nodes seen on the network that are not peers yet, as the control channel shows them.
fn nearby_not_joined(sh: &Shared) -> Vec<serde_json::Value> {
    let joined: Vec<String> = sh.cluster.peers.list().iter().map(|p| p.fp.clone()).collect();
    sh.nearby
        .list()
        .into_iter()
        .filter(|n| !joined.iter().any(|fp| fp.starts_with(&n.id)))
        .map(|n| json!({"name": n.name, "id": n.id, "addr": n.address(), "paused": n.paused}))
        .collect()
}

/// `join "Kitchen node" CODE` works when that node was seen on the network. Anything else is passed on as an address.
fn resolve_nearby(sh: &Shared, who: &str) -> String {
    sh.nearby.find(who).and_then(|n| n.address()).unwrap_or_else(|| who.to_string())
}
