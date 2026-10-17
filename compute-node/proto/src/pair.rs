//! Pairing: how a phone and a node come to trust each other without a cloud or an account.
//!
//! The node shows a short one-time code on its own screen. The phone connects, takes whatever
//! certificate it is shown, and sends a proof that it knows the code. The proof is bound to the
//! secrets of that very TLS session and to both certificates, so someone sitting in the middle
//! cannot pass it along: their two sessions would not match. The node answers with its own proof,
//! and only then does the phone pin the node's certificate. The code itself is never sent.

use rand::Rng;

const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CONTEXT: &str = "marginalia pairing v1";

/// Eight characters like `K7QM-4TDX`.
pub fn new_code() -> String {
    let mut rng = rand::rng();
    let c: String = (0..8).map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char).collect();
    format!("{}-{}", &c[..4], &c[4..])
}

/// What people type: any case, with or without the dash or spaces.
pub fn normalize(code: &str) -> String {
    code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Phone,
    Node,
    /// A compute node that has just paired as a phone proves it knows the code again to be taken as a peer.
    Peer,
}

/// The code is short, so it is stretched with scrypt (16 MiB, tens of milliseconds): someone who
/// recorded a proof cannot cheaply try every possible code against it offline.
fn code_key(code: &str) -> [u8; 32] {
    let params = scrypt::Params::new(14, 8, 1, 32).expect("scrypt parameters");
    let mut key = [0u8; 32];
    scrypt::scrypt(normalize(code).as_bytes(), CONTEXT.as_bytes(), &params, &mut key).expect("scrypt output length");
    key
}

pub fn proof(code: &str, side: Side, exporter: &[u8; 32], phone_fp: &str, node_fp: &str) -> [u8; 32] {
    let key = code_key(code);
    let mut h = blake3::Hasher::new_keyed(&key);
    h.update(match side {
        Side::Phone => b"phone",
        Side::Node => b"node-",
        Side::Peer => b"peer-",
    });
    h.update(exporter);
    h.update(phone_fp.as_bytes());
    h.update(node_fp.as_bytes());
    *h.finalize().as_bytes()
}

pub fn proofs_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    blake3::Hash::from_bytes(*a) == blake3::Hash::from_bytes(*b)
}

/// Both sides call this on their end of the same connection and get the same bytes.
pub fn exporter(conn: &quinn::Connection) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    conn.export_keying_material(&mut out, b"marginalia pairing", b"v1").ok()?;
    Some(out)
}
