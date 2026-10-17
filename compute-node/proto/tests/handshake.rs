//! End to end tests of identity, pinning, pairing and framing over real QUIC on loopback.
//!
//! The fake node here implements only Status and Pair, with the same rules as the real node:
//! unknown fingerprints may only pair, the proof is checked against the TLS exporter and both
//! fingerprints, and the code is single use.

use marginalia_proto::tls::{self, Identity};
use marginalia_proto::*;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

/// Every test runs under a deadline so a protocol bug fails instead of hanging.
async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), f).await.expect("test timed out")
}

fn loopback() -> SocketAddr {
    (std::net::Ipv4Addr::LOCALHOST, 0).into()
}

// ---------------------------------------------------------------------------------------------
// A minimal node
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct State {
    code: Mutex<Option<String>>,
    /// fingerprint -> phone name
    paired: Mutex<HashMap<String, String>>,
    /// (fingerprint, was it paired) for every request that arrived
    seen: Mutex<Vec<(String, bool)>>,
    /// every Pair answer the node gave
    pair_replies: Mutex<Vec<PairReply>>,
}

struct FakeNode {
    addr: SocketAddr,
    id: Identity,
    state: Arc<State>,
    _ep: quinn::Endpoint,
}

impl FakeNode {
    fn start(code: &str) -> FakeNode {
        let id = Identity::generate().unwrap();
        let state = Arc::new(State::default());
        *state.code.lock().unwrap() = Some(code.to_string());
        let ep = quinn::Endpoint::server(tls::server_config(&id).unwrap(), loopback()).unwrap();
        let addr = ep.local_addr().unwrap();
        let (st, id2, ep2) = (state.clone(), id.clone(), ep.clone());
        tokio::spawn(async move {
            while let Some(incoming) = ep2.accept().await {
                let (st, id) = (st.clone(), id2.clone());
                tokio::spawn(async move {
                    if let Ok(conn) = incoming.await {
                        serve_connection(st, id, conn).await;
                    }
                });
            }
        });
        FakeNode { addr, id, state, _ep: ep }
    }

    fn fp(&self) -> String {
        self.id.fingerprint()
    }

    fn paired_count(&self) -> usize {
        self.state.paired.lock().unwrap().len()
    }
}

async fn serve_connection(st: Arc<State>, id: Identity, conn: quinn::Connection) {
    let Some(fp) = tls::peer_fingerprint(&conn) else {
        conn.close(1u32.into(), b"no certificate");
        return;
    };
    while let Ok((send, recv)) = conn.accept_bi().await {
        let (st, id, conn, fp) = (st.clone(), id.clone(), conn.clone(), fp.clone());
        tokio::spawn(async move {
            let _ = handle(st, id, conn, fp, send, recv).await;
        });
    }
}

async fn handle(st: Arc<State>, id: Identity, conn: quinn::Connection, fp: String, mut send: quinn::SendStream, mut recv: quinn::RecvStream) -> anyhow::Result<()> {
    let Some(req) = wire::read_frame::<Request>(&mut recv).await? else { return Ok(()) };
    let known = st.paired.lock().unwrap().contains_key(&fp);
    st.seen.lock().unwrap().push((fp.clone(), known));
    match (req, known) {
        (Request::Pair { name, proof }, false) => {
            let reply = try_pair(&st, &id, &conn, &fp, name, proof);
            st.pair_replies.lock().unwrap().push(reply.clone());
            wire::write_frame(&mut send, &reply).await?;
        }
        (Request::Pair { .. }, true) => {
            wire::write_frame(&mut send, &PairReply::Refused("already paired".into())).await?;
        }
        (_, false) => {
            conn.close(2u32.into(), b"not paired");
            return Ok(());
        }
        (Request::Status, true) => {
            let reply = StatusReply {
                proto: PROTO_VERSION,
                node_name: "fake-node".into(),
                version: "test".into(),
                caps: Capabilities { chat: None, embed: Some("emb".into()), embed_dim: 768, chat_tok_s: 0.0, embed_per_s: 12.5, ..Default::default() },
                running: 0,
                waiting: 0,
                clients: st.paired.lock().unwrap().len() as u32,
                ..Default::default()
            };
            wire::write_frame(&mut send, &reply).await?;
        }
        (_, true) => {} // Chat and Embed are not part of this fake.
    }
    let _ = send.finish();
    let _ = tokio::time::timeout(WAIT, send.stopped()).await;
    Ok(())
}

