//! Self-signed cert fixture for the mock server.
//!
//! Generates an ephemeral CA, server leaf, and client leaf at
//! call-time. Returns the PEMs the runtime expects + the SHA-256
//! fingerprint of the server cert (hex, lowercase).

use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct TestCerts {
    pub server_cert_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
    pub fingerprint: String,
    pub server_cert_der: Vec<u8>,
    pub server_key_der: Vec<u8>,
}

pub fn generate() -> TestCerts {
    use rcgen::{CertificateParams, IsCa, KeyPair, PKCS_ECDSA_P256_SHA256};

    // CA — just used as a signing parent so the leaf certs aren't
    // visibly self-signed. The client pins the leaf cert itself,
    // so the chain doesn't matter for verification.
    let mut ca_params = CertificateParams::default();
    ca_params.distinguished_name = rcgen::DistinguishedName::new();
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "mkp-test-ca");
    ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("ca keypair");
    let ca_cert = ca_params.self_signed(&ca_kp).expect("ca self-sign");

    // Server leaf — sign with the CA so the client's pinned
    // verifier accepts the leaf cert as-is.
    let mut server_params = CertificateParams::new(vec!["mkp-mock".to_string()]).expect("params");
    server_params.distinguished_name = rcgen::DistinguishedName::new();
    server_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "mkp-mock");
    let server_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("server keypair");
    let server_cert = server_params
        .signed_by(&server_kp, &ca_cert, &ca_kp)
        .expect("server sign");
    let server_cert_pem = server_cert.pem();
    let server_cert_der = server_cert.der().to_vec();
    let server_key_der = server_kp.serialize_der();

    // Client leaf.
    let mut client_params = CertificateParams::new(vec![]).expect("params");
    client_params.distinguished_name = rcgen::DistinguishedName::new();
    client_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "mkp-test-client");
    let client_kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("client keypair");
    let client_cert = client_params
        .signed_by(&client_kp, &ca_cert, &ca_kp)
        .expect("client sign");
    let client_cert_pem = client_cert.pem();
    let client_key_pem = client_kp.serialize_pem();

    let fingerprint = hex::encode_lower(Sha256::digest(&server_cert_der));

    TestCerts {
        server_cert_pem,
        client_cert_pem,
        client_key_pem,
        fingerprint,
        server_cert_der,
        server_key_der,
    }
}

mod hex {
    pub fn encode_lower(bytes: impl AsRef<[u8]>) -> String {
        let bytes = bytes.as_ref();
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }
}
