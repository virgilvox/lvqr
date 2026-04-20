//! [`WhisperCaptionsAgent`] -- the concrete `lvqr_agent::Agent`
//! implementation.
//!
//! Always available regardless of the `whisper` Cargo feature.
//! Without the feature `on_fragment` is a no-op and the agent
//! never emits captions; with the feature it forwards mdat-
//! extracted AAC frames to a worker thread that runs the
//! decoder and whisper inference.

use std::sync::Arc;

use bytes::Bytes;
use lvqr_agent::{Agent, AgentContext};
use lvqr_fragment::Fragment;
use tracing::{debug, warn};

use crate::caption::CaptionStream;
use crate::factory::WhisperConfig;
use crate::mdat::extract_first_mdat;

/// Bounded depth of the agent -> worker mpsc channel. Sized
/// for ~1 second of AAC frames at 48 kHz (1024 samples per
/// frame -> ~47 frames/sec); a slow worker drops frames into
/// `warn!` rather than back-pressuring the per-broadcast drain
/// task.
#[cfg(feature = "whisper")]
const WORKER_QUEUE_DEPTH: usize = 64;

/// Per-broadcast captions agent. One instance per
/// `(broadcast, "1.mp4")` pair built by
/// [`crate::WhisperCaptionsFactory`]. Each agent runs on its
/// own drain task on the tokio runtime that
/// `lvqr_agent::AgentRunner::install` was called from.
pub struct WhisperCaptionsAgent {
    config: Arc<WhisperConfig>,
    captions: CaptionStream,
    #[cfg(feature = "whisper")]
    worker: Option<crate::worker::WorkerHandle>,
    /// Asc + broadcast name captured at on_start so the worker
    /// (and the no-op variant's first-fragment log) can
    /// reference them without re-reading the registry.
    state: AgentState,
}

#[derive(Default)]
struct AgentState {
    broadcast: String,
    track: String,
    sample_rate: u32,
    asc: Option<Bytes>,
    /// True after we have logged the first fragment for this
    /// broadcast so the no-op variant does not flood the log.
    first_fragment_logged: bool,
}

impl WhisperCaptionsAgent {
    /// Build a fresh agent. Called from
    /// [`crate::WhisperCaptionsFactory::build`].
    pub fn new(config: Arc<WhisperConfig>, captions: CaptionStream) -> Self {
        Self {
            config,
            captions,
            #[cfg(feature = "whisper")]
            worker: None,
            state: AgentState::default(),
        }
    }
}

impl Agent for WhisperCaptionsAgent {
    fn on_start(&mut self, ctx: &AgentContext) {
        self.state.broadcast = ctx.broadcast.clone();
        self.state.track = ctx.track.clone();
        self.state.sample_rate = ctx.meta.timescale;
        self.state.asc = ctx.meta.init_segment.as_ref().and_then(crate::asc::extract_asc);

        debug!(
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            sample_rate = ctx.meta.timescale,
            asc_present = self.state.asc.is_some(),
            whisper_enabled = cfg!(feature = "whisper"),
            "WhisperCaptionsAgent on_start",
        );

        #[cfg(feature = "whisper")]
        {
            let Some(asc) = self.state.asc.clone() else {
                warn!(
                    broadcast = %ctx.broadcast,
                    "WhisperCaptionsAgent: init segment missing ASC; agent will no-op",
                );
                return;
            };
            match crate::worker::spawn(
                Arc::clone(&self.config),
                self.captions.clone(),
                ctx.broadcast.clone(),
                ctx.meta.timescale,
                asc,
                WORKER_QUEUE_DEPTH,
            ) {
                Ok(handle) => self.worker = Some(handle),
                Err(e) => warn!(
                    broadcast = %ctx.broadcast,
                    error = %e,
                    "WhisperCaptionsAgent: worker spawn failed; agent will no-op",
                ),
            }
        }
    }