fn try_pair(st: &State, id: &Identity, conn: &quinn::Connection, fp: &str, name: String, proof: [u8; 32]) -> PairReply {
    let mut code = st.code.lock().unwrap();
    let Some(c) = code.clone() else { return PairReply::Refused("pairing is not open".into()) };
    let Some(exp) = pair::exporter(conn) else { return PairReply::Refused("no session secret".into()) };
    let node_fp = id.fingerprint();
    let want = pair::proof(&c, pair::Side::Phone, &exp, fp, &node_fp);
    if !pair::proofs_equal(&want, &proof) {
        return PairReply::Refused("That code is not right.".into());
    }
    let reply = PairReply::Ok { node_name: "fake-node".into(), proof: pair::proof(&c, pair::Side::Node, &exp, fp, &node_fp) };
    *code = None;
    st.paired.lock().unwrap().insert(fp.to_string(), name);
    reply
}

// ---------------------------------------------------------------------------------------------
// The phone side
// ---------------------------------------------------------------------------------------------

/// Connects without a pin and tries to pair. On success returns the fingerprint to pin.
async fn try_pairing(ep: &quinn::Endpoint, phone: &Identity, addr: SocketAddr, code: &str) -> anyhow::Result<(String, String)> {
    let (conn, node_fp) = client::connect(ep, phone, addr, None, WAIT).await?;
    let name = client::pair(&conn, phone, &node_fp, "Test phone", code).await?;
    Ok((name, node_fp))
}

async fn pinned_status(ep: &quinn::Endpoint, phone: &Identity, addr: SocketAddr, pin: &str) -> anyhow::Result<StatusReply> {
    let (conn, seen) = client::connect(ep, phone, addr, Some(pin.to_string()), WAIT).await?;
    assert_eq!(seen, pin);
    client::status(&conn).await
}

// ---------------------------------------------------------------------------------------------
// (a) pairing, both proofs, pinned reconnect
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn correct_code_pairs_then_pinned_reconnect_works() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();

        let (name, pin) = try_pairing(&ep, &phone, node.addr, "K7QM-4TDX").await.expect("pairing should succeed");
        assert_eq!(name, "fake-node");
        assert_eq!(pin, node.fp(), "the fingerprint the phone saw is the node's");
        assert_eq!(node.paired_count(), 1);
        assert_eq!(node.state.paired.lock().unwrap().get(&phone.fingerprint()).map(String::as_str), Some("Test phone"));
        assert!(node.state.code.lock().unwrap().is_none(), "the code is single use");

        // A fresh connection pinned to the node, from the same identity, is now served.
        let st = pinned_status(&ep, &phone, node.addr, &pin).await.expect("status after pairing");
        assert_eq!(st.proto, PROTO_VERSION);
        assert_eq!(st.node_name, "fake-node");
        assert_eq!(st.clients, 1);
        assert_eq!(st.caps.embed_dim, 768);
    })
    .await;
}

#[tokio::test]
async fn both_proofs_are_exactly_what_the_rules_say() {
    within(async {
        let node = FakeNode::start("ABCD-2345");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let (conn, node_fp) = client::connect(&ep, &phone, node.addr, None, WAIT).await.unwrap();

        // Both ends of one connection derive the same session secret.
        let exp = pair::exporter(&conn).unwrap();
        let proof = pair::proof("ABCD-2345", pair::Side::Phone, &exp, &phone.fingerprint(), &node_fp);
        let reply: PairReply = client::call(&conn, &Request::Pair { name: "p".into(), proof }).await.unwrap();
        let PairReply::Ok { proof: node_proof, .. } = reply else { panic!("expected Ok, got {reply:?}") };
        assert!(pair::proofs_equal(&node_proof, &pair::proof("ABCD-2345", pair::Side::Node, &exp, &phone.fingerprint(), &node_fp)));
        // The two sides never produce the same value, so one proof cannot be reflected as the other.
        assert!(!pair::proofs_equal(&node_proof, &proof));
        // Neither proof verifies under another code, session or fingerprint.
        let wrong_code = pair::proof("ABCD-2346", pair::Side::Node, &exp, &phone.fingerprint(), &node_fp);
        let wrong_exp = pair::proof("ABCD-2345", pair::Side::Node, &[7u8; 32], &phone.fingerprint(), &node_fp);
        let wrong_fp = pair::proof("ABCD-2345", pair::Side::Node, &exp, &phone.fingerprint(), &phone.fingerprint());
        for bad in [wrong_code, wrong_exp, wrong_fp] {
            assert!(!pair::proofs_equal(&node_proof, &bad));
        }
    })
    .await;
}

