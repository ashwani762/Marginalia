//! What the phone app and a home compute node say to each other.
//!
//! One QUIC connection per pair of devices, one stream per request. Both sides prove who they are
//! with a self-signed certificate that the other side has pinned when they were paired (see [`pair`]).
//! Nothing here stores job data: a request and its answer only live as long as their stream.

pub mod client;
pub mod msg;
pub mod pair;
pub mod tls;
pub mod wire;

pub use msg::*;

/// Bumped when the messages change in a way old peers cannot read. It is part of the TLS
/// handshake, so a phone and a node on different versions refuse each other cleanly.
pub const PROTO_VERSION: u32 = 1;
pub const ALPN: &[u8] = b"marginalia/1";
/// The name both sides put in the certificate and the handshake. Trust comes from the pinned
/// fingerprint, not from this name.
pub const SERVER_NAME: &str = "marginalia-node";
/// DNS-SD service type the node advertises on the local network.
pub const SERVICE_TYPE: &str = "_marginalia._udp.local.";
pub const DEFAULT_PORT: u16 = 47615;
