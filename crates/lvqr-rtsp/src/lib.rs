//! RTSP/1.0 server for LVQR.
//!
//! Pure-Rust RTSP listener built around an explicit session state
//! machine ([`session::Session`] / [`session::SessionState`]),
//! per-codec RTP depacketizers ([`rtp::H264Depacketizer`],
//! [`rtp::HevcDepacketizer`], [`rtp::AacDepacketizer`]),
//! interleaved-RTP transport over the same TCP connection as the
//! request stream, and an SDP parser ([`sdp::parse_sdp_tracks`])
//! that picks out the codec + payload-type bindings the publisher
//! advertised in ANNOUNCE.
//!
//! ## Methods
//!
//! `OPTIONS`, `DESCRIBE`, `ANNOUNCE`, `SETUP`, `PLAY`, `RECORD`,
//! `TEARDOWN`, `GET_PARAMETER` (per the `Public:` header at
//! `crates/lvqr-rtsp/src/server.rs:28`). Both publisher (RECORD) and
//! subscriber (PLAY) flows ship; on the publisher side raw samples
//! land in [`lvqr_fragment`] via [`lvqr_ingest::publish_fragment`] +
//! [`lvqr_ingest::publish_init`] so every existing egress (HLS,
//! DASH, MoQ, archive) picks up RTSP-ingested broadcasts.
//!
//! ## Cluster integration
//!
//! [`OwnerResolver`] mirrors `lvqr_hls::OwnerResolver` and
//! `lvqr_dash::OwnerResolver`: a non-owner relay returns RTSP
//! `302 Moved Temporarily` with `Location: <owner_url>/<broadcast>`
//! on DESCRIBE / PLAY when the resolver returns `Some(url)`. Wired
//! through the lvqr-cli composition root via
//! `lvqr_cluster::Cluster::find_owner_endpoints`.

pub mod fmp4;
pub mod play;
pub mod proto;
pub mod rtcp;
pub mod rtp;
pub mod sdp;
pub mod server;
pub mod session;

pub use server::{OwnerResolver, RedirectFuture, RtspServer};
