//! Identity, pinning and the QUIC settings both sides share.
//!
//! Every device makes itself a self-signed certificate once. Nobody checks a name or a certificate
//! authority: a node knows its phones by the fingerprint of their certificate, and a phone knows its
//! nodes the same way. The fingerprint is what pairing hands over.

use anyhow::{anyhow, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub cert_der: Vec<u8>,
    pub key_der: Vec<u8>,
}

impl Identity {
    pub fn generate() -> Result<Identity> {
        let key = rcgen::KeyPair::generate()?;
        let params = rcgen::CertificateParams::new(vec![crate::SERVER_NAME.to_string()])?;
        let cert = params.self_signed(&key)?;
        Ok(Identity { cert_der: cert.der().to_vec(), key_der: key.serialize_der() })
    }

    pub fn fingerprint(&self) -> String {
        fingerprint(&self.cert_der)
    }

    fn parts(&self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        (
            vec![CertificateDer::from(self.cert_der.clone())],
            PrivateKeyDer::from(PrivatePkcs8KeyDer::from(self.key_der.clone())),
        )
    }
}

/// 64 hex characters.
pub fn fingerprint(cert_der: &[u8]) -> String {
    blake3::hash(cert_der).to_hex().to_string()
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(ring::default_provider())
}

fn transport() -> Arc<quinn::TransportConfig> {
    let mut t = quinn::TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_secs(10)));
    t.max_idle_timeout(Some(Duration::from_secs(30).try_into().expect("idle timeout")));
    t.max_concurrent_bidi_streams(64u32.into());
    t.max_concurrent_uni_streams(0u32.into());
    Arc::new(t)
}

/// Accepts the node's certificate if its fingerprint is the pinned one. With no pin (pairing) it
/// accepts any and notes the fingerprint it saw: pairing then proves the node knows the code.
#[derive(Debug)]
struct PinVerifier {
    pin: Option<String>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fp = fingerprint(end_entity.as_ref());
        *self.seen.lock().unwrap() = Some(fp.clone());
        match &self.pin {
            Some(p) if *p != fp => Err(rustls::Error::General("this is not the paired computer".into())),
            _ => Ok(rustls::client::danger::ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Takes any client certificate that proves it owns its key. Who may do what is decided afterwards
/// from the fingerprint, because an unpaired phone has to get as far as asking to pair.
#[derive(Debug)]
struct AnyClient {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, _e: &CertificateDer<'_>, _i: &[CertificateDer<'_>], _now: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

pub fn server_config(id: &Identity) -> Result<quinn::ServerConfig> {
    let p = provider();
    let (certs, key) = id.parts();
    let mut tls = rustls::ServerConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyClient { provider: p }))
        .with_single_cert(certs, key)?;
    tls.alpn_protocols = vec![crate::ALPN.to_vec()];
    let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// `pin` is the node's fingerprint, or None while pairing. `seen` receives the fingerprint of
/// whatever answered.
pub fn client_config(id: &Identity, pin: Option<String>, seen: Arc<Mutex<Option<String>>>) -> Result<quinn::ClientConfig> {
    let p = provider();
    let (certs, key) = id.parts();
    let mut tls = rustls::ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { pin, seen, provider: p }))
        .with_client_auth_cert(certs, key)?;
    tls.alpn_protocols = vec![crate::ALPN.to_vec()];
    let mut cfg = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).map_err(|e| anyhow!("{e}"))?));
    cfg.transport_config(transport());
    Ok(cfg)
}

/// The fingerprint of the certificate the other side presented.
pub fn peer_fingerprint(conn: &quinn::Connection) -> Option<String> {
    let any = conn.peer_identity()?;
    let certs = any.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    certs.first().map(|c| fingerprint(c.as_ref()))
}
