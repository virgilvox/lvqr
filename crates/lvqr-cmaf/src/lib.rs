//! CMAF segmenter for LVQR.
//!
//! This crate is Tier 2.3 of the roadmap (`tracking/ROADMAP.md`). It
//! consumes a [`lvqr_fragment::FragmentStream`] and produces
//! [`CmafChunk`] values aligned for three egress shapes simultaneously:
//!
//! * **HLS partials**: short chunks (default 200 ms) with independent-
//!   flag set on the first chunk of a parent segment so the LL-HLS
//!   playlist generator can emit `EXT-X-PART` tags without re-inspecting
//!   the payload.
//! * **DASH segments**: longer boundaries (default 2 s) at which the
//!   DASH MPD generator emits a new `SegmentTimeline` entry.
//! * **MoQ groups**: every keyframe also closes a MoQ group, matching the
//!   `lvqr_fragment::MoqTrackSink` behavior.
//!
//! A single [`CmafChunk`] carries the segmenter's opinion on all three
//! boundaries so downstream egress crates never need to re-scan the
//! fragment stream. The chunk payload is a complete fMP4 `moof + mdat`
//! pair built through [`mp4_atom`], the kixelated-maintained ISO BMFF
//! writer that already powers the MoQ reference relay.
//!
//! ## Why mp4-atom
//!
//! The hand-rolled writer at `lvqr-ingest::remux::fmp4` is good enough
//! for AVC + AAC but does not know how to emit `hev1` / `hvc1` / `av01`
//! sample entries. `mp4-atom` ships all three plus `Hvcc` (HEVC
//! configuration box) and `Av1C` (AV1 configuration box) types, is pure
//! Rust, MIT OR Apache-2.0, and is maintained by the same author as
//! `moq-lite`. Library research (session 5) confirmed it is the only
//! actively maintained option in the ecosystem; the alternatives are
//! all either Mozilla `mp4parse` (read-only, MPL-2.0, last release May
//! 2023) or `alfg/mp4` (also stale).
//!
//! ## Scope of this scaffold
//!
//! Session 5 lands the types ([`CmafChunk`], [`CmafPolicy`]), the init-
//! segment writer for AVC with resolution extracted via
//! `lvqr_codec::hevc` or `h264-reader`, and the segmenter skeleton. Full
//! HEVC / AV1 / audio-only paths land in follow-up sessions alongside
//! the first egress crate that consumes a `CmafChunk`. The hand-rolled
//! writer in `lvqr-ingest::remux::fmp4` stays in place during the
//! transition so `rtmp_ws_e2e` does not regress.
//!
//! ## 5-artifact contract
//!
//! Day-one coverage is 4 of 5: proptest on the policy state machine,
//! integration test that drives a synthetic `FragmentStream` through
//! the segmenter, workspace-level e2e (the `rtmp_ws_e2e` binary runs
//! the end-to-end path that the segmenter will replace), and an
//! ffprobe conformance check on the init segment. Fuzz slot opens when
//! a parser attack surface lands in this crate (today there is none --
//! the segmenter only reads `Bytes` from a trusted producer and writes
//! mp4-atom structures).

pub mod chunk;
pub mod coalescer;
pub mod init;
pub mod policy;
pub mod sample;
pub mod segmenter;

pub use chunk::{CmafChunk, CmafChunkKind};
pub use coalescer::{CmafSampleSegmenter, TrackCoalescer, build_moof_mdat};
pub use init::{
    AudioInitParams, HevcInitParams, InitSegmentError, OpusInitParams, VideoInitParams, detect_video_codec_string,
    write_aac_init_segment, write_avc_init_segment, write_hevc_init_segment, write_opus_init_segment,
};
pub use policy::{CmafPolicy, CmafPolicyState, PolicyDecision};
pub use sample::{RawSample, SampleStream};
pub use segmenter::{CmafSegmenter, SegmenterError};
