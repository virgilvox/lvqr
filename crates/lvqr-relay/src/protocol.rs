//! Pluggable relay protocol trait.
//!
//! Mirrors the `IngestProtocol` trait in `lvqr-ingest`: a `RelayProtocol`
//! serves MoQ track data to viewers via some transport (MoQ over QUIC,
//! WebSocket fMP4, future HLS or LL-HLS). Implementors run their own listener
//! loop and consume from the supplied `OriginProducer`.
//!
//! Splitting the relay into pluggable protocols means contributors can add a
//! new output (e.g. HLS) without touching the orchestration code in
//! `lvqr-cli`. The CLI just collects `Box<dyn RelayProtocol>` instances and
//! spawns them.

use crate::error::RelayError;
use lvqr_moq::OriginProducer;
use std::future::Future;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

/// Trait implemented by relay/output protocols (MoQ, WS, future HLS, ...).
pub trait RelayProtocol: Send + Sync {
    /// Human-readable name used for logging.
    fn name(&self) -> &str;

    /// Run the protocol until the cancellation token fires.
    fn run<'a>(
        &'a self,
        origin: &'a OriginProducer,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), RelayError>> + Send + 'a>>;
}
