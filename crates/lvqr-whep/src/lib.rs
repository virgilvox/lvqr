//! `lvqr-whep` — WHEP (WebRTC HTTP Egress Protocol) egress for LVQR.
//!
//! This crate is landing in stages. Session 14 (the start of the WHEP
//! implementation) ships only the building block that has no network
//! dependency: the H.264 RTP packetizer. The signaling layer, the
//! `str0m` integration, and the axum router all land in a later
//! session. The crate-level design note lives at
//! `crates/lvqr-whep/docs/design.md`; read it before extending the
//! surface.
//!
//! The H.264 packetizer consumes AVCC length-prefixed NAL unit bytes
//! (the same shape `lvqr_cmaf::RawSample::payload` carries for AVC)
//! and produces RFC 6184 RTP *payloads* — just the payload bytes
//! each packet places after its RTP header. The WebRTC stack
//! (`str0m`) writes the RTP header, sequence number, SSRC, and
//! marker bit from its own state machine.
//!
//! Single-NAL-unit mode (§5.6) is used when a NAL fits inside the
//! configured MTU budget. FU-A fragmentation (§5.8) is used when a
//! NAL exceeds the budget. STAP-A (§5.7) aggregation is intentionally
//! omitted for v0.x; it is a micro-optimization for tiny NALs that
//! adds a second encode path, and browser WHEP clients accept the
//! single-NAL form just fine.

pub mod rtp;

pub use rtp::{H264Packetizer, H264RtpPayload};
