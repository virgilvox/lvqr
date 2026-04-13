//! Core shared types for LVQR.
//!
//! After the Tier 2.1 fragment-model landing, the in-memory fanout types
//! that used to live here (`Registry`, `RingBuffer`, `GopCache`) are gone
//! -- their role has been taken over by `lvqr-moq` (MoQ routing and
//! fanout via `moq-lite::OriginProducer`) and `lvqr-fragment` (the
//! unified media interchange type). The internal audit at
//! `tracking/AUDIT-INTERNAL-2026-04-13.md` recommended deleting them in
//! the same PR that landed their replacement.
//!
//! What remains here:
//!
//! * [`Frame`] and [`TrackName`]: small value types kept as a stable
//!   cross-crate vocabulary for tests and simple in-memory scenarios.
//! * [`EventBus`] / [`RelayEvent`]: lifecycle bus used by the RTMP
//!   bridge, the WS ingest session, and the recorder to coordinate
//!   broadcast start/stop events without polling.
//! * [`CoreError`]: the shared error type for the above.

pub mod error;
pub mod events;
pub mod types;

pub use error::CoreError;
pub use events::{DEFAULT_EVENT_CAPACITY, EventBus, RelayEvent};
pub use types::*;
