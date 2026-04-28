//! [`VideoToolboxTranscoder`] + [`VideoToolboxTranscoderFactory`].
//!
//! Hardware-encoder backend v1 (Tier 4 item 4.6 session 156). Mirrors
//! the [`crate::software`] module shape -- same trait, same lifecycle,
//! same output broadcast naming convention -- but swaps the GStreamer
//! `x264enc` element for `vtenc_h264_hw` (Apple's VideoToolbox
//! HW-only H.264 encoder). The HW-only path is intentional: a HW
//! factory that silently falls back to CPU encoding under load defeats
//! the purpose of an operator-pickable hardware tier.
//!
//! Pipeline shape (feature-on path):
//!
//! ```text
//! appsrc name=src caps=video/quicktime
//!   ! qtdemux
//!   ! h264parse
//!   ! avdec_h264
//!   ! videoscale
//!   ! video/x-raw,width=<W>,height=<H>
//!   ! videoconvert
//!   ! vtenc_h264_hw bitrate=<kbps> realtime=true allow-frame-reordering=false max-keyframe-interval=60
//!   ! h264parse
//!   ! mp4mux streamable=true fragment-duration=2000
//!   ! appsink name=sink emit-signals=true
//! ```
//!
//! Worker thread pattern matches [`crate::software`] verbatim: one
//! OS thread per `(source, rendition)` pair owning the whole pipeline,
//! a bounded mpsc carrying `Bytes` from `on_fragment`, EOS-on-drop +
//! 5 s drain timeout on `on_stop`.
//!
//! Only the `"0.mp4"` video track is accepted; the existing
//! [`crate::AudioPassthroughTranscoderFactory`] (106 C) handles the
//! `"1.mp4"` audio sibling.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;

