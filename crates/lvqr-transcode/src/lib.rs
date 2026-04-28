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
//! Behind one of the `hw-*` features (each implies `transcode`):
//!
//! * `hw-videotoolbox` -- [`VideoToolboxTranscoder`] /
//!   [`VideoToolboxTranscoderFactory`] for macOS via Apple's
//!   `vtenc_h264_hw` (the `applemedia` plugin from gst-plugins-bad).
//! * `hw-nvenc` -- [`NvencTranscoder`] / [`NvencTranscoderFactory`]
//!   for Linux + Nvidia GPUs via `nvh264enc` (the `nvcodec` plugin
//!   from gst-plugins-bad, driven by the CUDA runtime).
//! * `hw-vaapi` -- [`VaapiTranscoder`] / [`VaapiTranscoderFactory`]
//!   for Linux + Intel iGPU / AMD via `vah264enc` (the modern `va`
//!   plugin from gst-plugins-bad, superseding the deprecated
//!   `vaapih264enc` from `gstreamer-vaapi`).
//! * `hw-qsv` -- [`QsvTranscoder`] / [`QsvTranscoderFactory`] for
//!   Linux + Intel Quick Sync via `qsvh264enc` (the `qsv` plugin
//!   from gst-plugins-bad, driving Intel Media SDK / oneVPL).
//!
//! All four HW backends mirror `SoftwareTranscoderFactory` shape
//! verbatim -- same `Transcoder` trait, same lifecycle, same
//! `<source>/<rendition>` output broadcast naming, same bounded
//! mpsc + dedicated worker thread per `(source, rendition)` pair --
//! and only swap the GStreamer encoder element + property mapping.
//! HW-only path is intentional across all four: a factory that
//! silently falls back to CPU encoding under load defeats the point
//! of an operator-pickable hardware tier. Each factory's
//! `is_available()` probes the required encoder element at
//! construction and `build()` opts out cleanly with a warn log when
//! missing.
//!
//! Future sessions may extract the shared scaffolding into a
//! dedicated `pipeline.rs` module (per the "three is the threshold
//! for an abstraction" rule). The current shape is intentional code
//! duplication: each backend stays readable on its own and the cost
//! of cross-backend changes is small enough that the mechanical-
//! sharing tradeoff is not yet a win.
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

#[cfg(feature = "hw-nvenc")]
mod nvenc;

#[cfg(feature = "hw-vaapi")]
mod vaapi;

#[cfg(feature = "hw-qsv")]
mod qsv;

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

#[cfg(feature = "hw-nvenc")]
pub use nvenc::{NvencTranscoder, NvencTranscoderFactory};

#[cfg(feature = "hw-vaapi")]
pub use vaapi::{VaapiTranscoder, VaapiTranscoderFactory};

#[cfg(feature = "hw-qsv")]
pub use qsv::{QsvTranscoder, QsvTranscoderFactory};
