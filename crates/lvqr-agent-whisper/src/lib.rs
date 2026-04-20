//! whisper.cpp captions agent for LVQR.
//!
//! **Tier 4 item 4.5, session B.** First concrete `Agent` impl
//! that drops into the session-97-A `lvqr-agent` scaffold: a
//! `WhisperCaptionsAgent` that subscribes to a broadcast's
//! audio track, decodes raw AAC frames out of each fragment's
//! `moof + mdat` payload via symphonia, buffers ~5 s of PCM,
//! and runs whisper.cpp inference on a worker thread to emit
//! `TranscribedCaption` values for downstream consumption
//! (session 99 C wires those into the LL-HLS subtitle rendition
//! group).
//!
//! # Crate shape
//!
//! Two surfaces, gated by the `whisper` Cargo feature:
//!
//! * **Always available** (`cargo build` without features):
//!   `WhisperCaptionsFactory`, `WhisperCaptionsAgent`,
//!   [`TranscribedCaption`], plus the always-pure helpers in
//!   [`mdat`] and [`asc`]. Without the feature the agent's
//!   `on_fragment` is a structured-tracing no-op so the trait
//!   contract still holds and `cargo test --workspace` builds
//!   the crate without paying the whisper.cpp build cost.
//! * **Feature `whisper`** (`cargo build --features whisper`):
//!   pulls in `whisper-rs 0.16` (bindgen + cmake against
//!   whisper.cpp) and `symphonia 0.6.0-alpha.2` (pure-Rust
//!   AAC-LC decoder). The agent's `on_fragment` then forwards
//!   each AAC frame to a worker thread that decodes via
//!   symphonia, buffers PCM, and runs `WhisperContext::full`
//!   on the buffered window.
//!
//! `cargo test --workspace` deliberately runs the no-feature
//! variant so the workspace gate stays fast and CI runners
//! without Xcode CLT / cmake / libclang do not have to compile
//! whisper.cpp on every push. To exercise the inference path:
//!
//! ```bash
//! WHISPER_MODEL_PATH=/path/to/ggml-tiny.en.bin \
//!   cargo test -p lvqr-agent-whisper --features whisper -- --ignored
//! ```
//!
//! # Lifecycle
//!
//! * `WhisperCaptionsFactory::build` returns `Some(agent)` only
//!   for tracks named `"1.mp4"` (the LVQR audio-track convention)
//!   and `None` for every other track. Video / catalog / future
//!   captions tracks see no agent spawn.
//! * `WhisperCaptionsAgent::on_start` (with `whisper` feature)
//!   spawns one OS worker thread that owns the
//!   `WhisperContext`, the symphonia decoder, and the PCM
//!   ring. The agent itself is a thin handle holding a bounded
//!   `tokio::sync::mpsc::Sender<WorkerMessage>` and the
//!   captions-output `tokio::sync::broadcast::Sender`.
//! * `WhisperCaptionsAgent::on_fragment` extracts the raw AAC
//!   frame bytes from the fragment's `moof + mdat` payload via
//!   [`mdat::extract_first_mdat`] and pushes them down the
//!   worker channel. The drain task is never blocked: a full
//!   channel logs `warn!` and drops the frame (caption gaps
//!   beat per-broadcast back-pressure).
//! * `WhisperCaptionsAgent::on_stop` closes the worker channel.
//!   The worker thread drains its remaining PCM, runs one final
//!   inference pass, then exits.
//!
//! The agent is registered against an existing
//! `lvqr_agent::AgentRunner`; session 100 D will thread the
//! factory through `lvqr_cli::start` behind a `--whisper-model
//! <path>` CLI flag. This session leaves the CLI untouched.
//!
//! # Anti-scope (session 98 B)
//!
//! * **No CLI wiring.** Session 100 D does that.
//! * **No HLS subtitle rendition.** Session 99 C wires
//!   `TranscribedCaption` into the `lvqr-hls` MultiHlsServer's
//!   subtitle rendition group; this session leaves the captions
//!   on a public `tokio::sync::broadcast::Receiver` that
//!   session 99 subscribes to.
//! * **No multi-language tuning.** English only. The factory
//!   accepts a `WhisperConfig` so language can be plumbed in
//!   later, but the inference path always uses English in this
//!   session (per section 4.5 anti-scope).
//! * **No GPU acceleration.** whisper-rs ships `metal` /
//!   `cuda` / `coreml` features; LVQR keeps them off so the
//!   default CI build stays portable. Operators with GPUs
//!   enable them via their own `lvqr-cli` build with the
//!   appropriate whisper-rs feature pinned.

pub mod asc;
pub mod caption;
pub mod factory;
pub mod mdat;

mod agent;

#[cfg(feature = "whisper")]
mod decode;
#[cfg(feature = "whisper")]
mod worker;

pub use agent::WhisperCaptionsAgent;
pub use caption::{CaptionStream, TranscribedCaption};
pub use factory::{WhisperCaptionsFactory, WhisperConfig};
