//! SRT ingest for LVQR.
//!
//! Pure Rust SRT listener (via `srt-tokio`) with integrated MPEG-TS
//! demux (via `lvqr_codec::TsDemuxer`). Accepts SRT connections from
//! broadcast encoders (OBS, vMix, Larix, ffmpeg), demuxes the
//! MPEG-TS transport stream, extracts H.264/HEVC video and AAC
//! audio elementary streams, and converts them to LVQR Fragments
//! that flow through the same `FragmentObserver` chain the RTMP
//! and WHIP bridges use.
//!
//! ## Usage
//!
//! ```text
//! let listener = SrtIngestServer::bind("0.0.0.0:9000").await?;
//! listener.run(origin, fragment_observer, events, shutdown).await;
//! ```
//!
//! Each accepted SRT connection spawns a tokio task that feeds
//! bytes through a `TsDemuxer`, detects codec parameters from
//! the elementary stream, builds fMP4 init segments, and emits
//! Fragments. When the connection drops, the task emits
//! `BroadcastStopped` on the event bus so the HLS/DASH finalize
//! subscribers fire.

pub mod ingest;

pub use ingest::SrtIngestServer;
