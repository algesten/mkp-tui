//! View model for the pairing-confirmation modal.
//!
//! Renders the short verification code + server fingerprint while
//! `PairingPhase::AwaitingConfirmation`. The memo projects the
//! pairing source so a frame change in unrelated sources doesn't
//! re-format the modal body.

use mkpclient_state_pairing::Pairing;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PairingModalModel {
    pub code: std::sync::Arc<str>,
    pub fingerprint: std::sync::Arc<str>,
}

#[derive(drv::Input)]
pub struct PairingModalInput<'a> {
    pub code: Option<&'a std::sync::Arc<str>>,
    pub server_fingerprint: Option<&'a std::sync::Arc<str>>,
}

impl<'a> PairingModalInput<'a> {
    pub fn new(p: &'a Pairing) -> Self {
        Self {
            code: p.code.as_ref(),
            server_fingerprint: p.server_fingerprint.as_ref(),
        }
    }
}

#[drv::memo(single)]
pub fn pairing_modal_model<'a>(input: PairingModalInput<'a>) -> PairingModalModel {
    PairingModalModel {
        code: input
            .code
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::from("—")),
        fingerprint: input
            .server_fingerprint
            .cloned()
            .unwrap_or_else(|| std::sync::Arc::from("—")),
    }
}
