//! The phone's side of a conversation with a node (also used by the node's own `probe` command).

use crate::msg::*;
use crate::{pair, tls, wire};
use anyhow::{anyhow, bail, Result};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One UDP socket serves every connection.
pub fn new_endpoint() -> Result<quinn::Endpoint> {
    Ok(quinn::Endpoint::client((std::net::Ipv4Addr::UNSPECIFIED, 0).into())?)
}

/// Opens a connection. With `pin`, only the node with that fingerprint is accepted. Returns the
/// fingerprint of the node that answered.
pub async fn connect(ep: &quinn::Endpoint, id: &tls::Identity, addr: SocketAddr, pin: Option<String>, wait: Duration) -> Result<(quinn::Connection, String)> {
    let seen = Arc::new(Mutex::new(None));
    let cfg = tls::client_config(id, pin, seen.clone())?;
    let connecting = ep.connect_with(cfg, addr, crate::SERVER_NAME)?;
    let conn = tokio::time::timeout(wait, connecting)
        .await
        .map_err(|_| anyhow!("no answer from {addr}"))??;
    let fp = seen.lock().unwrap().clone().ok_or_else(|| anyhow!("no certificate"))?;
    Ok((conn, fp))
}

/// Sends the request and hands back the stream the answer arrives on.
pub async fn open(conn: &quinn::Connection, req: &Request) -> Result<(quinn::SendStream, quinn::RecvStream)> {
    let (mut send, recv) = conn.open_bi().await?;
    wire::write_frame(&mut send, req).await?;
    Ok((send, recv))
}

/// One request, one answer.
pub async fn call<T: serde::de::DeserializeOwned>(conn: &quinn::Connection, req: &Request) -> Result<T> {
    let (mut send, mut recv) = open(conn, req).await?;
    let _ = send.finish();
    wire::read_frame(&mut recv).await?.ok_or_else(|| anyhow!("the node closed without an answer"))
}

pub async fn status(conn: &quinn::Connection) -> Result<StatusReply> {
    call(conn, &Request::Status).await
}

/// Pairs this phone with the node on `conn` (which must have been opened without a pin).
/// Returns the node's name; the caller pins `node_fp` when this succeeds.
pub async fn pair(conn: &quinn::Connection, id: &tls::Identity, node_fp: &str, name: &str, code: &str) -> Result<String> {
    if tls::peer_fingerprint(conn).as_deref() != Some(node_fp) {
        bail!("the compute node on this connection is not the one being paired");
    }
    let exp = pair::exporter(conn).ok_or_else(|| anyhow!("no session secret"))?;
    let proof = pair::proof(code, pair::Side::Phone, &exp, &id.fingerprint(), node_fp);
    match call::<PairReply>(conn, &Request::Pair { name: name.to_string(), proof }).await? {
        PairReply::Ok { node_name, proof } => {
            let expect = pair::proof(code, pair::Side::Node, &exp, &id.fingerprint(), node_fp);
            if !pair::proofs_equal(&proof, &expect) {
                bail!("this compute node did not know the code");
            }
            Ok(node_name)
        }
        PairReply::Refused(why) => bail!(why),
    }
}

/// Asks to be let in without a code. The caller has already asked the person whether they trust
/// the compute node, and pins `node_fp` (returned by [`connect`]) when this does not fail.
pub async fn knock(conn: &quinn::Connection, name: &str) -> Result<KnockReply> {
    call(conn, &Request::Knock { name: name.to_string() }).await
}

pub async fn knock_status(conn: &quinn::Connection) -> Result<KnockReply> {
    call(conn, &Request::KnockStatus).await
}

pub async fn decide(conn: &quinn::Connection, who: &str, allow: bool) -> Result<()> {
    match call::<DecideReply>(conn, &Request::Decide { who: who.to_string(), allow }).await? {
        DecideReply::Ok => Ok(()),
        DecideReply::Refused(why) => bail!(why),
    }
}
