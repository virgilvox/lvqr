//! HTTP Live Streaming (HLS) egress for LVQR.
//!
//! This crate is the first egress protocol to land on top of the
//! Tier 2.3 [`lvqr_cmaf::CmafChunk`] type. It produces HLS and
//! Low-Latency HLS (LL-HLS) manifests from a sequence of `CmafChunk`
//! values. Per the competitive audit (`tracking/AUDIT-2026-04-13.md`),
//! LL-HLS is the single loudest v1.0 gap: every evaluator of a live
//! video server asks "can it serve HLS?" and the answer being "no,
//! we only do MoQ and WS" is not acceptable.
//!
//! ## What this crate ships
//!
//! * [`PlaylistBuilder`] -- pure state machine that consumes
//!   [`lvqr_cmaf::CmafChunk`] values and produces an in-memory
//!   [`Manifest`].
//! * Manifest renderer ([`Manifest::render`]) emitting:
//!   * the media playlist (`#EXTM3U` + `#EXT-X-VERSION:9` + part /
//!     segment entries),
//!   * `#EXT-X-PART` entries for every partial chunk,
//!   * `#EXT-X-PART-INF` for the target part duration up front,
//!   * `#EXT-X-SERVER-CONTROL` with `CAN-BLOCK-RELOAD=YES` because
//!     LL-HLS without blocking reload is not LL-HLS,
//!   * per-segment `#EXT-X-PROGRAM-DATE-TIME` (load-bearing for
//!     glass-to-glass SLO recovery via `getStartDate()` in the
//!     `@lvqr/dvr-player` SLO sampler),
//!   * `#EXT-X-DATERANGE` for SCTE-35 ad markers (session 152) with
//!     `CLASS="urn:scte:scte35:2014:bin"` and SCTE35-OUT/IN/CMD
//!     attributes (see [`SCTE35_DATERANGE_CLASS`]).
//! * Multivariant master playlist via [`MasterPlaylist`] +
//!   [`VariantStream`] (session 106 C transcode ladder); each
//!   transcode rendition surfaces as one `#EXT-X-STREAM-INF` line.
//! * Subtitles rendition group via [`SubtitlesServer`] for the
//!   whisper captions agent (Tier 4 item 4.5 session 99 C).
//! * HTTP serving surface via [`HlsServer`] + [`MultiHlsServer`]
//!   with cluster-aware [`OwnerResolver`] redirect-to-owner.
//!
//! ## What is NOT in this crate
//!
//! * Byte-range delivery (segments are served whole today).
//! * Media segment encryption (`#EXT-X-KEY`).
//! * Discontinuity handling (`#EXT-X-DISCONTINUITY`).
//!
//! ## 5-artifact contract
//!
//! * **proptest**: shipped (`tests/proptest_manifest.rs` -- manifest
//!   renderer never panics on arbitrary chunk sequences, output is
//!   always well-formed UTF-8, every `#EXT-X-PART` URI appears in
//!   ascending media sequence order).
//! * **fuzz**: shipped (`fuzz/fuzz_targets/playlist_builder.rs`).
//! * **integration**: shipped (`tests/integration_builder.rs`).
//! * **e2e**: shipped via the lvqr-cli RTMP -> HLS integration
//!   tests under `crates/lvqr-cli/tests/`.
//! * **conformance**: still open -- byte-level conformance against
//!   Apple `mediastreamvalidator` lands when a `lvqr-test-utils::
//!   mediastreamvalidator_bytes` helper is written (same soft-skip
//!   pattern as `ffprobe_bytes`). This is the largest known
//!   conformance gap on the HLS side.

pub mod manifest;
pub mod master;
pub mod server;
pub mod subtitles;

pub use manifest::{
    DateRange, DateRangeKind, HlsError, Manifest, Part, PlaylistBuilder, PlaylistBuilderConfig, SCTE35_DATERANGE_CLASS,
    Segment, ServerControl, render_manifest,
};
pub use master::{MasterPlaylist, MediaRendition, MediaRenditionType, RenditionMeta, VariantStream};
pub use server::{HlsServer, MultiHlsServer, OwnerResolver, RedirectFuture};
pub use subtitles::{CaptionCue, DEFAULT_MAX_CUES, DEFAULT_MIN_TARGET_DURATION_SECS, SubtitlesServer, now_unix_millis};