#[tokio::test]
async fn exporter_matches_on_both_sides_and_differs_per_connection() {
    within(async {
        let node = FakeNode::start("ABCD-2345");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let (c1, _) = client::connect(&ep, &phone, node.addr, None, WAIT).await.unwrap();
        let (c2, _) = client::connect(&ep, &phone, node.addr, None, WAIT).await.unwrap();
        let (e1, e2) = (pair::exporter(&c1).unwrap(), pair::exporter(&c2).unwrap());
        assert_ne!(e1, e2, "two sessions must not share a secret");

        // The node accepted both connections; its view of the secret is checked by the fake node in
        // every pairing test (a mismatch there would fail pairing). Here compare against a raw pair.
        let proof = pair::proof("ABCD-2345", pair::Side::Phone, &e1, &phone.fingerprint(), &node.fp());
        let reply: PairReply = client::call(&c1, &Request::Pair { name: "p".into(), proof }).await.unwrap();
        assert!(matches!(reply, PairReply::Ok { .. }));
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// (b) wrong code
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn wrong_code_is_refused() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();

        let err = try_pairing(&ep, &phone, node.addr, "K7QM-4TDY").await.expect_err("wrong code must fail");
        assert!(err.to_string().contains("not right"), "unexpected error: {err}");
        assert_eq!(node.paired_count(), 0);
        assert!(node.state.code.lock().unwrap().is_some(), "a wrong guess does not burn the code in the fake");

        // And the phone is still unknown: it cannot get a status.
        let (conn, _) = client::connect(&ep, &phone, node.addr, Some(node.fp()), WAIT).await.unwrap();
        assert!(client::status(&conn).await.is_err());

        // The right code still works afterwards.
        try_pairing(&ep, &phone, node.addr, "K7QM-4TDX").await.expect("correct code after a wrong one");
    })
    .await;
}

#[tokio::test]
async fn pairing_closed_is_refused() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        *node.state.code.lock().unwrap() = None;
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let err = try_pairing(&ep, &phone, node.addr, "K7QM-4TDX").await.expect_err("no open code");
        assert!(err.to_string().contains("not open"), "unexpected error: {err}");
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// (c) wrong pin
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn wrong_pin_fails_the_tls_handshake() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let wrong = "0".repeat(64);
        let err = client::connect(&ep, &phone, node.addr, Some(wrong), WAIT).await.err().expect("connect with a wrong pin must fail");
        let msg = format!("{err:#}");
        assert!(!msg.contains("no answer"), "should fail the handshake, not time out: {msg}");
        assert!(node.state.seen.lock().unwrap().is_empty(), "no request may reach the node");
    })
    .await;
}

#[tokio::test]
async fn an_impostor_at_the_paired_address_is_rejected() {
    within(async {
        let real = FakeNode::start("K7QM-4TDX");
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let (_, pin) = try_pairing(&ep, &phone, real.addr, "K7QM-4TDX").await.unwrap();

        // Another device takes over the address (or answers instead). Same name, other key.
        let impostor = FakeNode::start("K7QM-4TDX");
        assert_ne!(impostor.fp(), pin);
        assert!(pinned_status(&ep, &phone, impostor.addr, &pin).await.is_err());
        // The real one is still fine.
        pinned_status(&ep, &phone, real.addr, &pin).await.unwrap();
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// (d) unknown client
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn unpaired_client_is_recognised_as_unknown_by_fingerprint() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let stranger = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();

        // TLS itself succeeds: a stranger must get as far as asking to pair.
        let (conn, _) = client::connect(&ep, &stranger, node.addr, Some(node.fp()), WAIT).await.unwrap();
        // The server sees exactly the stranger's certificate...
        let server_side = node.state.paired.lock().unwrap().contains_key(&stranger.fingerprint());
        assert!(!server_side);
        // ...but refuses to do any work for it.
        let err = client::status(&conn).await.expect_err("an unpaired phone gets nothing");
        let _ = err;
        let seen = node.state.seen.lock().unwrap().clone();
        assert_eq!(seen, vec![(stranger.fingerprint(), false)]);
        assert_eq!(node.paired_count(), 0);
    })
    .await;
}

