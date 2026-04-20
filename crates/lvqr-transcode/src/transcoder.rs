//! [`Transcoder`] trait + [`TranscoderFactory`] + [`TranscoderContext`].
//!
//! Mirrors [`lvqr_agent::Agent`] / [`lvqr_agent::AgentFactory`]
//! with a [`RenditionSpec`] carried through the context, so a
//! single factory type (e.g. `SoftwareTranscoderFactory` in
//! session 105 B) can be instantiated multiple times, once per
//! rendition, to produce an ABR ladder.

use lvqr_fragment::{Fragment, FragmentMeta};

use crate::rendition::RenditionSpec;

/// Snapshot of the `(broadcast, track, FragmentMeta, rendition)`
/// tuple a fresh [`Transcoder`] sees at construction time.
///
/// `meta` is a snapshot at transcoder-build time; see the same
/// caveat as [`lvqr_agent::AgentContext::meta`]. For the 4.6
/// software / hardware encoders the codec + timescale never
/// change mid-broadcast, so the snapshot is sufficient.
#[derive(Debug, Clone)]
pub struct TranscoderContext {
    /// Source broadcast name (`<app>/<name>`), e.g. `"live/cam1"`.
    ///
    /// Session 105 B's output broadcast name is derived as
    /// `<broadcast>/<rendition.name>` (e.g. `live/cam1/720p`).
    pub broadcast: String,

    /// Source track name. Typically `"0.mp4"` for video; session
    /// 104 A filters non-video tracks at the factory level via
    /// [`PassthroughTranscoderFactory::build`](crate::PassthroughTranscoderFactory).
    pub track: String,

    /// Codec, timescale, and (optionally) the init segment of the
    /// source track at transcoder-build time.
    pub meta: FragmentMeta,

    /// Target rendition for this transcoder instance. One
    /// transcoder + one rendition per ladder rung.
    pub rendition: RenditionSpec,
}

/// In-process consumer of source [`Fragment`] values for one
/// `(broadcast, track, rendition)` tuple. The 104 A trait is
/// observe-only; 105 B extends the concrete implementations with
/// an output-publish side without changing the trait surface.
///
/// Lifecycle is identical to [`lvqr_agent::Agent`]:
///
/// * [`Transcoder::on_start`] runs exactly once before the first
///   [`Transcoder::on_fragment`].
/// * [`Transcoder::on_fragment`] fires for every source fragment.
/// * [`Transcoder::on_stop`] runs exactly once after the source
///   [`lvqr_fragment::BroadcasterStream`] closes (every producer
///   clone dropped).
///
/// All three calls are wrapped in
/// `std::panic::catch_unwind(AssertUnwindSafe(..))` by the
/// [`crate::TranscodeRunner`]; see the crate-level docs.
///
/// The trait is sync. Transcoders that want async or blocking
/// work (every real gstreamer pipeline) spawn from inside
/// [`Transcoder::on_start`] with a bounded channel to a worker
/// thread / task -- typical pattern for the 105 B
/// `SoftwareTranscoder`.
pub trait Transcoder: Send {
    /// One-shot setup. Called exactly once before the first
    /// [`Transcoder::on_fragment`]. Default: no-op.
    fn on_start(&mut self, _ctx: &TranscoderContext) {}

    /// Process one source fragment.
    ///
    /// Synchronous. Heavy work MUST be offloaded to a worker
    /// thread spawned in `on_start`; blocking here back-pressures
    /// the per-broadcast drain task with
    /// [`lvqr_fragment::FragmentBroadcaster`]'s documented
    /// `RecvError::Lagged` skip semantics.
    fn on_fragment(&mut self, fragment: &Fragment);

    /// One-shot teardown after the source broadcaster closes.
    /// Default: no-op.
    ///
    /// `on_stop` does NOT fire when the
    /// [`crate::TranscodeRunnerHandle`] is dropped mid-stride (the
    /// spawned task is aborted). Matches the
    /// [`lvqr_agent::AgentRunner`] shutdown shape.
    fn on_stop(&mut self) {}
}

/// Factory that builds a [`Transcoder`] for one specific
/// rendition of one specific `(broadcast, track)` stream.
///
/// One factory instance per rendition: for an ABR ladder of three
/// rungs the caller registers three factory instances on the
/// [`crate::TranscodeRunner`], each carrying its own
/// [`RenditionSpec`]. The factory either returns a fresh
/// `Box<dyn Transcoder>` (one instance per source stream the
/// factory opts into) or `None` to skip this stream.
///
/// `Send + Sync + 'static` so the factory lives behind an `Arc`
/// shared across the registry callback's worker thread.
pub trait TranscoderFactory: Send + Sync + 'static {
    /// Stable identifier used in metric labels and logs (e.g.
    /// `"passthrough"`, `"x264"`, `"nvenc"`). Pick something
    /// short, lowercase, snake_case.
    fn name(&self) -> &str;

    /// Target rendition this factory produces. Consumed by the
    /// [`crate::TranscodeRunner`] when building the
    /// [`TranscoderContext`].
    fn rendition(&self) -> &RenditionSpec;

    /// Build a fresh transcoder for `ctx`, or return `None` to
    /// skip this `(broadcast, track)` entirely. Returning `None`
    /// is the correct path when the factory wants to opt out --
    /// e.g. a video transcoder returning `None` for an audio
    /// track, or a factory targeting only source broadcasts (not
    /// already-transcoded renditions).
    fn build(&self, ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>>;
}
