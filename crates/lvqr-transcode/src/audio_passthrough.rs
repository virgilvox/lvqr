//! [`AudioPassthroughTranscoder`] + [`AudioPassthroughTranscoderFactory`].
//!
//! Sibling of [`crate::SoftwareTranscoder`] that covers the audio track for
//! the 4.6 ABR ladder. Unlike the software video encoder this factory
//! carries no GStreamer dependency and is always available: the source's
//! AAC `1.mp4` fragments are forwarded verbatim to
//! `<source>/<rendition>/1.mp4` so each rendition broadcaster is a
//! self-contained mp4 that LL-HLS composition can drain directly.
//!
//! Session 106 C ships this alongside the CLI wiring so every ladder rung
//! gets paired video (software-encoded) + audio (passthrough) output
//! broadcasts without the LL-HLS bridge having to special-case the
//! missing audio.

use std::sync::Arc;

use lvqr_fragment::{Fragment, FragmentBroadcaster, FragmentBroadcasterRegistry, FragmentMeta};
use tracing::{debug, info, warn};

use crate::rendition::RenditionSpec;
use crate::transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

/// Source track this factory targets. LVQR's audio-track convention is
/// `"1.mp4"` across every ingest protocol.
const SOURCE_TRACK: &str = "1.mp4";

/// Output track name on the rendition broadcaster. Kept identical to the
/// source track so the LL-HLS bridge's `ensure_audio` path picks it up
/// without any special-casing.
const OUTPUT_TRACK: &str = "1.mp4";

/// Factory that builds one [`AudioPassthroughTranscoder`] per source
/// audio track, republishing fragments onto `<source>/<rendition>/1.mp4`.
///
/// Ships without the `transcode` feature on purpose: the factory carries
/// no GStreamer dependency, so operators without the ladder build still
/// get audio passthrough when the CLI wiring installs it alongside the
/// software video encoder.
pub struct AudioPassthroughTranscoderFactory {
    rendition: RenditionSpec,
    output_registry: FragmentBroadcasterRegistry,
    skip_source_suffixes: Vec<String>,
}

impl AudioPassthroughTranscoderFactory {
    /// Build a factory for `rendition` that republishes source audio
    /// fragments into `output_registry` under `<source>/<rendition>/1.mp4`.
    pub fn new(rendition: RenditionSpec, output_registry: FragmentBroadcasterRegistry) -> Self {
        Self {
            rendition,
            output_registry,
            skip_source_suffixes: Vec::new(),
        }
    }

    /// Register additional trailing-component suffixes that the factory
    /// should treat as already-transcoded outputs and skip. Appends to the
    /// built-in `\d+p` heuristic; the default recursion guard (an all-
    /// digits + trailing `p` suffix like `720p`) stays in effect.
    ///
    /// Operators running custom rendition names (`ultra`, `low-motion`,
    /// etc.) pass them here so the factory does not rebuild transcoders
    /// on its own outputs.
    pub fn skip_source_suffixes(mut self, suffixes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.skip_source_suffixes.extend(suffixes.into_iter().map(Into::into));
        self
    }
}

impl TranscoderFactory for AudioPassthroughTranscoderFactory {
    fn name(&self) -> &str {
        "audio-passthrough"
    }

    fn rendition(&self) -> &RenditionSpec {
        &self.rendition
    }

    fn build(&self, ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>> {
        if ctx.track != SOURCE_TRACK {
            return None;
        }
        if looks_like_rendition_output(&ctx.broadcast, &self.skip_source_suffixes) {
            debug!(
                broadcast = %ctx.broadcast,
                rendition = %self.rendition.name,
                "AudioPassthroughTranscoderFactory: skipping already-transcoded broadcast",
            );
            return None;
        }
        Some(Box::new(AudioPassthroughTranscoder::new(
            self.rendition.clone(),
            ctx.broadcast.clone(),
            self.output_registry.clone(),
        )))
    }
}

/// Per-`(source, rendition)` audio passthrough. Forwards every source
/// `Fragment` verbatim to the rendition broadcaster.
pub struct AudioPassthroughTranscoder {
    rendition: RenditionSpec,
    source_broadcast: String,
    output_registry: FragmentBroadcasterRegistry,
    output_bc: Option<Arc<FragmentBroadcaster>>,
    forwarded: u64,
}

impl AudioPassthroughTranscoder {
    fn new(rendition: RenditionSpec, source_broadcast: String, output_registry: FragmentBroadcasterRegistry) -> Self {
        Self {
            rendition,
            source_broadcast,
            output_registry,
            output_bc: None,
            forwarded: 0,
        }
    }

