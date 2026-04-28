//! [`NvencTranscoder`] + [`NvencTranscoderFactory`].
//!
//! Hardware-encoder backend on Linux + Nvidia GPUs. Mirrors the
//! [`crate::software`] and [`crate::videotoolbox`] modules in shape
//! -- same `Transcoder` trait, same lifecycle, same
//! `<source>/<rendition>` output broadcast naming -- but swaps the
//! GStreamer encoder element for `nvh264enc` (the H.264 encoder
//! provided by the `nvcodec` plugin in `gst-plugins-bad`, which
//! drives Nvidia's NVENC silicon via the CUDA runtime). HW-only
//! path is intentional: a HW factory that silently falls back to
//! CPU encoding under load defeats the purpose of an
//! operator-pickable hardware tier.
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
//!   ! nvh264enc bitrate=<kbps> gop-size=60 rc-mode=cbr zerolatency=true
//!   ! h264parse
//!   ! mp4mux streamable=true fragment-duration=2000
//!   ! appsink name=sink emit-signals=true
//! ```
//!
//! Worker thread pattern matches [`crate::videotoolbox`] verbatim:
//! one OS thread per `(source, rendition)` pair owning the whole
//! pipeline, a bounded mpsc carrying `Bytes` from `on_fragment`,
//! EOS-on-drop + 5 s drain timeout on `on_stop`.
//!
//! Only the `"0.mp4"` video track is accepted; the existing
//! [`crate::AudioPassthroughTranscoderFactory`] handles the
//! `"1.mp4"` audio sibling.
//!
//! Operator install on Ubuntu / Debian:
//!
//! ```text
//! sudo apt-get install \
//!   gstreamer1.0-plugins-bad gstreamer1.0-libav \
//!   libnvidia-encode1 nvidia-cuda-toolkit
//! ```
//!
//! plus a working Nvidia driver. `nvh264enc` lives in
//! `gst-plugins-bad`'s `nvcodec` plugin and probes the CUDA runtime
//! at `gst::init()` time.

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

const DEFAULT_SOURCE_TRACK: &str = "0.mp4";

/// GStreamer elements the NVENC pipeline requires. Probed at factory
/// construction; if any is missing the factory opts out of every
/// `build()` with a warn log. `nvh264enc` ships in the `nvcodec`
/// plugin from `gst-plugins-bad` and dynamically probes the CUDA
/// runtime + a usable Nvidia GPU at element-factory load time, so a
/// host without the driver / GPU registers no `nvh264enc` factory
/// and we report it missing here.
const REQUIRED_ELEMENTS: &[&str] = &[
    "appsrc",
    "qtdemux",
    "h264parse",
    "avdec_h264",
    "videoscale",
    "videoconvert",
    "nvh264enc",
    "mp4mux",
    "appsink",
];

const FACTORY_NAME: &str = "nvenc";

const WORKER_QUEUE_DEPTH: usize = 64;
const SHUTDOWN_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

/// Output codec string for the rendition's `FragmentMeta`. NVENC
/// `nvh264enc` emits Main / High depending on configuration; the
/// constant matches the `software` + `videotoolbox` sibling so
/// downstream HLS / DASH consumers see a consistent codec hint.
const OUTPUT_CODEC: &str = "avc1.640028";

const OUTPUT_TRACK: &str = "0.mp4";
const OUTPUT_TIMESCALE: u32 = 90_000;

/// Factory that builds [`NvencTranscoder`] instances for the
/// `"0.mp4"` video track. One factory instance per rendition; the
/// CLI's `--transcode-encoder nvenc` switch installs three of these
/// (one per default-ladder rung) instead of
/// [`crate::SoftwareTranscoderFactory`].
pub struct NvencTranscoderFactory {
    rendition: RenditionSpec,
    output_registry: FragmentBroadcasterRegistry,
    missing_elements: Vec<&'static str>,
    skip_source_suffixes: Vec<String>,
}

impl NvencTranscoderFactory {
    /// Construct a factory for `rendition` that publishes output
    /// fragments into `output_registry` under `<source>/<rendition>`
    /// broadcasts.
    ///
    /// `gst::init()` is called here (idempotent across threads) and
    /// the required plugin list is probed. Missing elements are
    /// logged once at construction and cause every subsequent
    /// `build(ctx)` call to return `None`.
    pub fn new(rendition: RenditionSpec, output_registry: FragmentBroadcasterRegistry) -> Self {
        let missing_elements = match gst::init() {
            Ok(()) => missing_required_elements(),
            Err(err) => {
                warn!(
                    rendition = %rendition.name,
                    error = %err,
                    "NvencTranscoderFactory: gst::init() failed",
                );
                REQUIRED_ELEMENTS.to_vec()
            }
        };
        if !missing_elements.is_empty() {
            warn!(
                rendition = %rendition.name,
                missing = ?missing_elements,
                "NvencTranscoderFactory: required GStreamer elements absent; factory will opt out of every build()",
            );
        }
        Self {
            rendition,
            output_registry,
            missing_elements,
            skip_source_suffixes: Vec::new(),
        }
    }

    pub fn skip_source_suffixes(mut self, suffixes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.skip_source_suffixes.extend(suffixes.into_iter().map(Into::into));
        self
    }

    pub fn is_available(&self) -> bool {
        self.missing_elements.is_empty()
    }

    pub fn missing_elements(&self) -> &[&'static str] {
        &self.missing_elements
    }
}

