//! [`WhisperCaptionsFactory`] + [`WhisperConfig`].
//!
//! Always available regardless of the `whisper` Cargo feature.
//! Without the feature `build()` still returns `Some(agent)` for
//! audio tracks; the agent's `on_fragment` then no-ops with a
//! single tracing line per broadcast (the agent contract still
//! holds; just no inference).

use std::path::PathBuf;
use std::sync::Arc;

use lvqr_agent::{Agent, AgentContext, AgentFactory};
use lvqr_fragment::FragmentBroadcasterRegistry;

use crate::agent::WhisperCaptionsAgent;
use crate::caption::CaptionStream;

/// Track-id convention LVQR uses for the audio track on every
/// `(broadcast, track)` pair the ingest emits. Matches
/// `lvqr_ingest::bridge`'s `audio_track` constant.
const AUDIO_TRACK_ID: &str = "1.mp4";

/// Track-id LVQR uses for the captions track on the shared
/// `FragmentBroadcasterRegistry`. Tier 4 item 4.5 session C
/// adopts this so the same `(broadcast, track)` keying that
/// powers HLS / archive / WASM consumers also fans out to the
/// LL-HLS subtitle rendition.
pub const CAPTIONS_TRACK_ID: &str = "captions";

/// Static config for the whisper agent.
///
/// `model_path` is the on-disk path to a `ggml-*.bin` whisper.cpp
/// model file. Cloned per-broadcast at factory build time so a
/// single factory installation can serve many concurrent
/// broadcasts without re-resolving the path. Without the
/// `whisper` Cargo feature `model_path` is held but unused.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Path to a whisper.cpp `ggml-*.bin` model file.
    pub model_path: PathBuf,

    /// Window of audio (in milliseconds) to buffer before
    /// running each whisper inference pass. Smaller -> lower
    /// latency captions but lower accuracy and more CPU.
    /// Defaults to 5000.
    pub window_ms: u32,
}

impl WhisperConfig {
    /// New config with the default 5-second window.
    pub fn new(model_path: impl Into<PathBuf>) -> Self {
        Self {
            model_path: model_path.into(),
            window_ms: 5_000,
        }
    }

    /// Override the inference window in milliseconds.
    pub fn with_window_ms(mut self, window_ms: u32) -> Self {
        self.window_ms = window_ms;
        self
    }
}

/// Factory that builds one [`WhisperCaptionsAgent`] per
/// audio track on the shared
/// [`lvqr_fragment::FragmentBroadcasterRegistry`]. Returns
/// `None` for video / catalog / future captions tracks --
/// only the audio track gets transcribed.
///
/// Cheaply cloneable: the inner config is `Arc`'d so all
/// per-broadcast agents share the model path + window. The
/// captions output channel is also `Arc`'d so downstream
/// consumers (session 99 C's HLS subtitle wiring) can grab
/// one [`CaptionStream::subscribe`] handle and see every
/// caption from every broadcast.
#[derive(Clone)]
pub struct WhisperCaptionsFactory {
    config: Arc<WhisperConfig>,
    captions: CaptionStream,
    /// Optional shared registry the agent will additionally
    /// publish each `TranscribedCaption` into under track id
    /// [`CAPTIONS_TRACK_ID`]. Tier 4 item 4.5 session C wires
    /// this so the LL-HLS subtitle rendition can drain the
    /// captions through the same `on_entry_created` callback
    /// pattern every other LVQR consumer uses. Without it the
    /// agent only feeds the in-process [`CaptionStream`].
    caption_registry: Option<FragmentBroadcasterRegistry>,
}

impl WhisperCaptionsFactory {
    /// Construct a new factory with a freshly created
    /// [`CaptionStream`]. Use [`WhisperCaptionsFactory::captions`]
    /// to get a clone of the stream and subscribe before the
    /// factory is installed on an `AgentRunner`.
    pub fn new(config: WhisperConfig) -> Self {
        Self {
            config: Arc::new(config),
            captions: CaptionStream::new(),
            caption_registry: None,
        }
    }

