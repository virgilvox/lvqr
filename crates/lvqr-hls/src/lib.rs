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
//! ## Scope of the day-one scaffold
//!
//! * A pure-library manifest generator that emits:
//!   * the media playlist (`#EXTM3U` + `#EXT-X-VERSION:9` + part /
//!     segment entries),
//!   * `#EXT-X-PART` entries for every partial chunk,
//!   * `#EXT-X-PART-INF` so the client knows the target part
//!     duration up front,
//!   * `#EXT-X-SERVER-CONTROL` with `CAN-BLOCK-RELOAD=YES` because
//!     LL-HLS without blocking reload is not LL-HLS.
//! * Types ([`Manifest`], [`Segment`], [`Part`], [`ServerControl`])
//!   that other code can construct without touching the renderer.
//! * A text renderer via [`Manifest::render`] that emits UTF-8
//!   compatible with Apple's `mediastreamvalidator`.
//! * A pure state machine [`PlaylistBuilder`] that consumes
//!   [`lvqr_cmaf::CmafChunk`] values and produces an in-memory
//!   [`Manifest`]. No axum router yet; that lands when the first
//!   real HTTP consumer appears (browser, hls.js, mediastreamvalidator).
//!
//! ## What is NOT in this crate yet
//!
//! * Multivariant master playlists (`#EXT-X-STREAM-INF`). Single
//!   rendition only for now.
//! * Byte-range delivery.
//! * Media segment encryption (`#EXT-X-KEY`).
//! * Discontinuity handling (`#EXT-X-DISCONTINUITY`).
//! * Rendition groups (alternate audio / subtitles).
//! * The actual axum router that serves `playlist.m3u8` and the
//!   part / segment URIs. Session 8 will add this when there is a
//!   real consumer to validate against.
//! * Byte-level conformance against Apple `mediastreamvalidator`.
//!   The 5-artifact contract slot stays open until the validator is
//!   wired in via `lvqr-test-utils::mediastreamvalidator_bytes` (to
//!   be written; same soft-skip pattern as `ffprobe_bytes`).
//!
//! ## 5-artifact contract (day-one state)
//!
//! * **proptest**: yes (`tests/proptest_manifest.rs` — manifest
//!   renderer never panics on arbitrary chunk sequences, output is
//!   always well-formed UTF-8, every `#EXT-X-PART` URI appears in
//!   ascending media sequence order).
//! * **fuzz**: open. No parser attack surface yet; the renderer
//!   only reads structured input. Fuzz lands when a playlist
//!   parser ever enters the crate.
//! * **integration**: minimal (`tests/integration_builder.rs` — drive
//!   a scripted `CmafChunk` sequence through `PlaylistBuilder` and
//!   snapshot the rendered manifest).
//! * **e2e**: open. Lands with the axum router in a later session.
//! * **conformance**: open. Lands when Apple `mediastreamvalidator`
//!   is wired into CI.
//!
//! So day-one coverage is 2 of 5. The remaining three open slots all
//! require code that does not yet exist (the axum router, the
//! validator wrapper). Session 8 or later will fill them.

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
