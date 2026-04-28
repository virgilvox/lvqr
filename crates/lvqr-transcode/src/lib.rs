//! Server-side transcoding for LVQR (Tier 4 item 4.6).
//!
//! Generates an ABR ladder (720p / 480p / 240p by default) from a
//! single high-resolution source broadcast, with the output
//! renditions re-injected into the caller-supplied
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] under
//! `<source>/<rendition>` broadcast names. Every egress surface
//! (LL-HLS, DASH, MoQ relay, archive) picks them up without
//! per-protocol wiring; the LL-HLS master playlist composer emits
//! one `#EXT-X-STREAM-INF` per rendition automatically.
//!
//! ## What this crate ships
//!
//! Always available (default features):
//!
//! * [`Transcoder`] trait + [`TranscoderFactory`] +
//!   [`TranscoderContext`] -- the "subscribe / drain / panic-isolate"
//!   shape generalised from `lvqr_agent::Agent`. Each factory
//!   carries its own [`RenditionSpec`].
//! * [`TranscodeRunner`] + [`TranscodeRunnerHandle`] -- registry-side
//!   installer + cheaply-cloneable handle, mirroring
//!   `lvqr_agent::AgentRunner`.
//! * [`RenditionSpec`] with width / height / bitrate fields,
//!   [`RenditionSpec::preset_720p`] / `preset_480p` / `preset_240p`
//!   constructors, and [`RenditionSpec::default_ladder`].
//! * [`PassthroughTranscoder`] -- in-memory observer that proves
//!   the registry-callback / drain / panic-isolation wiring; useful
//!   for tests and as a metrics tap.
//! * [`AudioPassthroughTranscoder`] -- copies `<source>/1.mp4` audio
//!   fragments verbatim into every rendition's audio track so each
//!   rendition broadcaster is a self-contained mp4 the LL-HLS
//!   bridge drains without special-casing the missing audio.
//!
//! Behind the `transcode` feature (pulls gstreamer-rs 0.23 +
//! base/good/bad/ugly + gst-libav from the host):
//!
//! * [`SoftwareTranscoder`] / [`SoftwareTranscoderFactory`] -- the
//!   `appsrc -> qtdemux -> h264parse -> avdec_h264 -> videoscale ->
//!   x264enc -> ... -> mp4mux -> appsink` ladder, one worker thread
//!   per `(source, rendition)` pair, bounded mpsc.
//! * [`AacToOpusEncoder`] / [`AacToOpusEncoderFactory`] -- AAC -> Opus
//!   transcoder used by `lvqr-whep` (under its `aac-opus` feature)
//!   so AAC publishers reach Opus-negotiated WHEP subscribers.
//!
//! Behind the `hw-videotoolbox` feature (implies `transcode`;
//! requires the `applemedia` plugin from gst-plugins-bad):
//!
//! * [`VideoToolboxTranscoder`] / [`VideoToolboxTranscoderFactory`]
//!   -- mirrors `SoftwareTranscoderFactory` but swaps the
//!   `x264enc bitrate=... tune=zerolatency speed-preset=superfast`
//!   pipeline element for Apple's HW-only `vtenc_h264_hw bitrate=...
//!   realtime=true allow-frame-reordering=false
//!   max-keyframe-interval=60`. HW-only path is intentional: a
//!   factory that silently falls back to CPU encoding under load
//!   defeats the point of an operator-pickable hardware tier.
//!   `is_available()` probes for the encoder element at construction
//!   and `build()` opts out cleanly with a warn log when missing.
//!
//! NVENC, VAAPI, and QSV stay deferred to v1.2 per the README's
//! existing language. When a third HW backend lands, that session
//! is also the right moment to extract a shared `pipeline.rs`
//! scaffolding module from `software.rs` + `videotoolbox.rs`.
//!
//! ## Where this crate fits in the consumer family
//!
//! Pattern-matches the existing
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] consumers:
//!
//! | Crate | Wires | Purpose |
//! |-------|-------|---------|
//! | `lvqr_cli::hls::BroadcasterHlsBridge` | `on_entry_created` | LL-HLS playlist composition |
//! | `lvqr_cli::archive::BroadcasterArchiveIndexer` | `on_entry_created` | DVR archive index + on-disk segments |
//! | `lvqr_wasm::install_wasm_filter_bridge` | `on_entry_created` | Per-fragment WASM filter tap |
//! | `lvqr_cli::cluster_claim::install_cluster_claim_bridge` | `on_entry_created` | Renew cluster broadcast claim |
//! | `lvqr_agent::AgentRunner` | `on_entry_created` | Per-broadcast user-defined agents |
//! | `lvqr_transcode::TranscodeRunner` | `on_entry_created` | Per-broadcast ABR-ladder transcoders |
//!
//! ## Operator wiring
//!
//! `lvqr-cli` exposes `--transcode-rendition 720p,480p,240p` (or a
//! `.toml` `RenditionSpec` path) and, on `hw-videotoolbox` builds,
//! `--transcode-encoder software|videotoolbox`. End-to-end shape:
//! ingest one source RTMP stream, the LL-HLS master playlist
//! advertises one variant per rendition + the source.

mod audio_passthrough;
mod passthrough;
mod rendition;
mod runner;
mod transcoder;

#[cfg(feature = "transcode")]
mod aac_opus;
#[cfg(feature = "transcode")]
mod software;
#[cfg(feature = "transcode")]
pub mod test_support;

#[cfg(feature = "hw-videotoolbox")]
mod videotoolbox;

pub use audio_passthrough::{AudioPassthroughTranscoder, AudioPassthroughTranscoderFactory};
pub use passthrough::{PassthroughTranscoder, PassthroughTranscoderFactory};
pub use rendition::RenditionSpec;
pub use runner::{TranscodeRunner, TranscodeRunnerHandle, TranscoderStats};
pub use transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

#[cfg(feature = "transcode")]
pub use aac_opus::{AacAudioConfig, AacToOpusEncoder, AacToOpusEncoderFactory, OpusFrame};
#[cfg(feature = "transcode")]
pub use software::{SoftwareTranscoder, SoftwareTranscoderFactory};

#[cfg(feature = "hw-videotoolbox")]
pub use videotoolbox::{VideoToolboxTranscoder, VideoToolboxTranscoderFactory};
