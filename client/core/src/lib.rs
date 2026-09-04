//! Shared primitives for the drv-based client: cross-driver wake
//! signalling plus whatever small types end up being imported from
//! more than one state / driver crate.

mod notify;

pub use notify::Notifier;
