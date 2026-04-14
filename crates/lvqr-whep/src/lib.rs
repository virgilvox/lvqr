//! `lvqr-whep` — WHEP (WebRTC HTTP Egress Protocol) egress for LVQR.
//!
//! The crate lands in stages. Session 15 shipped the H.264 RTP
//! packetizer (the building block with no network dependency).
//! Session 16 adds the signaling layer: the axum router for the
//! WHEP HTTP surface (`POST` / `PATCH` / `DELETE` under
//! `/whep/{broadcast}` and `/whep/{broadcast}/{session_id}`), the
//! session registry, and the trait boundary
//! ([`SdpAnswerer`] + [`SessionHandle`]) that decouples the router
//! from the concrete WebRTC state machine.
//!
//! A future session plugs in `str0m` behind the [`SdpAnswerer`]
//! trait. Once that lands, [`WhepServer`] is wired into
//! `lvqr-cli` via `RtmpMoqBridge::with_raw_sample_observer` so
//! every published RTMP sample flows into every subscribed WHEP
//! client as RTP.
//!
//! The full design note lives at `crates/lvqr-whep/docs/design.md`.

pub mod router;
pub mod rtp;
pub mod server;

pub use router::router as router_for;
pub use rtp::{H264Packetizer, H264RtpPayload};
pub use server::{SdpAnswerer, SessionHandle, SessionId, WhepError, WhepServer};
