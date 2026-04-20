//! [`PassthroughTranscoder`] + [`PassthroughTranscoderFactory`].
//!
//! Scaffold implementation for session 104 A. Observes source
//! fragments and counts them but does not actually transcode or
//! republish. Exists to prove the
//! [`lvqr_fragment::FragmentBroadcasterRegistry`] callback /
//! subscribe / drain / panic-isolation wiring end-to-end before
//! session 105 B pulls `gstreamer-rs` in.
//!
//! The factory defaults to video-only: it returns `None` for any
//! track other than `"0.mp4"`. Operators who want to observe
//! audio / captions / catalog tracks can construct their own
//! factory with a wider track filter.

use lvqr_fragment::Fragment;
use tracing::{debug, info};

use crate::rendition::RenditionSpec;
use crate::transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

/// Source track the 104 A pass-through accepts. Video only; audio
/// passes through the registry untouched, and caption / catalog
/// tracks have no transcoder use case on the 4.6 ladder.
const DEFAULT_SOURCE_TRACK: &str = "0.mp4";

/// Pass-through transcoder: logs each fragment and counts calls
/// but does NOT encode or republish. The real encoder lives in
/// session 105 B.
///
/// Held by the [`crate::TranscodeRunner`]'s drain task. One
/// instance per `(source_broadcast, rendition)` pair the factory
/// opts into.
pub struct PassthroughTranscoder {
    rendition_name: String,
    fragments_seen: u64,
}

impl PassthroughTranscoder {
    /// Construct a fresh transcoder for `rendition`. The
    /// [`crate::TranscodeRunner`] owns the call site; operators
    /// typically use [`PassthroughTranscoderFactory`] instead of
    /// constructing one directly.
    pub fn new(rendition: &RenditionSpec) -> Self {
        Self {
            rendition_name: rendition.name.clone(),
            fragments_seen: 0,
        }
    }

    /// How many fragments this transcoder has observed. Exposed
    /// for test assertions; production code should consult the
    /// [`crate::TranscodeRunnerHandle`]'s per-
    /// `(transcoder, rendition, broadcast, track)` counters
    /// instead.
    pub fn fragments_seen(&self) -> u64 {
        self.fragments_seen
    }
}

impl Transcoder for PassthroughTranscoder {
    fn on_start(&mut self, ctx: &TranscoderContext) {
        info!(
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            rendition = %ctx.rendition.name,
            width = ctx.rendition.width,
            height = ctx.rendition.height,
            "passthrough transcoder started (scaffold; does not re-encode)",
        );
    }

    fn on_fragment(&mut self, fragment: &Fragment) {
        self.fragments_seen = self.fragments_seen.saturating_add(1);
        debug!(
            rendition = %self.rendition_name,
            group_id = fragment.group_id,
            object_id = fragment.object_id,
            bytes = fragment.payload.len(),
            "passthrough transcoder observed fragment",
        );
    }

    fn on_stop(&mut self) {
        info!(
            rendition = %self.rendition_name,
            seen = self.fragments_seen,
            "passthrough transcoder stopped",
        );
    }
}

/// Factory that builds a [`PassthroughTranscoder`] for each
/// video-track source stream a [`crate::TranscodeRunner`] sees.
///
/// Constructed with a single [`RenditionSpec`]; register N
/// factory instances on the runner (typically three, one per
/// rung of [`RenditionSpec::default_ladder`]) to scaffold an
/// ABR ladder's observability without the gstreamer dep.
pub struct PassthroughTranscoderFactory {
    rendition: RenditionSpec,
}

impl PassthroughTranscoderFactory {
    /// Build a factory for the supplied rendition.
    pub fn new(rendition: RenditionSpec) -> Self {
        Self { rendition }
    }
}

impl TranscoderFactory for PassthroughTranscoderFactory {
    fn name(&self) -> &str {
        "passthrough"
    }

    fn rendition(&self) -> &RenditionSpec {
        &self.rendition
    }

    fn build(&self, ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>> {
        if ctx.track != DEFAULT_SOURCE_TRACK {
            return None;
        }
        Some(Box::new(PassthroughTranscoder::new(&ctx.rendition)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};

    fn ctx(track: &str, rendition: RenditionSpec) -> TranscoderContext {
        TranscoderContext {
            broadcast: "live/demo".into(),
            track: track.into(),
            meta: FragmentMeta::new("avc1.640028", 90_000),
            rendition,
        }
    }

    fn frag(idx: u64) -> Fragment {
        Fragment::new(
            "0.mp4",
            idx,
            0,
            0,
            idx * 1000,
            idx * 1000,
            1000,
            FragmentFlags::DELTA,
            Bytes::from(vec![0xAB; 16]),
        )
    }

    #[test]
    fn factory_returns_transcoder_for_video_track() {
        let factory = PassthroughTranscoderFactory::new(RenditionSpec::preset_720p());
        let ctx = ctx("0.mp4", factory.rendition().clone());
        assert!(factory.build(&ctx).is_some());
    }

    #[test]
    fn factory_skips_non_video_tracks() {
        let factory = PassthroughTranscoderFactory::new(RenditionSpec::preset_720p());
        for track in ["1.mp4", "captions", "catalog", "0-alt.mp4"] {
            let ctx = ctx(track, factory.rendition().clone());
            assert!(factory.build(&ctx).is_none(), "factory must skip track {track}");
        }
    }

    #[test]
    fn factory_name_is_stable_snake_case() {
        let factory = PassthroughTranscoderFactory::new(RenditionSpec::preset_480p());
        assert_eq!(factory.name(), "passthrough");
    }

    #[test]
    fn factory_exposes_configured_rendition() {
        let factory = PassthroughTranscoderFactory::new(RenditionSpec::preset_240p());
        assert_eq!(factory.rendition().name, "240p");
        assert_eq!(factory.rendition().width, 426);
    }

    #[test]
    fn transcoder_counts_each_fragment() {
        let mut t = PassthroughTranscoder::new(&RenditionSpec::preset_720p());
        let ctx = ctx("0.mp4", RenditionSpec::preset_720p());
        t.on_start(&ctx);
        for i in 0..5 {
            t.on_fragment(&frag(i));
        }
        assert_eq!(t.fragments_seen(), 5);
        t.on_stop();
    }
}
