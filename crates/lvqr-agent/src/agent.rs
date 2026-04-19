//! [`Agent`] trait + [`AgentContext`].
//!
//! See the crate-level docs for the design rationale.

use lvqr_fragment::{Fragment, FragmentMeta};

/// Snapshot of the `(broadcast, track, FragmentMeta)` triple a
/// fresh [`Agent`] sees at construction time.
///
/// The factory consults this to decide whether to build (e.g. a
/// captions agent might return `None` for video tracks). The
/// agent reads it from [`Agent::on_start`] to size its internal
/// state to the broadcast's timescale / codec / init segment
/// without paying for a registry lookup on every fragment.
///
/// `meta` is a snapshot taken at agent-build time. If the
/// underlying broadcaster receives a late init-segment update
/// (typical for RTMP reconnects), the snapshot here goes stale;
/// agents that need the latest init can re-read it off the
/// fragment's incoming subscription via the registry. For the
/// 4.5 captions agent the snapshot is sufficient because the
/// codec / timescale never change mid-broadcast.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// `<app>/<name>` -- e.g. `"live/cam1"`.
    pub broadcast: String,

    /// MoQ track name, e.g. `"0.mp4"` for video, `"1.mp4"` for
    /// audio. Matches the convention every other LVQR
    /// `(broadcast, track)`-keyed surface uses.
    pub track: String,

    /// Codec, timescale, and (optionally) the init segment
    /// available at agent-build time.
    pub meta: FragmentMeta,
}

/// In-process consumer of [`Fragment`] values for one
/// `(broadcast, track)`.
///
/// Lifecycle: [`Agent::on_start`] is called exactly once before
/// the first [`Agent::on_fragment`]. Then [`Agent::on_fragment`]
/// fires for every fragment the broadcaster emits while the
/// agent is alive. When the broadcaster closes (every producer-
/// side clone dropped), [`Agent::on_stop`] is called exactly
/// once and the agent is dropped. Each call is wrapped in
/// `std::panic::catch_unwind` by the runner; see the crate-level
/// docs.
///
/// The trait is sync. Agents that want async or blocking work
/// (e.g. a `whisper-rs` decoder) spawn from inside `on_start`
/// (typical: a bounded `tokio::sync::mpsc` to a worker task
/// that owns the heavy state) and forward each fragment down
/// the channel from `on_fragment`. Doing the work inline in
/// `on_fragment` is supported but blocks the per-broadcast
/// drain task, which the [`lvqr_fragment::FragmentBroadcaster`]
/// will then back-pressure with the documented
/// `RecvError::Lagged` skip semantics.
pub trait Agent: Send {
    /// One-shot setup. Called exactly once before the first
    /// [`Agent::on_fragment`]. Default: no-op so factories can
    /// register stateless agents without boilerplate.
    fn on_start(&mut self, _ctx: &AgentContext) {}

    /// Process one fragment from the live stream.
    ///
    /// Synchronous. Implementations that need to do heavy or
    /// blocking work MUST offload to a separate thread (typical
    /// pattern: a bounded `tokio::sync::mpsc` to a worker
    /// task spawned in [`Agent::on_start`]).
    fn on_fragment(&mut self, fragment: &Fragment);

    /// One-shot teardown. Called exactly once after the
    /// underlying [`lvqr_fragment::BroadcasterStream`] returns
    /// `None` (every producer-side clone of the broadcaster has
    /// been dropped). Default: no-op.
    ///
    /// Note: when the [`crate::AgentRunnerHandle`] is dropped,
    /// the spawned tokio task is aborted mid-stride and
    /// `on_stop` does NOT fire. This matches the
    /// `WasmFilterBridgeHandle` shutdown semantics.
    fn on_stop(&mut self) {}
}
