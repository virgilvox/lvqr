//! [`TranscodeRunner`] + [`TranscodeRunnerHandle`] + [`TranscoderStats`].
//!
//! Wires registered [`crate::TranscoderFactory`] instances into a
//! shared [`lvqr_fragment::FragmentBroadcasterRegistry`] and drives
//! one tokio drain task per `(transcoder, rendition, broadcast,
//! track)` instance. Mirrors [`lvqr_agent::AgentRunner`] one-for-
//! one, with `(factory_name, rendition_name, broadcast, track)` as
//! the four-tuple stats key so metrics distinguish renditions of
//! the same factory.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use lvqr_fragment::{BroadcasterStream, FragmentBroadcasterRegistry, FragmentStream};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::transcoder::{Transcoder, TranscoderContext, TranscoderFactory};

/// Per-`(transcoder, rendition, broadcast, track)` outcome
/// counters.
#[derive(Debug, Default)]
pub struct TranscoderStats {
    /// Total fragments handed to [`Transcoder::on_fragment`]
    /// (regardless of panic outcome).
    pub fragments_seen: AtomicU64,

    /// Count of caught panics across `on_start`, `on_fragment`,
    /// and `on_stop` for this key.
    pub panics: AtomicU64,
}

/// Stats key: `(transcoder_name, rendition_name, broadcast, track)`.
/// Two factories of the same name targeting different renditions
/// live under separate keys so metrics distinguish them.
type StatsKey = (String, String, String, String);

/// Cheaply-cloneable handle returned by
/// [`TranscodeRunner::install`].
///
/// Holds the spawned per-transcoder drain tasks alive for the
/// server lifetime; tests and admin consumers read per-
/// `(transcoder, rendition, broadcast, track)` counters off this
/// handle. Dropping it aborts every spawned task; mid-stride
/// aborts do NOT call [`Transcoder::on_stop`], matching the
/// [`lvqr_agent::AgentRunnerHandle`] shutdown shape.
#[derive(Clone)]
pub struct TranscodeRunnerHandle {
    stats: Arc<DashMap<StatsKey, Arc<TranscoderStats>>>,
    _tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl std::fmt::Debug for TranscodeRunnerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TranscodeRunnerHandle")
            .field("tracked_keys", &self.stats.len())
            .finish()
    }
}

