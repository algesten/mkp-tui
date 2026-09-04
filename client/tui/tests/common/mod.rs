//! Test scaffolding: a TLS mock server that speaks the mkp wire
//! protocol, plus a builder that wires it into a `Runtime` for
//! integration tests.

pub mod certs;
pub mod harness;
pub mod mock_server;
