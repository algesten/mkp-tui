//! Step 4 of the lifecycle: surface a server-side `ServerMsg::Error`
//! response as an `ErrorModal`.
//!
//! Spec exception (per `EXAMPLE-ARCH.md` guideline on `nearest_deadline`
//! and the equivalent rule in this codebase): the responses queue
//! changes every time a response lands, so memoising a scan over it
//! buys nothing. The trampoline is a free function that runs inline
//! with the rest of the execute phase.

use mkproto::ServerMsg;

use mkpclient_state_ui_screen::Screen;

use crate::sources::Sources;

/// Scan `sources.responses` for an error reply and, if exactly one is
/// queued *and* the user is still on the now-playing screen, take the
/// response and open `Screen::ErrorModal` with its message.
///
/// Skipped entirely when the screen isn't `NowPlaying` so a more
/// important modal (pairing, server-lost) doesn't get clobbered.
pub fn apply_server_errors(sources: &mut Sources) {
    if !matches!(sources.screen, Screen::NowPlaying) {
        return;
    }
    let Some((seq, message)) =
        sources
            .responses
            .by_seq
            .iter()
            .find_map(|(seq, msg)| match &**msg {
                ServerMsg::Error { message } => Some((*seq, message.clone())),
                _ => None,
            })
    else {
        return;
    };
    sources.responses.take(seq);
    sources.screen = Screen::ErrorModal {
        message: std::sync::Arc::from(message),
    };
}