    fn on_fragment(&mut self, fragment: &Fragment) {
        let Some(aac_frame) = extract_first_mdat(&fragment.payload) else {
            // A fragment with no mdat is a producer bug; log
            // once per broadcast and move on.
            if !self.state.first_fragment_logged {
                warn!(
                    broadcast = %self.state.broadcast,
                    track = %self.state.track,
                    "WhisperCaptionsAgent: fragment payload has no mdat box",
                );
                self.state.first_fragment_logged = true;
            }
            return;
        };

        #[cfg(feature = "whisper")]
        {
            if let Some(worker) = self.worker.as_ref() {
                worker.send_frame(fragment.dts, aac_frame);
            }
        }

        #[cfg(not(feature = "whisper"))]
        {
            if !self.state.first_fragment_logged {
                debug!(
                    broadcast = %self.state.broadcast,
                    aac_bytes = aac_frame.len(),
                    "WhisperCaptionsAgent: whisper feature off; AAC frames dropped",
                );
                self.state.first_fragment_logged = true;
            }
            // Hold a borrow on captions so the field is not
            // flagged as unused without the feature.
            let _ = self.captions.subscriber_count();
            // Suppress the "field never read" warning on
            // `config` when the feature is off; the field is
            // still needed because `WhisperCaptionsFactory`
            // hands it in.
            let _ = self.config.window_ms;
            let _ = aac_frame;
        }
    }

    fn on_stop(&mut self) {
        debug!(
            broadcast = %self.state.broadcast,
            track = %self.state.track,
            "WhisperCaptionsAgent on_stop",
        );

        #[cfg(feature = "whisper")]
        {
            if let Some(handle) = self.worker.take() {
                handle.shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;
    use lvqr_fragment::{FragmentFlags, FragmentMeta};
    use std::path::PathBuf;

    fn ctx(track: &str) -> AgentContext {
        AgentContext {
            broadcast: "live/cam1".into(),
            track: track.into(),
            meta: FragmentMeta::new("mp4a.40.2", 44_100),
        }
    }

    fn agent() -> WhisperCaptionsAgent {
        let cfg = Arc::new(WhisperConfig::new(PathBuf::from("/nonexistent/ggml-tiny.en.bin")));
        WhisperCaptionsAgent::new(cfg, CaptionStream::new())
    }

    fn fragment_with_mdat(payload: &[u8]) -> Fragment {
        let mut buf = BytesMut::new();
        // moof (opaque body)
        let moof_body = b"opaque";
        buf.extend_from_slice(&((8 + moof_body.len()) as u32).to_be_bytes());
        buf.extend_from_slice(b"moof");
        buf.extend_from_slice(moof_body);
        // mdat
        buf.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
        buf.extend_from_slice(b"mdat");
        buf.extend_from_slice(payload);

        Fragment::new(
            "1.mp4",
            1,
            0,
            0,
            1024,
            1024,
            1024,
            FragmentFlags::KEYFRAME,
            buf.freeze(),
        )
    }

    #[test]
    fn on_fragment_without_whisper_feature_is_a_no_op() {
        let mut a = agent();
        a.on_start(&ctx("1.mp4"));
        // Pass a fragment with valid mdat; no panic, no caption
        // (no subscriber to assert against), no state mutation
        // beyond the first-fragment-logged flag.
        let frag = fragment_with_mdat(&[0x21, 0x12, 0x34]);
        a.on_fragment(&frag);
        a.on_fragment(&frag); // second call must not re-log
        a.on_stop();
    }

    #[test]
    fn on_fragment_with_no_mdat_is_a_no_op_and_logs_once() {
        let mut a = agent();
        a.on_start(&ctx("1.mp4"));
        let frag = Fragment::new(
            "1.mp4",
            1,
            0,
            0,
            0,
            0,
            0,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(b"this is not BMFF"),
        );
        a.on_fragment(&frag);
        a.on_fragment(&frag);
        // The state assertion is "did not panic"; the
        // first_fragment_logged flag prevents log spam in
        // production but is not user-visible here.
    }

    #[test]
    fn on_start_reads_sample_rate_from_meta() {
        let mut a = agent();
        let mut c = ctx("1.mp4");
        c.meta = FragmentMeta::new("mp4a.40.2", 48_000);
        a.on_start(&c);
        assert_eq!(a.state.sample_rate, 48_000);
        assert_eq!(a.state.broadcast, "live/cam1");
    }

    #[test]
    fn on_start_handles_missing_init_segment_gracefully() {
        let mut a = agent();
        // ctx.meta.init_segment is None by default.
        a.on_start(&ctx("1.mp4"));
        assert!(a.state.asc.is_none());
        // on_fragment must still not panic.
        let frag = fragment_with_mdat(&[0xAA, 0xBB]);
        a.on_fragment(&frag);
        a.on_stop();
    }
}
