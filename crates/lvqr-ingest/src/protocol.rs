//! Pluggable ingest protocol trait.
//!
//! This trait defines the contract that all ingest protocols (RTMP, future
//! WHIP, future SRT) implement so the CLI can run them through a single
//! plugin-style interface. The point is to make it easy to add new protocols
//! without touching the orchestration code.
//!
//! ```no_run
//! # use lvqr_ingest::{IngestError, IngestProtocol};
//! use moq_lite::OriginProducer;
//! use tokio_util::sync::CancellationToken;
//!
//! struct MyIngest;
//!
//! impl IngestProtocol for MyIngest {
//!     fn name(&self) -> &str { "my-protocol" }
//!     fn run<'a>(
//!         &'a self,
//!         _origin: &'a OriginProducer,
//!         _cancel: CancellationToken,
//!     ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), IngestError>> + Send + 'a>> {
//!         Box::pin(async move { Ok(()) })
//!     }
//! }
//! ```

use crate::error::IngestError;
use moq_lite::OriginProducer;
use std::future::Future;
use std::pin::Pin;
use tokio_util::sync::CancellationToken;

/// Trait implemented by ingest protocols (RTMP, WHIP, SRT, ...).
///
/// Implementors run their own listener loop and write fMP4 data to MoQ tracks
/// via the supplied `OriginProducer`. The loop must respect `cancel` to allow
/// graceful shutdown.
pub trait IngestProtocol: Send + Sync {
    /// Human-readable name used for logging (e.g. "RTMP", "WHIP").
    fn name(&self) -> &str;

    /// Start accepting connections. Runs until the cancellation token fires.
    ///
    /// Returns `Box<dyn Future>` rather than `async fn` so the trait remains
    /// object-safe (we hold `Box<dyn IngestProtocol>` in the CLI).
    fn run<'a>(
        &'a self,
        origin: &'a OriginProducer,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<(), IngestError>> + Send + 'a>>;
}

#[cfg(feature = "rtmp")]
mod rtmp_impl {
    use super::*;
    use crate::bridge::RtmpMoqBridge;
    use crate::rtmp::RtmpConfig;
    use lvqr_auth::{NoopAuthProvider, SharedAuth};
    use std::sync::Arc;

    /// Adapter wrapping `RtmpMoqBridge + RtmpServer` as an `IngestProtocol`.
    pub struct RtmpIngest {
        config: RtmpConfig,
        auth: SharedAuth,
    }

    impl RtmpIngest {
        pub fn new(config: RtmpConfig) -> Self {
            Self {
                config,
                auth: Arc::new(NoopAuthProvider),
            }
        }

        pub fn with_auth(config: RtmpConfig, auth: SharedAuth) -> Self {
            Self { config, auth }
        }
    }

    impl IngestProtocol for RtmpIngest {
        fn name(&self) -> &str {
            "RTMP"
        }

        fn run<'a>(
            &'a self,
            origin: &'a OriginProducer,
            cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<(), IngestError>> + Send + 'a>> {
            let bridge = RtmpMoqBridge::with_auth(origin.clone(), self.auth.clone());
            let server = bridge.create_rtmp_server(self.config.clone());
            Box::pin(async move { server.run(cancel).await })
        }
    }
}

#[cfg(feature = "rtmp")]
pub use rtmp_impl::RtmpIngest;

// NOTE: no mock/object-safety unit tests live here by design. A test that only
// verifies `Box<dyn IngestProtocol>` compiles is theatrical; the trait's
// real contract is covered by the RTMP integration tests in `tests/`, which
// drive a real publisher against a real `RtmpIngest` through the CLI.
