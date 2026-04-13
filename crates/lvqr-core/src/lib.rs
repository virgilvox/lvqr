//! Core types and data structures for LVQR.
//!
//! This crate provides shared types (`Frame`, `TrackName`, `RelayStats`) used across the
//! LVQR workspace, plus standalone data structures (`RingBuffer`, `GopCache`, `Registry`)
//! that are tested and benchmarked but **not currently in the relay's hot path**.
//!
//! The relay uses moq-lite's `OriginProducer` for all track routing and fan-out.
//! The structures here exist for:
//! - Shared type definitions consumed by `lvqr-admin`, `lvqr-relay`, `lvqr-ingest`, etc.
//! - Future use: WS-fMP4 fallback delivery, stats aggregation, or custom fan-out paths
//!   that need GOP-aware buffering outside of moq-lite.
//!
//! If you're looking for the actual media data path, see `lvqr-relay` and `moq-lite`.

pub mod error;
pub mod events;
pub mod gop;
pub mod registry;
pub mod ringbuf;
pub mod types;

pub use error::CoreError;
pub use events::{DEFAULT_EVENT_CAPACITY, EventBus, RelayEvent};
pub use gop::GopCache;
pub use registry::{Registry, Subscription};
pub use ringbuf::RingBuffer;
pub use types::*;