    /// Construct with a pre-built captions stream. Useful when
    /// the caller wants to subscribe to a single stream that
    /// outlives the factory or that is shared with another
    /// factory.
    pub fn with_caption_stream(config: WhisperConfig, captions: CaptionStream) -> Self {
        Self {
            config: Arc::new(config),
            captions,
            caption_registry: None,
        }
    }

    /// Wire a shared [`FragmentBroadcasterRegistry`] so the
    /// agent additionally publishes each
    /// [`crate::TranscribedCaption`] into the registry under
    /// track id [`CAPTIONS_TRACK_ID`]. Returns `self` for
    /// chaining. Tier 4 item 4.5 session C: pair this with
    /// `lvqr_cli::captions::BroadcasterCaptionsBridge` so the
    /// LL-HLS subtitle rendition is fed by the same registry
    /// callback the audio + video renditions drink from.
    ///
    /// Without this builder the factory still works -- it
    /// fans out captions only to the in-process
    /// [`CaptionStream`].
    pub fn with_caption_registry(mut self, registry: FragmentBroadcasterRegistry) -> Self {
        self.caption_registry = Some(registry);
        self
    }

    /// Cloneable handle to the captions output channel. Subscribe
    /// to receive every [`crate::TranscribedCaption`] the agents
    /// emit.
    pub fn captions(&self) -> CaptionStream {
        self.captions.clone()
    }

    /// Cloneable handle to the optional captions registry, when
    /// installed via [`Self::with_caption_registry`].
    pub fn caption_registry(&self) -> Option<&FragmentBroadcasterRegistry> {
        self.caption_registry.as_ref()
    }

    /// Read access to the shared config.
    pub fn config(&self) -> &WhisperConfig {
        &self.config
    }
}

impl AgentFactory for WhisperCaptionsFactory {
    fn name(&self) -> &str {
        "captions"
    }

    fn build(&self, ctx: &AgentContext) -> Option<Box<dyn Agent>> {
        if ctx.track != AUDIO_TRACK_ID {
            return None;
        }
        Some(Box::new(WhisperCaptionsAgent::new(
            Arc::clone(&self.config),
            self.captions.clone(),
            self.caption_registry.clone(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_fragment::FragmentMeta;

    fn ctx(track: &str) -> AgentContext {
        AgentContext {
            broadcast: "live/cam1".into(),
            track: track.into(),
            meta: FragmentMeta::new("mp4a.40.2", 44_100),
        }
    }

    fn config() -> WhisperConfig {
        WhisperConfig::new("/nonexistent/ggml-tiny.en.bin")
    }

    #[test]
    fn build_returns_some_for_audio_track() {
        let f = WhisperCaptionsFactory::new(config());
        assert!(f.build(&ctx("1.mp4")).is_some());
    }

    #[test]
    fn build_returns_none_for_video_track() {
        let f = WhisperCaptionsFactory::new(config());
        assert!(f.build(&ctx("0.mp4")).is_none());
    }

    #[test]
    fn build_returns_none_for_catalog_or_other_tracks() {
        let f = WhisperCaptionsFactory::new(config());
        assert!(f.build(&ctx("catalog")).is_none());
        assert!(f.build(&ctx("captions")).is_none());
        assert!(f.build(&ctx("99.mp4")).is_none());
    }

    #[test]
    fn name_is_captions_for_metric_label_consistency() {
        let f = WhisperCaptionsFactory::new(config());
        assert_eq!(f.name(), "captions");
    }

    #[test]
    fn config_window_ms_defaults_to_5_seconds() {
        let c = WhisperConfig::new("model.bin");
        assert_eq!(c.window_ms, 5_000);
        let custom = WhisperConfig::new("model.bin").with_window_ms(2_500);
        assert_eq!(custom.window_ms, 2_500);
    }

    #[test]
    fn captions_handle_clone_shares_underlying_sender() {
        let f = WhisperCaptionsFactory::new(config());
        let a = f.captions();
        let _sub = a.subscribe();
        let b = f.captions();
        assert_eq!(b.subscriber_count(), 1, "captions handle is shared");
    }
}
