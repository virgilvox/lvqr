//! MoQ relay over `moq-lite` with fan-out engine for LVQR.
//!
//! `RelayServer` owns the QUIC / WebTransport listener, accepts
//! `moq-lite` sessions, and dispatches publisher / subscriber roles
//! against the shared [`lvqr_auth::SharedAuth`] provider. Fan-out is
//! zero-copy via `moq-lite::OriginProducer`; per-fragment hot-path
//! state stays node-local (load-bearing decision #5 in
//! `tracking/ROADMAP.md`).
//!
//! ## Transport backends
//!
//! Behind the default `quinn-transport` feature, the listener uses
//! `web-transport-quinn` + `quinn` + `rustls` + `moq-native` to
//! terminate WebTransport over QUIC. Without the feature, the crate
//! still compiles but [`RelayServer::run`] returns a
//! [`RelayError::Transport`] error -- this is the shape integrators
//! use when they want to embed `lvqr-relay`'s control surface in a
//! custom transport pipeline.
//!
//! ## Public surface
//!
//! [`RelayConfig`] / [`RelayServer`] for the listener;
//! [`RelayProtocol`] for the published-broadcast accept loop on the
//! `OriginProducer` side; [`RelayError`] for the unified error type.

pub mod error;
pub mod protocol;
pub mod server;

pub use error::RelayError;
pub use protocol::RelayProtocol;
pub use server::{RelayConfig, RelayServer};