impl TranscoderFactory for NvencTranscoderFactory {
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
                "NvencTranscoderFactory: skipping already-transcoded broadcast",
            );
            return None;
        }
        Some(Box::new(NvencTranscoder::new(
            ctx.rendition.clone(),
            ctx.broadcast.clone(),
            self.output_registry.clone(),
            ctx.meta.init_segment.clone(),
        )))
    }
}

pub struct NvencTranscoder {
    rendition: RenditionSpec,
    source_broadcast: String,
    output_registry: FragmentBroadcasterRegistry,
    initial_header: Option<Bytes>,
    worker: Option<WorkerHandle>,
    dropped_fragments: Arc<AtomicU64>,
}

impl NvencTranscoder {
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

    pub fn dropped_fragments(&self) -> u64 {
        self.dropped_fragments.load(Ordering::Relaxed)
    }
}

impl Transcoder for NvencTranscoder {
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
                    "NvencTranscoder: worker spawned",
                );
                self.worker = Some(handle);
            }
            Err(err) => {
                warn!(
                    broadcast = %self.source_broadcast,
                    rendition = %self.rendition.name,
                    error = %err,
                    "NvencTranscoder: failed to spawn worker; transcoder will drop every fragment",
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

impl Drop for NvencTranscoder {
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
            .name(format!("lvqr-transcode-nv:{source_broadcast}:{}", rendition.name))
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
                    "NvencTranscoder: worker channel full; dropping source fragment",
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                debug!(
                    broadcast = %self.source_broadcast,
                    rendition = %self.rendition,
                    "NvencTranscoder: worker already shut down; fragment discarded",
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
                "NvencTranscoder: worker thread panicked during shutdown",
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

fn build_pipeline(rendition: &RenditionSpec) -> Result<BuiltPipeline, WorkerSpawnError> {
    // NVENC property mapping (vs the x264enc software path):
    //   bitrate=<kbps>      same units (nvh264enc takes kbit/s)
    //   gop-size=60         replaces key-int-max=60
    //   rc-mode=cbr         constant-bitrate rate control
    //   zerolatency=true    matches tune=zerolatency intent (no B-frames,
    //                       low-delay encode)
    let pipeline_str = format!(
        "appsrc name=src caps=video/quicktime is-live=false format=time \
         ! qtdemux \
         ! h264parse \
         ! avdec_h264 \
         ! videoscale \
         ! video/x-raw,width={w},height={h} \
         ! videoconvert \
         ! nvh264enc bitrate={kbps} gop-size=60 rc-mode=cbr zerolatency=true \
         ! h264parse \
         ! mp4mux streamable=true fragment-duration=2000 \
         ! appsink name=sink emit-signals=true sync=false",
        w = rendition.width,
        h = rendition.height,
        kbps = rendition.video_bitrate_kbps,
    );
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
                        "NvencTranscoder: output header buffer received; caching as init_segment",
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
            "NvencTranscoder: failed to set pipeline to Playing",
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
            "NvencTranscoder: end_of_stream signal failed",
        );
    }

    wait_for_drain(&pipeline, &source_broadcast, &rendition.name);

    if let Err(err) = pipeline.set_state(gst::State::Null) {
        warn!(
            broadcast = %source_broadcast,
            rendition = %rendition.name,
            error = %err,
            "NvencTranscoder: failed to set pipeline to Null",
        );
    }

    info!(
        broadcast = %source_broadcast,
        output = %output_broadcast,
        rendition = %rendition.name,
        dropped = dropped_counter.load(Ordering::Relaxed),
        "NvencTranscoder: worker exited",
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
            warn!(error = ?err, "NvencTranscoder: appsrc.push_buffer failed");
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
            "NvencTranscoder: pipeline did not drain within timeout; forcing Null",
        );
        return;
    };
    if let gst::MessageView::Error(err) = msg.view() {
        warn!(
            broadcast = source_broadcast,
            rendition,
            error = %err.error(),
            debug = ?err.debug(),
            "NvencTranscoder: pipeline reported error during drain",
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

fn ns_to_ticks(ns: u64, timescale: u32) -> u64 {
    ns.saturating_mul(timescale as u64) / 1_000_000_000u64
}

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
    fn pipeline_string_embeds_rendition_geometry_and_bitrate() {
        let pipeline_str = format!(
            "bitrate={kbps} ... width={w} height={h} ...",
            w = 854,
            h = 480,
            kbps = 1_200,
        );
        assert!(pipeline_str.contains("width=854"));
        assert!(pipeline_str.contains("height=480"));
        assert!(pipeline_str.contains("bitrate=1200"));
    }

    #[test]
    fn factory_opts_out_of_non_video_tracks_when_available() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = NvencTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
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
        let factory = NvencTranscoderFactory::new(RenditionSpec::preset_480p(), registry);
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
        assert!(factory.build(&ctx).is_some(), "video track must build a transcoder");
    }

    #[test]
    fn nvenc_transcoder_output_broadcast_name_concatenates_source_and_rendition() {
        let registry = FragmentBroadcasterRegistry::new();
        let transcoder = NvencTranscoder::new(RenditionSpec::preset_240p(), "live/cam1".into(), registry, None);
        assert_eq!(transcoder.output_broadcast_name(), "live/cam1/240p");
    }

    #[test]
    fn factory_name_is_nvenc_for_metric_labels() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = NvencTranscoderFactory::new(RenditionSpec::preset_720p(), registry);
        assert_eq!(factory.name(), "nvenc", "metric labels expect a stable factory name");
    }

    #[test]
    fn factory_skip_source_suffixes_builder_opts_out_of_custom_names() {
        let registry = FragmentBroadcasterRegistry::new();
        let factory = NvencTranscoderFactory::new(RenditionSpec::preset_720p(), registry)
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
