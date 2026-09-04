//! rustls configuration helpers — TOFU verifier for pairing, pinned
//! verifier for client sessions, PEM <-> DER conversions.
//!
//! Lifted from the legacy `client/src/tls.rs`; stripped of tokio,
//! lives inside `native-std` because it's tied to a specific rustls
//! version.

use std::sync::{Arc, Mutex, Once};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use sha2::{Digest, Sha256};

pub const ALPN_PAIR: &[u8] = b"mkp-pair";
pub const ALPN_CLIENT: &[u8] = b"mkp-client";

pub fn cert_fingerprint(cert_der: &[u8]) -> String {
    hex::encode(Sha256::digest(cert_der))
}

pub fn export_keying_material(conn: &rustls::ClientConnection) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    conn.export_keying_material(&mut out, b"mkplay-pairing-v1", Some(b""))
        .expect("export keying material");
    out
}

// Install aws-lc-rs as the default provider exactly once. Multiple
// `spawn()` calls must not double-install.
static INSTALL_PROVIDER: Once = Once::new();
pub fn ensure_crypto_provider() {
    INSTALL_PROVIDER.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn restricted_provider() -> Arc<rustls::crypto::CryptoProvider> {
    use rustls::crypto::aws_lc_rs;
    Arc::new(rustls::crypto::CryptoProvider {
        cipher_suites: vec![aws_lc_rs::cipher_suite::TLS13_AES_256_GCM_SHA384],
        kx_groups: vec![aws_lc_rs::kx_group::X25519],
        ..aws_lc_rs::default_provider()
    })
}

fn verify_tls13(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> Result<HandshakeSignatureValid, Error> {
    rustls::crypto::verify_tls13_signature(
        message,
        cert,
        dss,
        &rustls::crypto::CryptoProvider::get_default()
            .expect("rustls crypto provider installed")
            .signature_verification_algorithms,
    )
}

fn client_builder() -> rustls::ConfigBuilder<ClientConfig, rustls::WantsVerifier> {
    ClientConfig::builder_with_provider(restricted_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3 config")
}

#[derive(Debug)]
pub struct TofuVerifier {
    captured: Arc<Mutex<Option<Vec<u8>>>>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if let Ok(mut g) = self.captured.lock() {
            *g = Some(end_entity.to_vec());
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 not supported".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

#[derive(Debug)]
pub struct PinnedVerifier {
    expected_cert_der: Vec<u8>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if end_entity.as_ref() == self.expected_cert_der.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            let expected = cert_fingerprint(&self.expected_cert_der);
            let actual = cert_fingerprint(end_entity.as_ref());
            Err(Error::General(format!(
                "Server cert mismatch: expected {expected}, got {actual}"
            )))
        }
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 not supported".into()))
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ECDSA_NISTP256_SHA256]
    }
}

/// Result of a TOFU probe / pairing config build: a ready-to-use
/// `ClientConfig` plus the slot the verifier writes the captured
/// server-cert DER into when the handshake lands.
pub type TofuConfig = (Arc<ClientConfig>, Arc<Mutex<Option<Vec<u8>>>>);

/// Probe config: TOFU server verification, no client cert, ALPN
/// `mkp-client`. Used to fetch the server cert fingerprint before
/// picking a stored credential to authenticate with.
pub fn probe_config() -> TofuConfig {
    let captured = Arc::new(Mutex::new(None));
    let verifier = TofuVerifier {
        captured: captured.clone(),
    };
    let mut cfg = client_builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![ALPN_CLIENT.to_vec()];
    (Arc::new(cfg), captured)
}

pub fn pairing_config() -> TofuConfig {
    let captured = Arc::new(Mutex::new(None));
    let verifier = TofuVerifier {
        captured: captured.clone(),
    };
    let mut cfg = client_builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![ALPN_PAIR.to_vec()];
    (Arc::new(cfg), captured)
}

pub fn authenticated_config(
    server_cert_pem: &str,
    client_cert_pem: &str,
    client_key_pem: &str,
) -> Result<Arc<ClientConfig>, String> {
    let server_cert_der = cert_pem_to_der(server_cert_pem);
    let verifier = PinnedVerifier {
        expected_cert_der: server_cert_der,
    };
    let cert = cert_pem_to_der_typed(client_cert_pem);
    let key = key_pem_to_der(client_key_pem);
    let mut cfg = client_builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_auth_cert(vec![cert], PrivateKeyDer::Pkcs8(key))
        .map_err(|e| format!("with_client_auth_cert: {e}"))?;
    cfg.alpn_protocols = vec![ALPN_CLIENT.to_vec()];
    Ok(Arc::new(cfg))
}

pub fn cert_pem_to_der(pem: &str) -> Vec<u8> {
    cert_pem_to_der_typed(pem).to_vec()
}

fn cert_pem_to_der_typed(pem: &str) -> CertificateDer<'static> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse cert PEM")
        .into_iter()
        .next()
        .expect("at least one cert in PEM")
}

fn key_pem_to_der(pem: &str) -> PrivatePkcs8KeyDer<'static> {
    let mut reader = std::io::BufReader::new(pem.as_bytes());
    rustls_pemfile::pkcs8_private_keys(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("parse key PEM")
        .into_iter()
        .next()
        .expect("at least one key in PEM")
}

pub fn der_to_cert_pem(der: &[u8]) -> String {
    let b64 = base64_encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in b64.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let combined = (b0 << 16) | (b1 << 8) | b2;
        result.push(CHARS[((combined >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((combined >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((combined >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(combined & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