    fn output_broadcast_name(&self) -> String {
        format!("{}/{}", self.source_broadcast, self.rendition.name)
    }

    /// Count of fragments forwarded to the output broadcaster since
    /// construction. Test-facing.
    pub fn forwarded(&self) -> u64 {
        self.forwarded
    }
}

impl Transcoder for AudioPassthroughTranscoder {
    fn on_start(&mut self, ctx: &TranscoderContext) {
        let output_name = self.output_broadcast_name();
        let output_meta = FragmentMeta {
            codec: ctx.meta.codec.clone(),
            timescale: ctx.meta.timescale,
            init_segment: ctx.meta.init_segment.clone(),
        };
        let bc = self
            .output_registry
            .get_or_create(&output_name, OUTPUT_TRACK, output_meta);
        if let Some(ref init) = ctx.meta.init_segment {
            bc.set_init_segment(init.clone());
        }
        info!(
            broadcast = %self.source_broadcast,
            output = %output_name,
            rendition = %self.rendition.name,
            codec = %ctx.meta.codec,
            timescale = ctx.meta.timescale,
            "AudioPassthroughTranscoder started",
        );
        self.output_bc = Some(bc);
    }

    fn on_fragment(&mut self, fragment: &Fragment) {
        let Some(bc) = self.output_bc.as_ref() else {
            warn!(
                rendition = %self.rendition.name,
                broadcast = %self.source_broadcast,
                "AudioPassthroughTranscoder: on_fragment before on_start; dropping",
            );
            return;
        };
        let clone = Fragment::new(
            OUTPUT_TRACK,
            fragment.group_id,
            fragment.object_id,
            fragment.priority,
            fragment.dts,
            fragment.pts,
            fragment.duration,
            fragment.flags,
            fragment.payload.clone(),
        );
        bc.emit(clone);
        self.forwarded = self.forwarded.saturating_add(1);
        metrics::counter!(
            "lvqr_transcode_output_fragments_total",
            "transcoder" => "audio-passthrough",
            "rendition" => self.rendition.name.clone(),
        )
        .increment(1);
        metrics::counter!(
            "lvqr_transcode_output_bytes_total",
            "transcoder" => "audio-passthrough",
            "rendition" => self.rendition.name.clone(),
        )
        .increment(fragment.payload.len() as u64);
    }

    fn on_stop(&mut self) {
        info!(
            broadcast = %self.source_broadcast,
            rendition = %self.rendition.name,
            forwarded = self.forwarded,
            "AudioPassthroughTranscoder stopped",
        );
        self.output_bc = None;
    }
}

/// `true` when `broadcast`'s trailing path component looks like a
/// rendition-output broadcast that this factory should skip. Matches the
/// built-in `\d+p` convention (`720p`, `480p`, `1080p`) OR any suffix in
/// `extra` (for operators running custom rendition names via
/// [`AudioPassthroughTranscoderFactory::skip_source_suffixes`]).
fn looks_like_rendition_output(broadcast: &str, extra: &[String]) -> bool {
    let Some(suffix) = broadcast.rsplit('/').next() else {
        return false;
    };
    if suffix.is_empty() {
        return false;
    }
    if extra.iter().any(|s| s == suffix) {
        return true;
    }
    if suffix.len() < 2 {
        return false;
    }
    let bytes = suffix.as_bytes();
    if *bytes.last().unwrap() != b'p' {
        return false;
    }
    bytes[..bytes.len() - 1].iter().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, FragmentStream};

    fn ctx(broadcast: &str, track: &str, rendition: RenditionSpec) -> TranscoderContext {
        TranscoderContext {
            broadcast: broadcast.into(),
            track: track.into(),
            meta: FragmentMeta::new("mp4a.40.2", 48_000),
            rendition,
        }
    }

    fn audio_frag(idx: u64, payload: &[u8]) -> Fragment {
        Fragment::new(
            "1.mp4",
            idx,
            0,
            0,
            idx * 1024,
            idx * 1024,
            1024,
            FragmentFlags::KEYFRAME,
            Bytes::copy_from_slice(payload),
        )
    }

