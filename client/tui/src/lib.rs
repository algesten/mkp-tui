//! Library face of the TUI crate. The binary entry point lives in
//! `main.rs`; this re-exports the modules it builds so integration
//! tests in `tests/` can drive the TUI's state machine and render
//! against a `TestBackend` without spinning the full binary.

pub mod app;
pub mod cli;
pub mod history_offsets;
pub mod input;
pub mod render;