#[tokio::test]
async fn two_phones_are_told_apart_and_one_pairing_does_not_admit_the_other() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        assert_ne!(a.fingerprint(), b.fingerprint());
        let ep = client::new_endpoint().unwrap();
        let (_, pin) = try_pairing(&ep, &a, node.addr, "K7QM-4TDX").await.unwrap();
        pinned_status(&ep, &a, node.addr, &pin).await.unwrap();
        assert!(pinned_status(&ep, &b, node.addr, &pin).await.is_err());
        let seen = node.state.seen.lock().unwrap().clone();
        assert!(seen.contains(&(a.fingerprint(), true)));
        assert!(seen.contains(&(b.fingerprint(), false)));
    })
    .await;
}

#[tokio::test]
async fn identity_survives_serialisation_with_the_same_fingerprint() {
    let id = Identity::generate().unwrap();
    let bytes = postcard::to_stdvec(&id).unwrap();
    let back: Identity = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(id.fingerprint(), back.fingerprint());
    assert_eq!(id.fingerprint().len(), 64);
    assert!(id.fingerprint().chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(id.fingerprint(), Identity::generate().unwrap().fingerprint());
}

// ---------------------------------------------------------------------------------------------
// (e) man in the middle
// ---------------------------------------------------------------------------------------------

/// Terminates TLS from the phone with its own identity, opens its own connection to the real node
/// and forwards Pair frames between the two.
struct Relay {
    addr: SocketAddr,
    id: Identity,
    /// What the real node answered to the forwarded Pair.
    node_answers: Arc<Mutex<Vec<PairReply>>>,
    _ep: quinn::Endpoint,
}

fn start_relay(node_addr: SocketAddr) -> Relay {
    let id = Identity::generate().unwrap();
    let ep = quinn::Endpoint::server(tls::server_config(&id).unwrap(), loopback()).unwrap();
    let addr = ep.local_addr().unwrap();
    let node_answers = Arc::new(Mutex::new(Vec::new()));
    let (ep2, id2, answers) = (ep.clone(), id.clone(), node_answers.clone());
    tokio::spawn(async move {
        let up_ep = client::new_endpoint().unwrap();
        while let Some(incoming) = ep2.accept().await {
            let Ok(phone_conn) = incoming.await else { continue };
            // Second leg: the relay presents its own certificate to the node.
            let Ok((up, _)) = client::connect(&up_ep, &id2, node_addr, None, WAIT).await else { continue };
            let answers = answers.clone();
            tokio::spawn(async move {
                while let Ok((mut send, mut recv)) = phone_conn.accept_bi().await {
                    let Ok(Some(req)) = wire::read_frame::<Request>(&mut recv).await else { break };
                    let Ok(reply) = client::call::<PairReply>(&up, &req).await else { break };
                    answers.lock().unwrap().push(reply.clone());
                    let _ = wire::write_frame(&mut send, &reply).await;
                    let _ = send.finish();
                    let _ = tokio::time::timeout(WAIT, send.stopped()).await;
                }
            });
        }
    });
    Relay { addr, id, node_answers, _ep: ep }
}

#[tokio::test]
async fn man_in_the_middle_cannot_complete_pairing() {
    within(async {
        let node = FakeNode::start("K7QM-4TDX");
        let relay = start_relay(node.addr);
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();

        // The phone is honest and has the right code, but its address leads to the relay.
        let (conn, seen_fp) = client::connect(&ep, &phone, relay.addr, None, WAIT).await.unwrap();
        assert_eq!(seen_fp, relay.id.fingerprint(), "the phone is talking to the relay");
        assert_ne!(seen_fp, node.fp());
        let err = client::pair(&conn, &phone, &seen_fp, "Test phone", "K7QM-4TDX").await.expect_err("pairing through a relay must not succeed");
        let _ = err;

        // The node refused the forwarded proof, and admitted nobody.
        let answers = node.state.pair_replies.lock().unwrap().clone();
        assert_eq!(answers.len(), 1, "the relay did forward the frame");
        assert!(matches!(answers[0], PairReply::Refused(_)), "node must refuse: {:?}", answers[0]);
        assert!(matches!(relay.node_answers.lock().unwrap()[0], PairReply::Refused(_)));
        assert_eq!(node.paired_count(), 0);
        assert!(node.state.code.lock().unwrap().is_some(), "the code was not consumed");

        // The honest direct path still works afterwards.
        try_pairing(&ep, &phone, node.addr, "K7QM-4TDX").await.expect("direct pairing after the attempt");
    })
    .await;
}

#[tokio::test]
async fn a_node_that_does_not_know_the_code_cannot_win_the_phones_trust() {
    within(async {
        // A fake node that says yes to anything, with a made up proof.
        let id = Identity::generate().unwrap();
        let ep_s = quinn::Endpoint::server(tls::server_config(&id).unwrap(), loopback()).unwrap();
        let addr = ep_s.local_addr().unwrap();
        tokio::spawn(async move {
            while let Some(incoming) = ep_s.accept().await {
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                        let _ = wire::read_frame::<Request>(&mut recv).await;
                        let _ = wire::write_frame(&mut send, &PairReply::Ok { node_name: "evil".into(), proof: [0u8; 32] }).await;
                        let _ = send.finish();
                        let _ = tokio::time::timeout(WAIT, send.stopped()).await;
                    }
                });
            }
        });
        let phone = Identity::generate().unwrap();
        let ep = client::new_endpoint().unwrap();
        let err = try_pairing(&ep, &phone, addr, "K7QM-4TDX").await.expect_err("a bogus node proof must be rejected");
        assert!(err.to_string().contains("did not know the code"), "unexpected error: {err}");
    })
    .await;
}