    #[test]
    fn factory_opts_out_of_non_audio_tracks() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = AudioPassthroughTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
        for track in ["0.mp4", "captions", "catalog", ".catalog"] {
            let c = ctx("live/demo", track, factory.rendition().clone());
            assert!(
                factory.build(&c).is_none(),
                "factory must opt out of non-audio track {track}",
            );
        }
    }

    #[test]
    fn factory_builds_transcoder_for_audio_track() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = AudioPassthroughTranscoderFactory::new(RenditionSpec::preset_480p(), registry);
        let c = ctx("live/demo", "1.mp4", factory.rendition().clone());
        assert!(factory.build(&c).is_some());
    }

    #[test]
    fn factory_skips_already_transcoded_broadcast() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = AudioPassthroughTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
        for broadcast in ["live/demo/720p", "live/demo/480p", "cam/1080p"] {
            let c = ctx(broadcast, "1.mp4", factory.rendition().clone());
            assert!(
                factory.build(&c).is_none(),
                "factory must skip already-transcoded broadcast {broadcast}",
            );
        }
    }

    #[test]
    fn factory_honors_custom_skip_suffix() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = AudioPassthroughTranscoderFactory::new(RenditionSpec::preset_720p(), registry)
            .skip_source_suffixes(["ultra"]);
        let c = ctx("live/demo/ultra", "1.mp4", factory.rendition().clone());
        assert!(factory.build(&c).is_none());
        // The default `\d+p` heuristic still applies alongside the custom list.
        let c = ctx("live/demo/720p", "1.mp4", factory.rendition().clone());
        assert!(factory.build(&c).is_none());
        // A regular source broadcast still builds.
        let c = ctx("live/demo", "1.mp4", factory.rendition().clone());
        assert!(factory.build(&c).is_some());
    }

    #[test]
    fn transcoder_forwards_fragments_verbatim() {
        let registry = FragmentBroadcasterRegistry::new();
        let mut transcoder =
            AudioPassthroughTranscoder::new(RenditionSpec::preset_720p(), "live/demo".into(), registry.clone());
        let c = ctx("live/demo", "1.mp4", RenditionSpec::preset_720p());
        transcoder.on_start(&c);

        // Subscribe to the output broadcast AFTER on_start so the bc exists.
        let bc = registry
            .get("live/demo/720p", "1.mp4")
            .expect("audio output broadcaster must exist after on_start");
        let mut sub = bc.subscribe();

        let payloads: Vec<&[u8]> = vec![b"aac0", b"aac1xx", b"aac2xxxx"];
        for (i, p) in payloads.iter().enumerate() {
            transcoder.on_fragment(&audio_frag(i as u64, p));
        }

        // Poll the output; the runtime wraps next_fragment in a future, so
        // drive it via a tiny tokio current-thread runtime.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("runtime");
        rt.block_on(async {
            for (i, p) in payloads.iter().enumerate() {
                let got = tokio::time::timeout(std::time::Duration::from_millis(200), sub.next_fragment())
                    .await
                    .expect("timeout")
                    .expect("closed");
                assert_eq!(got.group_id, i as u64);
                assert_eq!(got.payload.as_ref(), *p);
                assert_eq!(got.track_id.as_str(), "1.mp4");
            }
        });
        assert_eq!(transcoder.forwarded(), 3);
        transcoder.on_stop();
    }

    #[test]
    fn transcoder_propagates_init_segment_to_output() {
        let registry = FragmentBroadcasterRegistry::new();
        let mut transcoder =
            AudioPassthroughTranscoder::new(RenditionSpec::preset_480p(), "live/demo".into(), registry.clone());
        let mut meta = FragmentMeta::new("mp4a.40.2", 48_000);
        meta.init_segment = Some(Bytes::from_static(b"AAC-ASC"));
        let c = TranscoderContext {
            broadcast: "live/demo".into(),
            track: "1.mp4".into(),
            meta,
            rendition: RenditionSpec::preset_480p(),
        };
        transcoder.on_start(&c);
        let bc = registry
            .get("live/demo/480p", "1.mp4")
            .expect("output broadcaster exists after on_start");
        let snapshot = bc.meta();
        assert_eq!(
            snapshot.init_segment.as_deref(),
            Some(b"AAC-ASC" as &[u8]),
            "passthrough must copy the source ASC onto the output broadcast",
        );
    }
}
