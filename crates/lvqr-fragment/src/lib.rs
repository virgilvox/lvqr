//! Unified Fragment Model for LVQR.
//!
//! This crate is roadmap decision 1 (see `tracking/ROADMAP.md`). Every ingest
//! protocol in LVQR (RTMP, future WHIP/SRT/RTSP/WebTransport) produces
//! [`Fragment`] values; every egress (MoQ, WebSocket fMP4, future HLS/DASH/
//! WHEP, archive recording) consumes them. The goal is that the server has
//! exactly one internal media type and every wire format is a projection of
//! it.
//!
//! ```text
//!   RTMP parser ---+
//!   WHIP RTP  ---+ +-- Fragment --+-- MoQ projection
//!   SRT demux ---+                 +-- HLS projection (future)
//!   RTSP pull ---+                 +-- DASH projection (future)
//!                                  +-- Archive sink
//! ```
//!
//! A [`Fragment`] carries one media unit: a CMAF chunk, an fMP4 segment, or
//! equivalently one MoQ object. It is addressable via
//! `(track_id, group_id, object_id)` so every projection can reconstruct
//! MoQ ordering, DASH segment numbering, and HLS partial segments from the
//! same record.
//!
//! [`FragmentStream`] is the async trait that producers implement and
//! consumers subscribe to. [`MoqTrackSink`] is the first concrete projection:
//! it consumes [`Fragment`] values and writes them into a
//! `lvqr_moq::TrackProducer`, opening a new MoQ group on every keyframe
//! boundary. The RTMP bridge in `lvqr-ingest` writes through this sink.
//!
//! What is intentionally *not* in this crate: CMAF box writing, codec
//! parsing, HLS playlist generation. Those live in `lvqr-ingest::remux`
//! today and will migrate to `lvqr-codec` / `lvqr-cmaf` / `lvqr-hls` when
//! those crates land in Tier 2.2 through 2.5. `lvqr-fragment` owns only the
//! interchange type and the MoQ adapters.

pub mod broadcaster;
pub mod fragment;
pub mod moq_sink;
pub mod moq_stream;
pub mod moq_timing_sink;
pub mod registry;
pub mod stream;

pub use broadcaster::{BroadcasterStream, DEFAULT_BROADCASTER_CAPACITY, FragmentBroadcaster};
pub use fragment::{Fragment, FragmentFlags, FragmentMeta};
pub use moq_sink::{MoqSinkError, MoqTrackSink};
pub use moq_stream::{MoqGroupStream, MoqTrackStream};
pub use moq_timing_sink::{MoqTimingTrackSink, TIMING_ANCHOR_SIZE, TIMING_TRACK_NAME, TimingAnchor, TimingSinkError};
pub use registry::{FragmentBroadcasterRegistry, SCTE35_TRACK};
pub use stream::FragmentStream;
