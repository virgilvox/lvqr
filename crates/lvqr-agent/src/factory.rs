//! [`AgentFactory`] trait.
//!
//! See the crate-level docs for the design rationale.

use crate::agent::{Agent, AgentContext};

/// Factory that builds an [`Agent`] for a specific
/// `(broadcast, track)` stream.
///
/// One factory is registered per agent *type* on the
/// [`crate::AgentRunner`]; the factory is then consulted on
/// every new `(broadcast, track)` pair the
/// [`lvqr_fragment::FragmentBroadcasterRegistry`] sees. The
/// factory either returns a fresh `Box<dyn Agent>` (one
/// instance per stream) or `None` to skip this stream.
///
/// `Send + Sync + 'static` so the factory lives in an `Arc`
/// shared across the registry callback's worker thread.
///
/// `name()` is the stable identifier used in metric labels and
/// logs (e.g. `lvqr_agent_fragments_total{agent="captions"}`).
/// Pick something short, lowercase, and snake_case.
pub trait AgentFactory: Send + Sync + 'static {
    /// Stable identifier used in metric labels and logs. Pick
    /// something short, lowercase, and snake_case
    /// (`"captions"`, `"keyframe_thumbnails"`).
    fn name(&self) -> &str;

    /// Build a fresh agent for `ctx`, or return `None` to skip
    /// this `(broadcast, track)` entirely. Returning `None` is
    /// the correct path when the factory wants to opt out --
    /// e.g. a captions agent that only consumes audio tracks
    /// returns `None` for the video track key.
    fn build(&self, ctx: &AgentContext) -> Option<Box<dyn Agent>>;
}