impl TranscodeRunnerHandle {
    /// Total fragments observed by `transcoder` producing
    /// `rendition` from `(broadcast, track)`. Returns 0 if no
    /// transcoder under that key has fired yet.
    pub fn fragments_seen(&self, transcoder: &str, rendition: &str, broadcast: &str, track: &str) -> u64 {
        self.stat(transcoder, rendition, broadcast, track)
            .map(|s| s.fragments_seen.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Caught-panic count for `transcoder` producing `rendition`
    /// from `(broadcast, track)`. Aggregates `on_start`,
    /// `on_fragment`, and `on_stop` panics under one counter.
    pub fn panics(&self, transcoder: &str, rendition: &str, broadcast: &str, track: &str) -> u64 {
        self.stat(transcoder, rendition, broadcast, track)
            .map(|s| s.panics.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Snapshot of every `(transcoder, rendition, broadcast, track)`
    /// quadruple the runner has spawned a drain task for.
    pub fn tracked(&self) -> Vec<StatsKey> {
        self.stats.iter().map(|e| e.key().clone()).collect()
    }

    fn stat(&self, transcoder: &str, rendition: &str, broadcast: &str, track: &str) -> Option<Arc<TranscoderStats>> {
        self.stats
            .get(&(
                transcoder.to_string(),
                rendition.to_string(),
                broadcast.to_string(),
                track.to_string(),
            ))
            .map(|e| Arc::clone(e.value()))
    }
}

/// Builder that collects [`TranscoderFactory`] registrations and
/// installs them onto a [`FragmentBroadcasterRegistry`]. Typical
/// usage -- three rungs of the default ladder:
///
/// ```no_run
/// # use lvqr_transcode::{PassthroughTranscoderFactory, RenditionSpec, TranscodeRunner};
/// # use lvqr_fragment::FragmentBroadcasterRegistry;
/// let registry = FragmentBroadcasterRegistry::new();
/// let _handle = TranscodeRunner::new()
///     .with_ladder(RenditionSpec::default_ladder(), |spec| {
///         PassthroughTranscoderFactory::new(spec)
///     })
///     .install(&registry);
/// // hold _handle for the server lifetime
/// ```
#[derive(Default)]
pub struct TranscodeRunner {
    factories: Vec<Arc<dyn TranscoderFactory>>,
}

impl TranscodeRunner {
    /// Construct an empty runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a transcoder factory by value.
    pub fn with_factory<F: TranscoderFactory>(mut self, factory: F) -> Self {
        self.factories.push(Arc::new(factory));
        self
    }

    /// Register a pre-arc'd factory. Useful when the caller
    /// already shares an `Arc<dyn TranscoderFactory>` with other
    /// server-side state.
    pub fn with_factory_arc(mut self, factory: Arc<dyn TranscoderFactory>) -> Self {
        self.factories.push(factory);
        self
    }

    /// Convenience: register one factory per rendition in the
    /// supplied ladder, building each factory from its rendition
    /// via `build`. Mirrors the `RenditionSpec::default_ladder()`
    /// -> three `PassthroughTranscoderFactory` pattern without
    /// forcing the caller to unroll it.
    pub fn with_ladder<F, Fn_>(mut self, ladder: Vec<crate::RenditionSpec>, build: Fn_) -> Self
    where
        F: TranscoderFactory,
        Fn_: Fn(crate::RenditionSpec) -> F,
    {
        for spec in ladder {
            self.factories.push(Arc::new(build(spec)));
        }
        self
    }

    /// How many factories are currently registered. Useful for
    /// `Default`-instantiated runners that want to gate their own
    /// install calls.
    pub fn factory_count(&self) -> usize {
        self.factories.len()
    }

    /// Wire an `on_entry_created` callback on `registry` so every
    /// new `(broadcast, track)` pair gets one drain task per
    /// transcoder the registered factories opt into. Returns a
    /// handle the caller MUST hold for the server lifetime;
    /// dropping it aborts every spawned task.
    ///
    /// Callback semantics mirror [`lvqr_agent::AgentRunner::install`]:
    /// the callback runs on the thread that wins the
    /// `get_or_create` insertion race, subscribes synchronously
    /// so no emit can race ahead of the drain loop, and spawns
    /// the per-transcoder drain task on the current tokio
    /// runtime. If no tokio runtime is available the warn logs
    /// and no task spawns.
    pub fn install(self, registry: &FragmentBroadcasterRegistry) -> TranscodeRunnerHandle {
        let stats: Arc<DashMap<StatsKey, Arc<TranscoderStats>>> = Arc::new(DashMap::new());
        let tasks: Arc<Mutex<Vec<JoinHandle<()>>>> = Arc::new(Mutex::new(Vec::new()));

        let factories = self.factories;
        let stats_cb = Arc::clone(&stats);
        let tasks_cb = Arc::clone(&tasks);

        registry.on_entry_created(move |broadcast, track, bc| {
            let handle = match Handle::try_current() {
                Ok(h) => h,
                Err(_) => {
                    warn!(
                        broadcast = %broadcast,
                        track = %track,
                        "TranscodeRunner: registry callback fired outside tokio runtime; no drain spawned",
                    );
                    return;
                }
            };

            for factory in &factories {
                let rendition = factory.rendition().clone();
                let ctx = TranscoderContext {
                    broadcast: broadcast.to_string(),
                    track: track.to_string(),
                    meta: bc.meta(),
                    rendition: rendition.clone(),
                };
                let Some(transcoder) = factory.build(&ctx) else {
                    continue;
                };

                let sub = bc.subscribe();
                let key: StatsKey = (
                    factory.name().to_string(),
                    rendition.name.clone(),
                    broadcast.to_string(),
                    track.to_string(),
                );
                let stat = Arc::clone(
                    stats_cb
                        .entry(key.clone())
                        .or_insert_with(|| Arc::new(TranscoderStats::default()))
                        .value(),
                );
                let factory_name = factory.name().to_string();
                let ctx_for_task = ctx.clone();
                let task = handle.spawn(drive(transcoder, factory_name, ctx_for_task, sub, stat));
                tasks_cb.lock().push(task);
            }
        });

        info!(
            tracked = stats.len(),
            "TranscodeRunner installed on FragmentBroadcasterRegistry",
        );

        TranscodeRunnerHandle { stats, _tasks: tasks }
    }
}

/// Per-transcoder drain task. Runs until the broadcaster closes.
/// All trait dispatch is wrapped in `catch_unwind` so a panic in
/// any of `on_start` / `on_fragment` / `on_stop` is logged +
/// counted but does not propagate to the spawning runtime.
async fn drive(
    mut transcoder: Box<dyn Transcoder>,
    transcoder_name: String,
    ctx: TranscoderContext,
    mut sub: BroadcasterStream,
    stats: Arc<TranscoderStats>,
) {
    let rendition_name = ctx.rendition.name.clone();

    // Refresh the meta snapshot before `on_start`. The
    // `on_entry_created` callback fires synchronously inside
    // `FragmentBroadcasterRegistry::get_or_create`, *before* the
    // ingest side calls `set_init_segment`. A transcoder that
    // reads `ctx.meta.init_segment` at on_start time would miss
    // the header bytes -- which is a silent break for the
    // software pipeline (qtdemux finds no playable streams). The
    // refresh below catches the late init without changing the
    // trait surface. Tier 4 item 4.6 session 106 C fix.
    sub.refresh_meta();
    let ctx = TranscoderContext {
        broadcast: ctx.broadcast,
        track: ctx.track,
        meta: sub.meta().clone(),
        rendition: ctx.rendition,
    };

    // on_start: a panic here means we abort the drain loop.
    // Handing fragments to a transcoder whose setup panicked
    // would amplify the fault, not contain it.
    let started = std::panic::catch_unwind(AssertUnwindSafe(|| transcoder.on_start(&ctx)));
    if started.is_err() {
        stats.panics.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lvqr_transcode_panics_total",
            "transcoder" => transcoder_name.clone(),
            "rendition" => rendition_name.clone(),
            "phase" => "start",
        )
        .increment(1);
        warn!(
            transcoder = %transcoder_name,
            rendition = %rendition_name,
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            "Transcoder::on_start panicked; skipping drain loop",
        );
        return;
    }

    while let Some(frag) = sub.next_fragment().await {
        stats.fragments_seen.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lvqr_transcode_fragments_total",
            "transcoder" => transcoder_name.clone(),
            "rendition" => rendition_name.clone(),
        )
        .increment(1);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| transcoder.on_fragment(&frag)));
        if result.is_err() {
            stats.panics.fetch_add(1, Ordering::Relaxed);
            metrics::counter!(
                "lvqr_transcode_panics_total",
                "transcoder" => transcoder_name.clone(),
                "rendition" => rendition_name.clone(),
                "phase" => "fragment",
            )
            .increment(1);
            warn!(
                transcoder = %transcoder_name,
                rendition = %rendition_name,
                broadcast = %ctx.broadcast,
                track = %ctx.track,
                group_id = frag.group_id,
                object_id = frag.object_id,
                "Transcoder::on_fragment panicked; skipping fragment and continuing",
            );
        }
    }

