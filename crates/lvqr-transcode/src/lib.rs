//! Server-side transcoding for LVQR.
//!
//! **Tier 4 item 4.6, session 104 A scaffold.** This is the crate
//! referenced by `tracking/TIER_4_PLAN.md` section 4.6. The goal is
//! to let LVQR generate an ABR ladder (720p / 480p / 240p by
//! default) from a single high-resolution source broadcast, with
//! the output renditions re-injected into the local
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] so every egress
//! surface (LL-HLS, DASH, MoQ relay, archive) serves them as if
//! they had been ingested directly.
//!
//! # Session 104 A scope
//!
//! Scaffold + one pass-through transcoder:
//!
//! * [`Transcoder`] trait + [`TranscoderFactory`] + [`TranscoderContext`]
//!   modeled on the `lvqr-agent` crate's
//!   [`Agent`](lvqr_fragment::FragmentBroadcasterRegistry) shape so
//!   operators see one consistent "subscriber-with-lifecycle"
//!   idiom across WASM filters, AI agents, and transcoders.
//! * [`TranscodeRunner`] + [`TranscodeRunnerHandle`]: registry-side
//!   installer + cheaply-cloneable handle, exactly mirroring
//!   `AgentRunner` in `lvqr-agent`.
//! * [`RenditionSpec`] with width / height / bitrate fields and
//!   three preset constructors ([`RenditionSpec::preset_720p`],
//!   [`RenditionSpec::preset_480p`], [`RenditionSpec::preset_240p`])
//!   plus [`RenditionSpec::default_ladder`] for the LVQR default
//!   3-rung ladder.
//! * [`PassthroughTranscoder`] + [`PassthroughTranscoderFactory`]:
//!   the 104 A concrete implementation. Logs + counts per-fragment
//!   but does NOT actually encode or republish output. Exists to
//!   prove the end-to-end wiring (`FragmentBroadcasterRegistry`
//!   callback -> drain task -> per-rendition transcoder
//!   instance) before the `gstreamer-rs` pipelines land in
//!   session 105 B.
//!
//! # What session 105 B adds
//!
//! * Real `gstreamer-rs` pipelines gated behind a
//!   `transcode` Cargo feature (default OFF so CI runners without
//!   gstreamer plugins continue to build).
//! * A `SoftwareTranscoder` using `appsrc -> qtdemux -> h264parse
//!   -> avdec_h264 -> videoscale -> x264enc -> ... -> mp4mux ->
//!   appsink` (plus passthrough audio) and re-injecting the output
//!   into the caller-supplied
//!   [`lvqr_fragment::FragmentBroadcasterRegistry`] as a new
//!   broadcast named `<source>/<rendition>` (e.g. `live/foo/720p`).
//! * Optional hardware-encoder backends behind per-encoder feature
//!   flags (`hw-nvenc`, `hw-vaapi`, `hw-qsv`, `hw-videotoolbox`).
//!
//! # What session 106 C adds
//!
//! * `lvqr-cli` wiring (`--transcode-rendition 720p,480p,240p`
//!   flag + `ServeConfig::transcode_renditions`).
//! * LL-HLS master playlist composition: the HLS bridge learns
//!   about source -> rendition relationships so one master
//!   playlist references every rendition as a variant with
//!   `BANDWIDTH` / `RESOLUTION` matching
//!   [`RenditionSpec`].
//! * End-to-end demo: ingest one 1080p RTMP stream, watch the
//!   LL-HLS master playlist advertise four variants
//!   (source + three ladder rungs).
//!
//! # Anti-scope (session 104 A)
//!
//! * **No `lvqr-cli` wiring.** 106 C owns the composition root.
//! * **No gstreamer dependency.** 105 B owns the real pipeline.
//!   Session 104 A ships a pass-through that exists only to
//!   prove the `FragmentBroadcasterRegistry` subscribe /
//!   drain / panic-isolation wiring without pulling a heavy C
//!   dep into the workspace build.
//! * **No output re-publish.** 104 A transcoders are observers
//!   only. Session 105 B adds the output side.
//! * **No config-file / admin-API ladder override.** 105 B +
//!   106 C own operator-facing configuration.
//! * **No HLS master-playlist integration.** 106 C owns the
//!   egress wiring.
//!
//! # Where this crate fits in the consumer family
//!
//! Pattern-matches the five existing
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] consumers:
//!
//! | Crate | Wires | Purpose |
//! |-------|-------|---------|
//! | `lvqr_cli::hls::BroadcasterHlsBridge` | `on_entry_created` | LL-HLS playlist composition |
//! | `lvqr_cli::archive::BroadcasterArchiveIndexer` | `on_entry_created` | DVR archive index + on-disk segments |
//! | `lvqr_wasm::install_wasm_filter_bridge` | `on_entry_created` | Per-fragment WASM filter tap |
//! | `lvqr_cli::cluster_claim::install_cluster_claim_bridge` | `on_entry_created` | Renew cluster broadcast claim |
//! | `lvqr_agent::AgentRunner` | `on_entry_created` | Per-broadcast user-defined agents |
//! | **`lvqr_transcode::TranscodeRunner`** (new) | `on_entry_created` | Per-broadcast ABR-ladder transcoders |
//!
//! No new abstractions invented: the trait surface is a
//! one-method generalisation of [`lvqr_agent`]'s `Agent` /
//! `AgentFactory` / `AgentRunner`, re-shaped so each factory
//! carries its own [`RenditionSpec`]. Every existing consumer
//! already encodes the same subscribe / drain / panic-isolate
//! pattern by hand.

mod passthrough;
mod rendition;
mod runner;
mod transcoder;

#[cfg(feature = "transcode")]
mod software;

pub use passthrough::{PassthroughTranscoder, PassthroughTranscoderFactory};
pub use rendition::RenditionSpec;
pub use runner::{TranscodeRunner, TranscodeRunnerHandle, TranscoderStats};
pub use transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

#[cfg(feature = "transcode")]
pub use software::{SoftwareTranscoder, SoftwareTranscoderFactory};
