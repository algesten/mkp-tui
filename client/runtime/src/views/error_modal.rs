//! View model for the `Screen::ErrorModal` overlay.
//!
//! Trivial mirror — the only "computation" is preserving the
//! message string so the renderer is a pure painter consuming a
//! `Clone + PartialEq` model. The memo cache lets the diffed
//! redraw path short-circuit when nothing has changed.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ErrorModalModel {
    pub message: std::sync::Arc<str>,
}

#[derive(drv::Input)]
pub struct ErrorModalInput<'a> {
    pub message: &'a std::sync::Arc<str>,
}

#[drv::memo(single)]
pub fn error_modal_model<'a>(input: ErrorModalInput<'a>) -> ErrorModalModel {
    ErrorModalModel {
        message: input.message.clone(),
    }
}
