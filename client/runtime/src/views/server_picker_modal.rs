//! View model for the server-picker modal opened from pressing
//! Enter on the connected-server row in the left pane.
//!
//! Legacy parity (mkp2 `nav/server_picker.rs`): a small centred
//! list of every discovered server, with the currently-connected
//! one tagged `✓` and the cursor row highlighted. Selecting the
//! already-connected entry is a no-op (modal closes); a different
//! entry triggers the disconnect + reconnect handled in dispatch.

use std::sync::Arc;

use imbl::Vector;
use mkpclient_driver_discovery_core::ServerAd;
use mkpclient_state_discovery::Discovery;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServerPickerRow {
    pub label: Arc<str>,
    pub is_current: bool,
    pub is_cursor: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServerPickerModalModel {
    pub rows: Vector<ServerPickerRow>,
    pub selected: usize,
}

#[derive(drv::Input)]
pub struct ServerPickerModalInput<'a> {
    pub servers: &'a Vector<ServerAd>,
    pub current_backend: Option<&'a str>,
    pub selected: usize,
}

impl<'a> ServerPickerModalInput<'a> {
    pub fn new(
        discovery: &'a Discovery,
        current_backend: Option<&'a str>,
        selected: usize,
    ) -> Self {
        Self {
            servers: &discovery.servers,
            current_backend,
            selected,
        }
    }
}

#[drv::memo(single)]
pub fn server_picker_modal_model<'a>(input: ServerPickerModalInput<'a>) -> ServerPickerModalModel {
    let rows: Vector<ServerPickerRow> = input
        .servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let display = s.host.strip_suffix(".local").unwrap_or(s.host.as_str());
            // Mark the currently-connected server with the legacy
            // checkmark so the user can tell at a glance which row
            // they're on; everything else aligns by two leading
            // spaces so the column reads cleanly.
            let is_current = input.current_backend.is_some_and(|b| s.name.as_str() == b);
            let label: Arc<str> = if is_current {
                format!("\u{2713} {display}").into()
            } else {
                format!("  {display}").into()
            };
            ServerPickerRow {
                label,
                is_current,
                is_cursor: i == input.selected,
            }
        })
        .collect();
    ServerPickerModalModel {
        rows,
        selected: input.selected,
    }
}