use bytes::Bytes;
use glib::object::Cast;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use lvqr_fragment::{Fragment, FragmentBroadcaster, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use tracing::{debug, info, warn};

use crate::rendition::RenditionSpec;
use crate::transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

/// Source track the VideoToolbox transcoder accepts. Video only;
/// the audio passthrough sibling owns `"1.mp4"`.
const DEFAULT_SOURCE_TRACK: &str = "0.mp4";

/// GStreamer elements required by the VideoToolbox pipeline. Probed
/// at factory construction; if any is missing the factory opts out
/// of every stream with a warn log. `vtenc_h264_hw` lives in the
/// `applemedia` plugin from `gst-plugins-bad`; the rest are shared
/// with [`crate::software::SoftwareTranscoderFactory`].
const REQUIRED_ELEMENTS: &[&str] = &[
    "appsrc",
    "qtdemux",
    "h264parse",
    "avdec_h264",
    "videoscale",
    "videoconvert",
    "vtenc_h264_hw",
    "mp4mux",
    "appsink",
];

/// Stable identifier returned from [`TranscoderFactory::name`]. Used
/// in metric labels (`lvqr_transcode_*_total{transcoder="videotoolbox"}`)
/// and worker thread names.
const FACTORY_NAME: &str = "videotoolbox";

/// Bounded mpsc depth between `on_fragment` and the worker. Matches
/// [`crate::software`].
const WORKER_QUEUE_DEPTH: usize = 64;

/// Shutdown grace for pipeline drain after EOS. Matches
/// [`crate::software`].
const SHUTDOWN_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

/// Output codec string for the rendition's `FragmentMeta`. VT's
/// `vtenc_h264_hw` emits a Main / High profile bitstream depending
/// on the source; downstream consumers that need the exact profile
/// parse the init segment bytes directly from
/// [`FragmentBroadcaster::meta`]. Matches [`crate::software::OUTPUT_CODEC`].
const OUTPUT_CODEC: &str = "avc1.640028";

/// Output track-name + timescale. Matches LVQR's `"0.mp4"` / 90 kHz
/// video convention; the HLS bridge and every MoQ consumer assume
/// this pairing.
const OUTPUT_TRACK: &str = "0.mp4";
const OUTPUT_TIMESCALE: u32 = 90_000;

/// Factory that builds [`VideoToolboxTranscoder`] instances for the
/// `"0.mp4"` video track of whatever source broadcast the registry
/// hands it. Mirrors [`crate::SoftwareTranscoderFactory`] but uses
/// VideoToolbox HW-only encoding.
///
/// One factory instance per rendition. The CLI's
/// `--transcode-encoder videotoolbox` switch installs three of these
/// (one per default-ladder rung) instead of [`crate::SoftwareTranscoderFactory`].
pub struct VideoToolboxTranscoderFactory {
    rendition: RenditionSpec,
    output_registry: FragmentBroadcasterRegistry,
    missing_elements: Vec<&'static str>,
    skip_source_suffixes: Vec<String>,
}

impl VideoToolboxTranscoderFactory {
    /// Construct a factory for `rendition` that publishes output fragments
    /// into `output_registry` under `<source>/<rendition>` broadcasts.
    ///
    /// `gst::init()` is called here (idempotent across threads) and the
    /// required plugin list is probed. Missing elements are logged once
    /// at construction and cause every subsequent `build(ctx)` call to
    /// return `None`; matches the factory opt-out idiom the runner
    /// already uses.
    pub fn new(rendition: RenditionSpec, output_registry: FragmentBroadcasterRegistry) -> Self {
        let missing_elements = match gst::init() {
            Ok(()) => missing_required_elements(),
            Err(err) => {
                warn!(
                    rendition = %rendition.name,
                    error = %err,
                    "VideoToolboxTranscoderFactory: gst::init() failed",
                );
                REQUIRED_ELEMENTS.to_vec()
            }
        };
        if !missing_elements.is_empty() {
            warn!(
                rendition = %rendition.name,
                missing = ?missing_elements,
                "VideoToolboxTranscoderFactory: required GStreamer elements absent; factory will opt out of every build()",
            );
        }
        Self {
            rendition,
            output_registry,
            missing_elements,
            skip_source_suffixes: Vec::new(),
        }
    }

    /// Register additional trailing-component suffixes the factory
    /// should treat as already-transcoded outputs and skip. Same
    /// semantics as [`crate::SoftwareTranscoderFactory::skip_source_suffixes`].
    pub fn skip_source_suffixes(mut self, suffixes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.skip_source_suffixes.extend(suffixes.into_iter().map(Into::into));
        self
    }

    /// `true` when every required GStreamer element was found at
    /// factory construction. When `false`, `build()` opts out of
    /// every `(broadcast, track)` with a `None` return.
    pub fn is_available(&self) -> bool {
        self.missing_elements.is_empty()
    }

    /// Snapshot of required GStreamer elements not found on the host.
    pub fn missing_elements(&self) -> &[&'static str] {
        &self.missing_elements
    }
}

impl TranscoderFactory for VideoToolboxTranscoderFactory {
    fn name(&self) -> &str {
        FACTORY_NAME
    }

    fn rendition(&self) -> &RenditionSpec {
        &self.rendition
    }

    fn build(&self, ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>> {
        if !self.is_available() {
            return None;
        }
        if ctx.track != DEFAULT_SOURCE_TRACK {
            return None;
        }
        if looks_like_rendition_output(&ctx.broadcast, &self.skip_source_suffixes) {
            debug!(
                broadcast = %ctx.broadcast,
                rendition = %self.rendition.name,
                "VideoToolboxTranscoderFactory: skipping already-transcoded broadcast",
            );
            return None;
        }
        Some(Box::new(VideoToolboxTranscoder::new(
            ctx.rendition.clone(),
            ctx.broadcast.clone(),
            self.output_registry.clone(),
            ctx.meta.init_segment.clone(),
        )))
    }
}

/// Per-`(source, rendition)` VideoToolbox transcoder. Thin shell
/// around [`WorkerHandle`]; all heavy work happens on the worker
/// thread.
pub struct VideoToolboxTranscoder {
    rendition: RenditionSpec,
    source_broadcast: String,
    output_registry: FragmentBroadcasterRegistry,
    initial_header: Option<Bytes>,
    worker: Option<WorkerHandle>,
    dropped_fragments: Arc<AtomicU64>,
}

impl VideoToolboxTranscoder {
    fn new(
        rendition: RenditionSpec,
        source_broadcast: String,
        output_registry: FragmentBroadcasterRegistry,
        initial_header: Option<Bytes>,
    ) -> Self {
        Self {
            rendition,
            source_broadcast,
            output_registry,
            initial_header,
            worker: None,
            dropped_fragments: Arc::new(AtomicU64::new(0)),
        }
    }

    fn output_broadcast_name(&self) -> String {
        format!("{}/{}", self.source_broadcast, self.rendition.name)
    }

    /// Fragments dropped on a full worker channel. Test-facing hook.
    pub fn dropped_fragments(&self) -> u64 {
        self.dropped_fragments.load(Ordering::Relaxed)
    }
}

impl Transcoder for VideoToolboxTranscoder {
    fn on_start(&mut self, ctx: &TranscoderContext) {
        if self.worker.is_some() {
            return;
        }
        let output_name = self.output_broadcast_name();
        let output_bc = self.output_registry.get_or_create(
            &output_name,
            OUTPUT_TRACK,
            FragmentMeta::new(OUTPUT_CODEC, OUTPUT_TIMESCALE),
        );
        let initial_header = self.initial_header.clone().or_else(|| ctx.meta.init_segment.clone());
        match WorkerHandle::spawn(WorkerSpawnArgs {
            rendition: self.rendition.clone(),
            source_broadcast: self.source_broadcast.clone(),
            output_broadcast: output_name.clone(),
            output_bc,
            initial_header,
            dropped_counter: Arc::clone(&self.dropped_fragments),
        }) {
            Ok(handle) => {
                info!(
                    broadcast = %self.source_broadcast,
                    output = %output_name,
                    rendition = %self.rendition.name,
                    width = self.rendition.width,
                    height = self.rendition.height,
                    kbps = self.rendition.video_bitrate_kbps,
                    "VideoToolboxTranscoder: worker spawned",
                );
                self.worker = Some(handle);
            }
            Err(err) => {
                warn!(
                    broadcast = %self.source_broadcast,
                    rendition = %self.rendition.name,
                    error = %err,
                    "VideoToolboxTranscoder: failed to spawn worker; transcoder will drop every fragment",
                );
            }
        }
    }

    fn on_fragment(&mut self, fragment: &Fragment) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        worker.push(fragment.payload.clone());
    }

    fn on_stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
}

impl Drop for VideoToolboxTranscoder {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
}

struct WorkerSpawnArgs {
    rendition: RenditionSpec,
    source_broadcast: String,
    output_broadcast: String,
    output_bc: Arc<FragmentBroadcaster>,
    initial_header: Option<Bytes>,
    dropped_counter: Arc<AtomicU64>,
}

struct WorkerHandle {
    tx: Option<SyncSender<Bytes>>,
    join: Option<thread::JoinHandle<()>>,
    source_broadcast: String,
    rendition: String,
    dropped_counter: Arc<AtomicU64>,
}

impl WorkerHandle {
    fn spawn(args: WorkerSpawnArgs) -> Result<Self, WorkerSpawnError> {
        let WorkerSpawnArgs {
            rendition,
            source_broadcast,
            output_broadcast,
            output_bc,
            initial_header,
            dropped_counter,
        } = args;

        let pipeline = build_pipeline(&rendition)?;
        attach_output_callback(&pipeline.appsink, &output_bc, &rendition.name);

        let (tx, rx) = sync_channel::<Bytes>(WORKER_QUEUE_DEPTH);
        let dropped_for_thread = Arc::clone(&dropped_counter);
        let rendition_for_thread = rendition.clone();
        let source_for_thread = source_broadcast.clone();
        let output_for_thread = output_broadcast.clone();

        let join = thread::Builder::new()
            .name(format!("lvqr-transcode-vt:{source_broadcast}:{}", rendition.name))
            .spawn(move || {
                run_worker(
                    pipeline,
                    initial_header,
                    rx,
                    dropped_for_thread,
                    rendition_for_thread,
                    source_for_thread,
                    output_for_thread,
                );
            })
            .map_err(|e| WorkerSpawnError::ThreadSpawn(e.to_string()))?;

        Ok(Self {
            tx: Some(tx),
            join: Some(join),
            source_broadcast,
            rendition: rendition.name.clone(),
            dropped_counter,
        })
    }

    fn push(&self, bytes: Bytes) {
        let Some(tx) = self.tx.as_ref() else {
            return;
        };
        match tx.try_send(bytes) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped_counter.fetch_add(1, Ordering::Relaxed);
                metrics::counter!(
                    "lvqr_transcode_dropped_fragments_total",
                    "transcoder" => FACTORY_NAME,
                    "rendition" => self.rendition.clone(),
                )
                .increment(1);
                warn!(
                    broadcast = %self.source_broadcast,
                    rendition = %self.rendition,
                    "VideoToolboxTranscoder: worker channel full; dropping source fragment",
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                debug!(
                    broadcast = %self.source_broadcast,
                    rendition = %self.rendition,
                    "VideoToolboxTranscoder: worker already shut down; fragment discarded",
                );
            }
        }
    }

    fn shutdown(mut self) {
        drop(self.tx.take());
        if let Some(join) = self.join.take()
            && let Err(panic_payload) = join.join()
        {
            warn!(
                broadcast = %self.source_broadcast,
                rendition = %self.rendition,
                payload = ?panic_payload,
                "VideoToolboxTranscoder: worker thread panicked during shutdown",
            );
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum WorkerSpawnError {
    #[error("gstreamer pipeline parse or downcast failed: {0}")]
    Pipeline(String),

    #[error("worker thread spawn failed: {0}")]
    ThreadSpawn(String),
}

struct BuiltPipeline {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
}

/// Build the GStreamer pipeline string for a VideoToolbox ladder
/// rung. Extracted from [`build_pipeline`] so the test suite can
/// call it directly; the runtime path always goes through
/// `build_pipeline`.
///
/// VideoToolbox property mapping (vs the x264enc software path):
/// * `bitrate=<kbps>` -- same units
/// * `realtime=true` -- replaces `tune=zerolatency`
/// * `allow-frame-reordering=false` -- no B-frames; matches the
///   zerolatency intent
/// * `max-keyframe-interval=60` -- replaces `key-int-max`
///
/// No threads property: VT uses the AVFoundation framework's worker
/// pool internally and ignores Pipeline-level thread hints, unlike
/// x264enc which has its own pthread pool.
fn pipeline_str_for(rendition: &RenditionSpec) -> String {
    format!(
        "appsrc name=src caps=video/quicktime is-live=false format=time \
         ! qtdemux \
         ! h264parse \
         ! avdec_h264 \
         ! videoscale \
         ! video/x-raw,width={w},height={h} \
         ! videoconvert \
         ! vtenc_h264_hw bitrate={kbps} realtime=true allow-frame-reordering=false max-keyframe-interval=60 \
         ! h264parse \
         ! mp4mux streamable=true fragment-duration=2000 \
         ! appsink name=sink emit-signals=true sync=false",
        w = rendition.width,
        h = rendition.height,
        kbps = rendition.video_bitrate_kbps,
    )
}

fn build_pipeline(rendition: &RenditionSpec) -> Result<BuiltPipeline, WorkerSpawnError> {
    let pipeline_str = pipeline_str_for(rendition);
    let element = gst::parse::launch(&pipeline_str).map_err(|e| WorkerSpawnError::Pipeline(e.to_string()))?;
    let pipeline = element
        .downcast::<gst::Pipeline>()
        .map_err(|_| WorkerSpawnError::Pipeline("parse_launch result is not a pipeline".into()))?;

    let appsrc_elem = pipeline
        .by_name("src")
        .ok_or_else(|| WorkerSpawnError::Pipeline("appsrc element 'src' not found".into()))?;
    let appsrc = appsrc_elem
        .downcast::<gst_app::AppSrc>()
        .map_err(|_| WorkerSpawnError::Pipeline("'src' is not an AppSrc".into()))?;
    appsrc.set_max_bytes(4 * 1024 * 1024);
    appsrc.set_property("block", true);

    let appsink_elem = pipeline
        .by_name("sink")
        .ok_or_else(|| WorkerSpawnError::Pipeline("appsink element 'sink' not found".into()))?;
    let appsink = appsink_elem
        .downcast::<gst_app::AppSink>()
        .map_err(|_| WorkerSpawnError::Pipeline("'sink' is not an AppSink".into()))?;
    appsink.set_sync(false);

    Ok(BuiltPipeline {
        pipeline,
        appsrc,
        appsink,
    })
}

fn attach_output_callback(appsink: &gst_app::AppSink, output_bc: &Arc<FragmentBroadcaster>, rendition_name: &str) {
    let bc_for_cb = Arc::clone(output_bc);
    let rendition_for_cb = rendition_name.to_string();
    let group_counter = Arc::new(AtomicU64::new(0));

    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = match sink.pull_sample() {
                    Ok(s) => s,
                    Err(_) => return Err(gst::FlowError::Eos),
                };
                let Some(buffer) = sample.buffer() else {
                    return Err(gst::FlowError::Error);
                };
                let is_header = buffer.flags().contains(gst::BufferFlags::HEADER);
                let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;
                let payload = Bytes::copy_from_slice(map.as_slice());
                drop(map);

                if is_header {
                    debug!(
                        rendition = %rendition_for_cb,
                        bytes = payload.len(),
                        "VideoToolboxTranscoder: output header buffer received; caching as init_segment",
                    );
                    bc_for_cb.set_init_segment(payload);
                } else {
                    let group_id = group_counter.fetch_add(1, Ordering::Relaxed);
                    let pts_ns = buffer.pts().map(|t| t.nseconds()).unwrap_or(0);
                    let dts_ns = buffer.dts().map(|t| t.nseconds()).unwrap_or(pts_ns);
                    let dur_ns = buffer.duration().map(|t| t.nseconds()).unwrap_or(0);
                    let frag = Fragment::new(
                        OUTPUT_TRACK,
                        group_id,
                        0,
                        0,
                        ns_to_ticks(dts_ns, OUTPUT_TIMESCALE),
                        ns_to_ticks(pts_ns, OUTPUT_TIMESCALE),
                        ns_to_ticks(dur_ns, OUTPUT_TIMESCALE),
                        FragmentFlags::KEYFRAME,
                        payload.clone(),
                    );
                    bc_for_cb.emit(frag);
                    metrics::counter!(
                        "lvqr_transcode_output_fragments_total",
                        "transcoder" => FACTORY_NAME,
                        "rendition" => rendition_for_cb.clone(),
                    )
                    .increment(1);
                    metrics::counter!(
                        "lvqr_transcode_output_bytes_total",
                        "transcoder" => FACTORY_NAME,
                        "rendition" => rendition_for_cb.clone(),
                    )
                    .increment(payload.len() as u64);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
}

fn run_worker(
    built: BuiltPipeline,
    initial_header: Option<Bytes>,
    rx: Receiver<Bytes>,
    dropped_counter: Arc<AtomicU64>,
    rendition: RenditionSpec,
    source_broadcast: String,
    output_broadcast: String,
) {
    let BuiltPipeline {
        pipeline,
        appsrc,
        appsink: _,
    } = built;

    if let Err(err) = pipeline.set_state(gst::State::Playing) {
        warn!(
            broadcast = %source_broadcast,
            rendition = %rendition.name,
            error = %err,
            "VideoToolboxTranscoder: failed to set pipeline to Playing",
        );
        let _ = pipeline.set_state(gst::State::Null);
        return;
    }

    if let Some(header) = initial_header {
        push_buffer(&appsrc, header, true);
    }

    while let Ok(bytes) = rx.recv() {
        if !push_buffer(&appsrc, bytes, false) {
            break;
        }
    }

    if let Err(err) = appsrc.end_of_stream() {
        warn!(
            broadcast = %source_broadcast,
            rendition = %rendition.name,
            error = %err,
            "VideoToolboxTranscoder: end_of_stream signal failed",
        );
    }

    wait_for_drain(&pipeline, &source_broadcast, &rendition.name);

    if let Err(err) = pipeline.set_state(gst::State::Null) {
        warn!(
            broadcast = %source_broadcast,
            rendition = %rendition.name,
            error = %err,
            "VideoToolboxTranscoder: failed to set pipeline to Null",
        );
    }

    info!(
        broadcast = %source_broadcast,
        output = %output_broadcast,
        rendition = %rendition.name,
        dropped = dropped_counter.load(Ordering::Relaxed),
        "VideoToolboxTranscoder: worker exited",
    );
}

fn push_buffer(appsrc: &gst_app::AppSrc, bytes: Bytes, is_header: bool) -> bool {
    let mut buffer = gst::Buffer::from_slice(bytes);
    if is_header && let Some(buf_ref) = buffer.get_mut() {
        buf_ref.set_flags(gst::BufferFlags::HEADER);
    }
    match appsrc.push_buffer(buffer) {
        Ok(_) => true,
        Err(gst::FlowError::Flushing) | Err(gst::FlowError::Eos) => false,
        Err(err) => {
            warn!(error = ?err, "VideoToolboxTranscoder: appsrc.push_buffer failed");
            false
        }
    }
}

fn wait_for_drain(pipeline: &gst::Pipeline, source_broadcast: &str, rendition: &str) {
    let Some(bus) = pipeline.bus() else {
        return;
    };
    let types = [gst::MessageType::Eos, gst::MessageType::Error];
    let Some(msg) = bus.timed_pop_filtered(Some(SHUTDOWN_TIMEOUT), &types) else {
        warn!(
            broadcast = source_broadcast,
            rendition,
            timeout_s = SHUTDOWN_TIMEOUT.seconds(),
            "VideoToolboxTranscoder: pipeline did not drain within timeout; forcing Null",
        );
        return;
    };
    if let gst::MessageView::Error(err) = msg.view() {
        warn!(
            broadcast = source_broadcast,
            rendition,
            error = %err.error(),
            debug = ?err.debug(),
            "VideoToolboxTranscoder: pipeline reported error during drain",
        );
    }
}

fn missing_required_elements() -> Vec<&'static str> {
    REQUIRED_ELEMENTS
        .iter()
        .copied()
        .filter(|name| gst::ElementFactory::find(name).is_none())
        .collect()
}

/// Convert a nanosecond duration into a tick count at `timescale` Hz.
/// Saturating, truncating; identical semantics to
/// [`crate::software`]'s helper.
fn ns_to_ticks(ns: u64, timescale: u32) -> u64 {
    ns.saturating_mul(timescale as u64) / 1_000_000_000u64
}

/// `true` when `broadcast`'s trailing path component matches the
/// conventional `<digits>p` rendition marker OR appears in `extra`.
/// Identical semantics to [`crate::software`]'s helper.
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

    #[test]
    fn pipeline_str_uses_vtenc_h264_hw_with_documented_property_mapping() {
        let rendition = RenditionSpec {
            name: "test720p".into(),
            width: 1280,
            height: 720,
            video_bitrate_kbps: 2_500,
            audio_bitrate_kbps: 128,
        };
        let s = pipeline_str_for(&rendition);

        // Right encoder element for THIS backend. Catches a
        // copy-paste from another HW backend that forgot to swap
        // the encoder element name.
        assert!(s.contains("vtenc_h264_hw"), "must use vtenc_h264_hw; got: {s}");
        assert!(!s.contains("nvh264enc"), "must not use nvenc encoder; got: {s}");
        assert!(!s.contains("vah264enc"), "must not use vaapi encoder; got: {s}");
        assert!(!s.contains("qsvh264enc"), "must not use qsv encoder; got: {s}");
        assert!(!s.contains("x264enc"), "must not use software encoder; got: {s}");

        // VideoToolbox property mapping per module docs.
        assert!(s.contains("bitrate=2500"), "bitrate substitution: {s}");
        assert!(s.contains("realtime=true"), "realtime property: {s}");
        assert!(
            s.contains("allow-frame-reordering=false"),
            "allow-frame-reordering property: {s}"
        );
        assert!(
            s.contains("max-keyframe-interval=60"),
            "max-keyframe-interval property: {s}"
        );

        assert!(s.contains("width=1280"), "width substitution: {s}");
        assert!(s.contains("height=720"), "height substitution: {s}");

        for required in [
            "appsrc",
            "qtdemux",
            "h264parse",
            "avdec_h264",
            "videoscale",
            "videoconvert",
            "mp4mux",
            "appsink",
        ] {
            assert!(s.contains(required), "missing pipeline element {required}: {s}");
        }
    }

    /// When the host has the `vtenc_h264_hw` plugin installed,
    /// `gst::parse::launch` should accept the pipeline string we
    /// generate. Soft-skips on hosts without the runtime.
    #[test]
    fn pipeline_str_parses_under_gstreamer_when_runtime_available() {
        if gst::init().is_err() {
            eprintln!("skipping: gst::init() failed (gstreamer-rs runtime not installed)");
            return;
        }
        if gst::ElementFactory::find("vtenc_h264_hw").is_none() {
            eprintln!("skipping: vtenc_h264_hw plugin not registered (not on macOS / applemedia plugin missing)");
            return;
        }
        let rendition = RenditionSpec::preset_720p();
        let s = pipeline_str_for(&rendition);
        let parsed = gst::parse::launch(&s);
        assert!(
            parsed.is_ok(),
            "gst::parse::launch failed: {:?}\npipeline: {s}",
            parsed.err()
        );
    }

    #[test]
    fn factory_opts_out_of_non_video_tracks_when_available() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = VideoToolboxTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
        if !factory.is_available() {
            eprintln!(
                "skipping: required GStreamer elements missing {:?}",
                factory.missing_elements()
            );
            return;
        }
        let ctx = TranscoderContext {
            broadcast: "live/demo".into(),
            track: "1.mp4".into(),
            meta: FragmentMeta::new("mp4a.40.2", 48_000),
            rendition: factory.rendition().clone(),
        };
        assert!(factory.build(&ctx).is_none(), "audio track must be skipped");
    }

    #[test]
    fn factory_returns_transcoder_for_video_track_when_available() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = VideoToolboxTranscoderFactory::new(RenditionSpec::preset_480p(), registry);
        if !factory.is_available() {
            eprintln!(
                "skipping: required GStreamer elements missing {:?}",
                factory.missing_elements()
            );
            return;
        }
        let ctx = TranscoderContext {
            broadcast: "live/demo".into(),
            track: "0.mp4".into(),
            meta: FragmentMeta::new("avc1.640028", 90_000),
            rendition: factory.rendition().clone(),
        };
        let built = factory.build(&ctx);
        assert!(built.is_some(), "video track must build a transcoder");
    }

    #[test]
    fn videotoolbox_transcoder_output_broadcast_name_concatenates_source_and_rendition() {
        let registry = FragmentBroadcasterRegistry::new();
        let transcoder = VideoToolboxTranscoder::new(RenditionSpec::preset_240p(), "live/cam1".into(), registry, None);
        assert_eq!(transcoder.output_broadcast_name(), "live/cam1/240p");
    }

    #[test]
    fn factory_name_is_videotoolbox_for_metric_labels() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = VideoToolboxTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
        assert_eq!(
            factory.name(),
            "videotoolbox",
            "metric labels expect a stable factory name"
        );
    }

    #[test]
    fn factory_skip_source_suffixes_builder_opts_out_of_custom_names() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = VideoToolboxTranscoderFactory::new(RenditionSpec::preset_720p(), registry)
            .skip_source_suffixes(["ultra".to_string()]);
        if !factory.is_available() {
            eprintln!(
                "skipping: required GStreamer elements missing {:?}",
                factory.missing_elements()
            );
            return;
        }
        let ctx = TranscoderContext {
            broadcast: "live/demo/ultra".into(),
            track: "0.mp4".into(),
            meta: FragmentMeta::new("avc1.640028", 90_000),
            rendition: factory.rendition().clone(),
        };
        assert!(
            factory.build(&ctx).is_none(),
            "custom suffix must be treated as already-transcoded",
        );
    }
}
