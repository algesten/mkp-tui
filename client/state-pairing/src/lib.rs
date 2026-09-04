//! External-fact source: the current pairing session's state, if one
//! exists. One pairing in flight at a time.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PairingPhase {
    #[default]
    Idle,
    /// Connected via TOFU; PairRequest sent; waiting for server's
    /// signed cert.
    AwaitingResponse,
    /// Server returned a signed cert; verification code computed.
    /// UI shows `code`; waiting for user to confirm or reject.
    AwaitingConfirmation,
    /// User confirmed; `PairConfirm` is shipped; driver will then
    /// close the pair link. After `Idle` reappears the credentials
    /// driver has (hopefully) already persisted the entry.
    Confirming,
    /// Failed with message.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pairing {
    pub phase: PairingPhase,
    /// Six-digit verification code derived from TLS EKM + signed
    /// client cert. Set during `AwaitingConfirmation`.
    pub code: Option<std::sync::Arc<str>>,
    /// The fingerprint that will become the credentials key once the
    /// user confirms.
    pub server_fingerprint: Option<std::sync::Arc<str>>,
    /// Server cert PEM captured during the TOFU handshake. Held so
    /// the confirm step has everything it needs to persist creds.
    pub server_cert_pem: Option<String>,
    /// Signed client cert returned by the server.
    pub client_cert_pem: Option<String>,
    /// Our client key (generated at pair start).
    pub client_key_pem: Option<String>,
    /// Error message if `phase == Failed`.
    pub error: Option<String>,
}
