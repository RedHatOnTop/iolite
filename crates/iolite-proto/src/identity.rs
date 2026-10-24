//! Self-signed certificates, fingerprints and pairing codes.
//!
//! Neither side trusts a certificate authority. The host keeps a list of client certificate
//! fingerprints it has paired with; a client pins the host fingerprint it saw when it paired.
//! Pairing shows a six-digit code derived from both fingerprints on both screens, so a relay in
//! the middle, which must present its own certificates, produces two different codes.

use std::any::Any;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};

use crate::{ALPN, SERVER_NAME};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert_der: &[u8]) -> Self {
        let digest = ring::digest::digest(&ring::digest::SHA256, cert_der);
        Self(digest.as_ref().try_into().expect("SHA-256 is 32 bytes"))
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
        }
        Some(Self(out))
    }

    /// The peer certificate of an established QUIC connection, from `Connection::peer_identity`.
    pub fn of_peer(identity: Option<Box<dyn Any>>) -> Option<Self> {
        let certs = identity?.downcast::<Vec<CertificateDer<'static>>>().ok()?;
        certs.first().map(|c| Self::of(c))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({}…)", &self.to_hex()[..12])
    }
}

/// Six decimal digits both ends compute independently from the two fingerprints.
pub fn pairing_code(host: &Fingerprint, client: &Fingerprint) -> String {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(b"iolite pairing v1");
    ctx.update(&host.0);
    ctx.update(&client.0);
    let digest = ctx.finish();
    let n = u32::from_be_bytes(digest.as_ref()[..4].try_into().unwrap()) % 1_000_000;
    format!("{n:06}")
}

pub struct Identity {
    pub cert: CertificateDer<'static>,
    pub key: PrivatePkcs8KeyDer<'static>,
}

impl Identity {
    pub fn generate(common_name: &str) -> Result<Self, rcgen::Error> {
        let key = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(vec![SERVER_NAME.to_string()])?;
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, common_name);
        let cert = params.self_signed(&key)?;
        Ok(Self {
            cert: cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()),
        })
    }

    /// Loads `<stem>-cert.der` and `<stem>-key.der` from `dir`, creating them on first use.
    pub fn load_or_generate(dir: &Path, stem: &str, common_name: &str) -> io::Result<Self> {
        let cert_path = dir.join(format!("{stem}-cert.der"));
        let key_path = dir.join(format!("{stem}-key.der"));
        if cert_path.exists() && key_path.exists() {
            return Ok(Self {
                cert: CertificateDer::from(fs::read(&cert_path)?),
                key: PrivatePkcs8KeyDer::from(fs::read(&key_path)?),
            });
        }
        fs::create_dir_all(dir)?;
        let id = Self::generate(common_name).map_err(io::Error::other)?;
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)?
            .write_all(id.key.secret_pkcs8_der())?;
        fs::write(&cert_path, id.cert.as_ref())?;
        Ok(id)
    }

    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(&self.cert)
    }

    fn key_der(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(self.key.clone_key())
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Host side: requires a client certificate but leaves the decision to the allow list.
pub fn server_config(id: &Identity) -> Result<rustls::ServerConfig, rustls::Error> {
    let provider = provider();
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(AnyClientCert { provider }))
        .with_single_cert(vec![id.cert.clone()], id.key_der())?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

/// Client side: accepts only `pinned_host` once paired, any host before that.
pub fn client_config(
    id: &Identity,
    pinned_host: Option<Fingerprint>,
) -> Result<rustls::ClientConfig, rustls::Error> {
    let provider = provider();
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedHost {
            pin: pinned_host,
            provider,
        }))
        .with_client_auth_cert(vec![id.cert.clone()], id.key_der())?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

#[derive(Debug)]
struct AnyClientCert {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for AnyClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[derive(Debug)]
struct PinnedHost {
    pin: Option<Fingerprint>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for PinnedHost {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            Some(pin) if Fingerprint::of(end_entity) != pin => Err(
                rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ),
            ),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_hex_round_trips() {
        let fp = Fingerprint::of(b"certificate");
        assert_eq!(Fingerprint::from_hex(&fp.to_hex()), Some(fp));
        assert_eq!(Fingerprint::from_hex("zz"), None);
    }

    #[test]
    fn pairing_code_depends_on_both_sides() {
        let host = Fingerprint::of(b"host");
        let client = Fingerprint::of(b"client");
        let relay = Fingerprint::of(b"relay");
        let code = pairing_code(&host, &client);
        assert_eq!(code.len(), 6);
        assert_eq!(code, pairing_code(&host, &client));
        assert_ne!(code, pairing_code(&relay, &client));
        assert_ne!(code, pairing_code(&host, &relay));
    }

    #[test]
    fn configs_build_from_generated_identity() {
        let id = Identity::generate("test").unwrap();
        server_config(&id).unwrap();
        client_config(&id, Some(id.fingerprint())).unwrap();
    }
}
