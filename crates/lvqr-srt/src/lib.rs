//! SRT ingest for LVQR.
//!
//! Pure Rust SRT listener (via `srt-tokio`) with integrated MPEG-TS
//! demux (via `lvqr_codec::TsDemuxer`). Accepts SRT connections from
//! broadcast encoders (OBS, vMix, Larix, ffmpeg), demuxes the
//! MPEG-TS transport stream, extracts H.264/HEVC video and AAC
//! audio elementary streams, and converts them to LVQR Fragments
//! that publish onto the shared
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] the RTMP, WHIP,
//! and RTSP bridges also feed.
//!
//! ## Usage
//!
//! ```text
//! let mut listener = SrtIngestServer::with_registry(addr, registry);
//! listener.bind().await?;
//! listener.run(events, shutdown).await;
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
