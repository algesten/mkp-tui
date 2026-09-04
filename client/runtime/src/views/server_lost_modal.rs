//! View model for the `Screen::ServerLostModal` overlay.
//!
//! Holds the lost-server name. The reconnect spinner glyph is
//! per-frame and lives outside the memo (the renderer reads
//! `app.tick`); folding it in would invalidate the memo every
//! frame and defeat the diff-skip path.

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServerLostModalModel {
    pub server: std::sync::Arc<str>,
}

#[derive(drv::Input)]
pub struct ServerLostModalInput<'a> {
    pub server: &'a std::sync::Arc<str>,
}

#[drv::memo(single)]
pub fn server_lost_modal_model<'a>(input: ServerLostModalInput<'a>) -> ServerLostModalModel {
    ServerLostModalModel {
        server: input.server.clone(),
    }
}
