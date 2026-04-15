//! `lvqr-whip` — WHIP (WebRTC HTTP Ingest Protocol) ingest for LVQR.
//!
//! Mirrors the `lvqr-whep` shape: an axum signaling router on top of
//! a trait boundary ([`SdpAnswerer`] / [`SessionHandle`]), a sans-IO
//! `str0m` poll loop, and a bridge that lands incoming H.264 samples
//! in the Unified Fragment Model so every existing egress (MoQ,
//! LL-HLS, WHEP, disk record, DVR archive) picks up WHIP ingest with
//! zero changes to the egress side.
//!
//! Session 25 closes the single biggest hole in the competitive
//! matrix: "any WebRTC client can publish to LVQR". The ingest
//! shape is deliberately a sibling of `RtmpMoqBridge` rather than a
//! refactor of it — the two bridges share observer types but not a
//! common state machine, and the existing RTMP path is untouched.

pub mod bridge;
pub mod depack;
pub mod router;
pub mod server;
pub mod str0m_backend;

pub use bridge::{IngestSample, IngestSampleSink, NoopIngestSampleSink, VideoCodec, WhipMoqBridge};
pub use depack::{annex_b_to_avcc, hevc_nal_type, split_annex_b};
pub use router::router as router_for;
pub use server::{SdpAnswerer, SessionHandle, SessionId, WhipError, WhipServer};
pub use str0m_backend::{Str0mIngestAnswerer, Str0mIngestConfig, Str0mIngestSessionHandle};