// ---------------------------------------------------------------------------------------------
// (f) code normalisation
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn code_normalisation_lets_people_type_it_loosely() {
    assert_eq!(pair::normalize("k7qm-4tdx"), "K7QM4TDX");
    assert_eq!(pair::normalize(" K7QM 4TDX\n"), "K7QM4TDX");
    assert_eq!(pair::normalize("k7qm4tdx"), "K7QM4TDX");
    assert_eq!(pair::normalize("K7QM-4TDX"), pair::normalize("K7QM4TDX"));

    for typed in ["k7qm-4tdx", "k7qm4tdx", "K7QM4TDX", "  k7qm 4tdx  "] {
        within(async {
            let node = FakeNode::start("K7QM-4TDX");
            let phone = Identity::generate().unwrap();
            let ep = client::new_endpoint().unwrap();
            try_pairing(&ep, &phone, node.addr, typed).await.unwrap_or_else(|e| panic!("{typed:?} should pair: {e:#}"));
        })
        .await;
    }
}

#[test]
fn generated_codes_look_right_and_survive_normalisation() {
    for _ in 0..200 {
        let c = pair::new_code();
        assert_eq!(c.len(), 9, "{c}");
        assert_eq!(c.as_bytes()[4], b'-');
        let n = pair::normalize(&c);
        assert_eq!(n.len(), 8);
        // No look-alike characters in what people have to read out.
        assert!(!n.chars().any(|ch| "01OIL".contains(ch)), "{c}");
        let (e, p) = ([1u8; 32], ("a", "b"));
        assert!(pair::proofs_equal(&pair::proof(&c, pair::Side::Phone, &e, p.0, p.1), &pair::proof(&c.to_lowercase(), pair::Side::Phone, &e, p.0, p.1)));
    }
}

// ---------------------------------------------------------------------------------------------
// (g) frames
// ---------------------------------------------------------------------------------------------

