use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute a 6-digit verification code from the TLS exported keying material
/// and the client certificate DER bytes.
///
/// HMAC-SHA256(key=ekm, message=client_cert_der), take first 4 bytes as u32
/// mod 1_000_000, zero-padded to 6 digits.
pub fn compute_pairing_code(ekm: &[u8], client_cert_der: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(ekm).expect("HMAC accepts any key size");
    mac.update(client_cert_der);
    let result = mac.finalize().into_bytes();

    let value = u32::from_be_bytes([result[0], result[1], result[2], result[3]]);
    let code = value % 1_000_000;
    format!("{:06}", code)
}
