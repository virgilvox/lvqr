//! `lvqr-whep` -- WHEP (WebRTC HTTP Egress Protocol) egress for LVQR.
//!
//! Axum signaling router on top of a trait boundary ([`SdpAnswerer`] +
//! [`SessionHandle`]) that decouples the HTTP shape from the concrete
//! WebRTC state machine. The HTTP surface mounts under
//! `/whep/{broadcast}` and `/whep/{broadcast}/{session_id}` for the
//! standard `POST` / `PATCH` / `DELETE` lifecycle. The concrete
//! `str0m` backend ([`Str0mAnswerer`]) plugs into the trait and is
//! re-exported below; lvqr-cli wires `WhepServer` into the relay
//! through the existing `RawSampleObserver` tap on
//! `RtmpMoqBridge::with_raw_sample_observer` so every published
//! sample (RTMP, SRT, RTSP, WS, WHIP) flows into every subscribed
//! WHEP client as RTP.
//!
//! ## Codecs
//!
//! H.264, HEVC, and Opus pass through directly. AAC publishers
//! (RTMP / SRT / RTSP / WS) reach Opus-negotiated WHEP subscribers
//! via the in-process [`lvqr_transcode::AacToOpusEncoder`] (session
//! 113), behind this crate's `aac-opus` Cargo feature which forwards
//! to `lvqr-transcode/transcode` so the GStreamer dep graph stays
//! opt-in.
//!
//! ## Open items
//!
//! Trickle ICE ingestion is still TODO (one-shot warn flag per
//! session in [`Str0mSessionHandle`]); operators relying on
//! candidate trickling should keep this in mind.
//!
//! The full design note lives at `crates/lvqr-whep/docs/design.md`.

pub mod router;
pub mod rtp;
pub mod server;
pub mod str0m_backend;

pub use router::router as router_for;
pub use rtp::{H264Packetizer, H264RtpPayload};
pub use server::{SdpAnswerer, SessionHandle, SessionId, WhepError, WhepServer};
pub use str0m_backend::{Str0mAnswerer, Str0mConfig, Str0mSessionHandle};

// Tier 4 item 4.7 session 110 B: re-export the shared latency
// tracker type so callers of `Str0mAnswerer::with_slo_tracker` do
// not need a direct `lvqr-admin` dep just to name the argument
// type. The re-export mirrors the `lvqr-cli` pattern from 107 A.
pub use lvqr_admin::LatencyTracker;