/// A server that echoes every frame of type T back on the same stream.
async fn echo_pair<T: Serialize + DeserializeOwned + Send + Sync + 'static>() -> (quinn::Connection, tokio::task::JoinHandle<anyhow::Result<()>>, quinn::Endpoint, quinn::Endpoint) {
    let sid = Identity::generate().unwrap();
    let server = quinn::Endpoint::server(tls::server_config(&sid).unwrap(), loopback()).unwrap();
    let addr = server.local_addr().unwrap();
    let s2 = server.clone();
    let task = tokio::spawn(async move {
        let conn = s2.accept().await.ok_or_else(|| anyhow::anyhow!("no connection"))?.await?;
        let (mut send, mut recv) = conn.accept_bi().await?;
        while let Some(v) = wire::read_frame::<T>(&mut recv).await? {
            wire::write_frame(&mut send, &v).await?;
        }
        send.finish()?;
        let _ = tokio::time::timeout(WAIT, send.stopped()).await;
        Ok(())
    });
    let phone = Identity::generate().unwrap();
    let cep = client::new_endpoint().unwrap();
    let (conn, _) = client::connect(&cep, &phone, addr, Some(sid.fingerprint()), WAIT).await.unwrap();
    (conn, task, cep, server)
}

/// Sends all values, reads the same number back and checks they encode identically.
async fn roundtrip<T: Serialize + DeserializeOwned + Debug + Send + Sync + 'static>(vals: Vec<T>) {
    let (conn, task, _cep, _server) = echo_pair::<T>().await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    for v in &vals {
        wire::write_frame(&mut send, v).await.unwrap();
    }
    send.finish().unwrap();
    for v in &vals {
        let back: T = wire::read_frame(&mut recv).await.unwrap().expect("a frame");
        assert_eq!(postcard::to_stdvec(v).unwrap(), postcard::to_stdvec(&back).unwrap(), "{v:?} changed on the way");
    }
    assert!(wire::read_frame::<T>(&mut recv).await.unwrap().is_none(), "clean end of stream is None");
    task.await.unwrap().unwrap();
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        messages: vec![
            ChatMsg { role: Role::System, content: "be brief".into() },
            ChatMsg { role: Role::User, content: "Grüße, 世界 \u{1F600}".into() },
            ChatMsg { role: Role::Assistant, content: String::new() },
        ],
        max_tokens: 512,
        temperature: 0.7,
        top_p: 0.95,
        top_k: -1,
        priority: Priority::Interactive,
    }
}

#[tokio::test]
async fn request_frames_round_trip() {
    within(roundtrip(vec![
        Request::Status,
        Request::Pair { name: "Ashwani's phone".into(), proof: [0xAB; 32] },
        Request::Chat(chat_request()),
        Request::Embed(EmbedRequest { texts: vec!["title: none | text: a".into(), String::new(), "b".repeat(100_000)], priority: Priority::Bulk }),
    ]))
    .await;
}

#[tokio::test]
async fn chat_event_stream_round_trips_in_order() {
    let mut evs = vec![ChatEvent::Queued { ahead: 3 }];
    evs.extend((0..500).map(|i| ChatEvent::Token(format!("tok{i} "))));
    evs.push(ChatEvent::Done { prompt_tokens: 1234, generated_tokens: 500, elapsed_ms: 1234 });
    evs.push(ChatEvent::Error("boom".into()));
    within(roundtrip(evs)).await;
}

#[tokio::test]
async fn pair_and_status_replies_round_trip() {
    within(roundtrip(vec![PairReply::Ok { node_name: "n".into(), proof: [1; 32] }, PairReply::Refused("no".into())])).await;
    within(roundtrip(vec![StatusReply {
        proto: PROTO_VERSION,
        node_name: "node".into(),
        version: "0.1.0".into(),
        caps: Capabilities { chat: Some("gemma".into()), embed: Some("emb".into()), embed_dim: 768, chat_tok_s: 41.5, embed_per_s: 250.0, transcribe: Some("whisper".into()), transcribe_x: 30.0, window_max_s: 120 },
        running: 1,
        waiting: 2,
        clients: 3,
        served: 42,
        uptime_s: 3600,
        pending: vec![PendingPhone { id: "0123456789abcdef".into(), name: "Pixel".into(), waited_s: 7 }],
        paused: true,
    }, StatusReply::default()]))
    .await;
}