    let stopped = std::panic::catch_unwind(AssertUnwindSafe(|| transcoder.on_stop()));
    if stopped.is_err() {
        stats.panics.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lvqr_transcode_panics_total",
            "transcoder" => transcoder_name.clone(),
            "rendition" => rendition_name.clone(),
            "phase" => "stop",
        )
        .increment(1);
        warn!(
            transcoder = %transcoder_name,
            rendition = %rendition_name,
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            "Transcoder::on_stop panicked",
        );
    }

    info!(
        transcoder = %transcoder_name,
        rendition = %rendition_name,
        broadcast = %ctx.broadcast,
        track = %ctx.track,
        seen = stats.fragments_seen.load(Ordering::Relaxed),
        panics = stats.panics.load(Ordering::Relaxed),
        "TranscodeRunner: drain terminated",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passthrough::PassthroughTranscoderFactory;
    use crate::rendition::RenditionSpec;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};
    use parking_lot::Mutex as PMutex;
    use std::time::Duration;

    fn meta() -> FragmentMeta {
        FragmentMeta::new("avc1.640028", 90_000)
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn passthrough_sees_every_fragment_and_stops() {
        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new()
            .with_factory(PassthroughTranscoderFactory::new(RenditionSpec::preset_720p()))
            .install(&registry);

        let bc = registry.get_or_create("live/demo", "0.mp4", meta());
        for i in 0..5 {
            bc.emit(frag(i));
        }
        drop(bc);
        registry.remove("live/demo", "0.mp4");
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(handle.fragments_seen("passthrough", "720p", "live/demo", "0.mp4"), 5);
        assert_eq!(handle.panics("passthrough", "720p", "live/demo", "0.mp4"), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn default_ladder_spawns_one_task_per_rendition() {
        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new()
            .with_ladder(RenditionSpec::default_ladder(), PassthroughTranscoderFactory::new)
            .install(&registry);

        let bc = registry.get_or_create("live/ladder", "0.mp4", meta());
        bc.emit(frag(0));
        bc.emit(frag(1));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Three renditions, each observing both fragments.
        let mut tracked = handle.tracked();
        tracked.sort();
        assert_eq!(tracked.len(), 3, "one drain task per rendition");
        for (_transcoder, rendition, _broadcast, _track) in &tracked {
            let seen = handle.fragments_seen("passthrough", rendition, "live/ladder", "0.mp4");
            assert_eq!(seen, 2, "rendition {rendition} saw both fragments");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_opt_out_skips_non_video_tracks() {
        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new()
            .with_factory(PassthroughTranscoderFactory::new(RenditionSpec::preset_720p()))
            .install(&registry);

        let bc_audio = registry.get_or_create("live/demo", "1.mp4", FragmentMeta::new("mp4a.40.2", 48_000));
        bc_audio.emit(frag(0));
        tokio::time::sleep(Duration::from_millis(80)).await;

        // Passthrough factory opts out of non-video tracks; no
        // drain task spawns for the audio track.
        assert!(handle.tracked().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_in_on_fragment_is_caught_and_counted() {
        struct PanicAtTwo;
        impl Transcoder for PanicAtTwo {
            fn on_fragment(&mut self, fragment: &Fragment) {
                if fragment.group_id == 2 {
                    panic!("simulated encoder fault at group 2");
                }
            }
        }
        struct PanicAtTwoFactory {
            rendition: RenditionSpec,
        }
        impl TranscoderFactory for PanicAtTwoFactory {
            fn name(&self) -> &str {
                "panicky"
            }
            fn rendition(&self) -> &RenditionSpec {
                &self.rendition
            }
            fn build(&self, _ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>> {
                Some(Box::new(PanicAtTwo))
            }
        }

        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new()
            .with_factory(PanicAtTwoFactory {
                rendition: RenditionSpec::preset_720p(),
            })
            .install(&registry);

        let bc = registry.get_or_create("live/panic", "0.mp4", meta());
        for i in 0..5 {
            bc.emit(frag(i));
        }
        tokio::time::sleep(Duration::from_millis(120)).await;

        assert_eq!(handle.fragments_seen("panicky", "720p", "live/panic", "0.mp4"), 5);
        assert_eq!(handle.panics("panicky", "720p", "live/panic", "0.mp4"), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_in_on_start_skips_drain_loop() {
        struct PanicStart;
        impl Transcoder for PanicStart {
            fn on_start(&mut self, _ctx: &TranscoderContext) {
                panic!("simulated start failure");
            }
            fn on_fragment(&mut self, _fragment: &Fragment) {
                unreachable!("on_fragment must not run after on_start panics");
            }
        }
        struct PanicStartFactory {
            rendition: RenditionSpec,
        }
        impl TranscoderFactory for PanicStartFactory {
            fn name(&self) -> &str {
                "bad_start"
            }
            fn rendition(&self) -> &RenditionSpec {
                &self.rendition
            }
            fn build(&self, _ctx: &TranscoderContext) -> Option<Box<dyn Transcoder>> {
                Some(Box::new(PanicStart))
            }
        }

        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new()
            .with_factory(PanicStartFactory {
                rendition: RenditionSpec::preset_480p(),
            })
            .install(&registry);

        let bc = registry.get_or_create("live/panic-start", "0.mp4", meta());
        bc.emit(frag(0));
        bc.emit(frag(1));
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(
            handle.fragments_seen("bad_start", "480p", "live/panic-start", "0.mp4"),
            0
        );
        assert_eq!(handle.panics("bad_start", "480p", "live/panic-start", "0.mp4"), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_runner_installs_callback_but_spawns_nothing() {
        let registry = FragmentBroadcasterRegistry::new();
        let handle = TranscodeRunner::new().install(&registry);

        let bc = registry.get_or_create("live/empty", "0.mp4", meta());
        bc.emit(frag(0));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(handle.tracked().is_empty());
    }

    #[test]
    fn runner_default_is_empty() {
        let r = TranscodeRunner::default();
        assert_eq!(r.factory_count(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downstream_subscriber_still_sees_every_fragment() {
        // A downstream consumer of the source broadcaster (e.g.
        // the LL-HLS bridge) must not be perturbed by transcoder
        // drain tasks. Assert the fan-out by subscribing
        // independently and reading every fragment.
        let registry = FragmentBroadcasterRegistry::new();
        let _handle = TranscodeRunner::new()
            .with_factory(PassthroughTranscoderFactory::new(RenditionSpec::preset_240p()))
            .install(&registry);

        let bc = registry.get_or_create("live/fanout", "0.mp4", meta());
        let mut downstream = bc.subscribe();
        let emitted = PMutex::new(Vec::<u64>::new());
        for i in 0..4 {
            bc.emit(frag(i));
            emitted.lock().push(i);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        for expected in 0..4u64 {
            let f = downstream.next_fragment().await.expect("downstream frag");
            assert_eq!(f.group_id, expected);
        }
    }
}
