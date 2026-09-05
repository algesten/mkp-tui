//! Lifecycle: the `desired_*` / `*_action` memo pairs that used to
//! live inline in the TUI's `auto_restore` helper.
//!
//! Per `EXAMPLE-ARCH.md` §5 ("Queries: desired state, not transitions")
//! and §6 ("Actions as diffs"): each flow has a `desired_X()` memo
//! that says what should be true, an `X_action()` memo that diffs
//! against the live driver source, and an `apply_X()` trampoline that
//! writes intent synchronously before firing any async work.
//!
//! Each sub-module owns one flow end-to-end (memo pair + projections
//! + trampoline), so adding a flow doesn't churn this file.

pub mod backend;
pub mod clipboard_toast;
pub mod connect;
pub mod cursor_clamp;
pub mod cursor_snap;
pub mod last_add_persist;
pub mod link_ack;
pub mod lost_modal;
pub mod pending_add;
pub mod playlists_refetch;
pub mod restore;
pub mod search_history_push;
pub mod search_reopen;
pub mod server_errors;
pub mod view_persist;