#[tokio::test]
async fn large_embed_reply_round_trips_exactly() {
    let vectors: Vec<Vec<f32>> = (0..64).map(|i| (0..768).map(|j| ((i * 768 + j) as f32).sin() * 0.03125 - 0.0001 * j as f32).collect()).collect();
    let reply = EmbedReply::Vectors(vectors.clone());
    assert!(postcard::to_stdvec(&reply).unwrap().len() > 64 * 768 * 4);

    let (conn, task, _cep, _server) = echo_pair::<EmbedReply>().await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    within(wire::write_frame(&mut send, &reply)).await.unwrap();
    within(wire::write_frame(&mut send, &EmbedReply::Queued { ahead: 2 })).await.unwrap();
    send.finish().unwrap();
    let back: EmbedReply = within(wire::read_frame(&mut recv)).await.unwrap().unwrap();
    let EmbedReply::Vectors(got) = back else { panic!("wrong variant") };
    assert_eq!(got.len(), 64);
    for (a, b) in vectors.iter().zip(&got) {
        assert_eq!(a.len(), 768);
        assert_eq!(a.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), b.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), "floats must be bit exact");
    }
    assert!(matches!(within(wire::read_frame::<EmbedReply>(&mut recv)).await.unwrap().unwrap(), EmbedReply::Queued { ahead: 2 }));
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn embed_reply_error_and_empty_round_trip() {
    within(roundtrip(vec![EmbedReply::Error("no model".into()), EmbedReply::Vectors(vec![]), EmbedReply::Vectors(vec![vec![]])])).await;
}

#[tokio::test]
async fn an_absurd_frame_length_is_rejected_without_allocating() {
    within(async {
        let (conn, task, _cep, _server) = echo_pair::<Request>().await;
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        send.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
        send.finish().unwrap();
        let res = task.await.unwrap();
        let err = res.expect_err("server must refuse the frame");
        assert!(err.to_string().contains("too large"), "{err}");
    })
    .await;
}

#[tokio::test]
async fn a_truncated_frame_is_an_error_but_an_empty_stream_is_not() {
    within(async {
        // Length says 100 bytes, only 3 arrive.
        let (conn, task, _cep, _server) = echo_pair::<Request>().await;
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        send.write_all(&100u32.to_le_bytes()).await.unwrap();
        send.write_all(&[1, 2, 3]).await.unwrap();
        send.finish().unwrap();
        assert!(task.await.unwrap().is_err());

        // A stream that ends before any frame is a clean None.
        let (conn, task, _cep, _server) = echo_pair::<Request>().await;
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        send.write_all(&[]).await.unwrap();
        send.finish().unwrap();
        task.await.unwrap().unwrap();
        drop(conn);
    })
    .await;
}

#[tokio::test]
async fn garbage_body_is_an_error_not_a_panic() {
    within(async {
        let (conn, task, _cep, _server) = echo_pair::<Request>().await;
        let (mut send, _recv) = conn.open_bi().await.unwrap();
        send.write_all(&4u32.to_le_bytes()).await.unwrap();
        send.write_all(&[0xFF, 0xFF, 0xFF, 0xFF]).await.unwrap();
        send.finish().unwrap();
        assert!(task.await.unwrap().is_err());
    })
    .await;
}

#[test]
fn transcribe_messages_round_trip() {
    use marginalia_proto::{AudioFrame, TranscribeEvent, TranscribeRequest};
    let req = Request::Transcribe(TranscribeRequest { language: Some("hi".into()), total_ms: 960_000, priority: Priority::Background, prompt: None });
    let bytes = postcard::to_stdvec(&req).unwrap();
    assert!(matches!(postcard::from_bytes::<Request>(&bytes).unwrap(), Request::Transcribe(r) if r.total_ms == 960_000));
    let pcm = AudioFrame::Pcm(vec![1, 2, 3, 4]);
    assert!(matches!(postcard::from_bytes::<AudioFrame>(&postcard::to_stdvec(&pcm).unwrap()).unwrap(), AudioFrame::Pcm(b) if b == vec![1, 2, 3, 4]));
    for ev in [TranscribeEvent::Working, TranscribeEvent::Progress { done_ms: 1, total_ms: 2 }, TranscribeEvent::Segment { t0_ms: 1, t1_ms: 2, text: "hello".into() }, TranscribeEvent::Done { language: "en".into(), segments: 3, elapsed_ms: 4 }] {
        let b = postcard::to_stdvec(&ev).unwrap();
        let _ = postcard::from_bytes::<TranscribeEvent>(&b).unwrap();
    }
}
