//! [`AgentRunner`] + [`AgentRunnerHandle`] + [`AgentStats`].
//!
//! Wires registered [`crate::AgentFactory`] instances into the
//! shared [`lvqr_fragment::FragmentBroadcasterRegistry`] and
//! drives one tokio drain task per agent instance. Mirrors the
//! shape of `lvqr_wasm::install_wasm_filter_bridge` and
//! `lvqr_cli::archive::BroadcasterArchiveIndexer::install`.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use lvqr_fragment::{BroadcasterStream, FragmentBroadcasterRegistry, FragmentStream};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::agent::{Agent, AgentContext};
use crate::factory::AgentFactory;

/// Per-`(agent, broadcast, track)` outcome counters.
#[derive(Debug, Default)]
pub struct AgentStats {
    /// Total fragments handed to [`Agent::on_fragment`]
    /// (regardless of panic outcome).
    pub fragments_seen: AtomicU64,

    /// Count of caught panics across `on_start`, `on_fragment`,
    /// and `on_stop` for this `(agent, broadcast, track)`.
    pub panics: AtomicU64,
}

type StatsKey = (String, String, String);

/// Cheaply-cloneable handle returned by
/// [`AgentRunner::install`].
///
/// Holds the spawned per-agent drain tasks alive for the
/// server lifetime; tests and admin consumers read per-
/// `(agent, broadcast, track)` counters off this handle.
/// Dropping it aborts every spawned task; mid-stride aborts
/// do NOT call [`Agent::on_stop`], same shutdown shape as
/// `WasmFilterBridgeHandle`.
#[derive(Clone)]
pub struct AgentRunnerHandle {
    stats: Arc<DashMap<StatsKey, Arc<AgentStats>>>,
    _tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl std::fmt::Debug for AgentRunnerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRunnerHandle")
            .field("tracked_keys", &self.stats.len())
            .finish()
    }
}

impl AgentRunnerHandle {
    /// Total fragments observed by `agent` for
    /// `(broadcast, track)` (kept plus dropped). Returns 0 if
    /// no agent under that label has fired yet for this key.
    pub fn fragments_seen(&self, agent: &str, broadcast: &str, track: &str) -> u64 {
        self.stat(agent, broadcast, track)
            .map(|s| s.fragments_seen.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Caught-panic count for `agent` at `(broadcast, track)`.
    /// Aggregates `on_start`, `on_fragment`, and `on_stop`
    /// panics under one counter.
    pub fn panics(&self, agent: &str, broadcast: &str, track: &str) -> u64 {
        self.stat(agent, broadcast, track)
            .map(|s| s.panics.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Snapshot of every `(agent, broadcast, track)` triple the
    /// runner has spawned a drain task for.
    pub fn tracked(&self) -> Vec<StatsKey> {
        self.stats.iter().map(|e| e.key().clone()).collect()
    }

    fn stat(&self, agent: &str, broadcast: &str, track: &str) -> Option<Arc<AgentStats>> {
        self.stats
            .get(&(agent.to_string(), broadcast.to_string(), track.to_string()))
            .map(|e| Arc::clone(e.value()))
    }
}

/// Builder that collects [`AgentFactory`] registrations and
/// installs them onto a [`FragmentBroadcasterRegistry`].
///
/// Typical usage:
///
/// ```no_run
/// # use lvqr_agent::{Agent, AgentContext, AgentFactory, AgentRunner};
/// # use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry};
/// # struct CaptionsFactory;
/// # impl AgentFactory for CaptionsFactory {
/// #     fn name(&self) -> &str { "captions" }
/// #     fn build(&self, _ctx: &AgentContext) -> Option<Box<dyn Agent>> { None }
/// # }
/// let registry = FragmentBroadcasterRegistry::new();
/// let _handle = AgentRunner::new()
///     .with_factory(CaptionsFactory)
///     .install(&registry);
/// // hold _handle for the server lifetime
/// ```
#[derive(Default)]
pub struct AgentRunner {
    factories: Vec<Arc<dyn AgentFactory>>,
}

impl AgentRunner {
    /// Construct an empty runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an agent factory by value.
    pub fn with_factory<F: AgentFactory>(mut self, factory: F) -> Self {
        self.factories.push(Arc::new(factory));
        self
    }

    /// Register a pre-arc'd agent factory. Useful when the
    /// caller already shares an `Arc<dyn AgentFactory>` with
    /// other server-side state (e.g. an admin route).
    pub fn with_factory_arc(mut self, factory: Arc<dyn AgentFactory>) -> Self {
        self.factories.push(factory);
        self
    }

    /// How many factories are currently registered. Useful for
    /// `Default`-instantiated runners that want to gate their
    /// own install calls.
    pub fn factory_count(&self) -> usize {
        self.factories.len()
    }

    /// Wire an `on_entry_created` callback on `registry` so
    /// every new `(broadcast, track)` pair gets one drain task
    /// per agent the registered factories opt into. Returns a
    /// handle the caller MUST hold for the server lifetime;
    /// dropping it aborts every spawned task.
    ///
    /// The callback runs on the thread that wins the
    /// `get_or_create` insertion race. It subscribes to the
    /// fresh broadcaster synchronously (so no emit races ahead
    /// of the drain loop) and spawns the per-agent drain task
    /// on the current tokio runtime. If no tokio runtime is
    /// available (the registry callback fires from a non-tokio
    /// context), the warn is logged and no task spawns.
    pub fn install(self, registry: &FragmentBroadcasterRegistry) -> AgentRunnerHandle {
        let stats: Arc<DashMap<StatsKey, Arc<AgentStats>>> = Arc::new(DashMap::new());
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
                        "AgentRunner: registry callback fired outside tokio runtime; no drain spawned",
                    );
                    return;
                }
            };

            let ctx = AgentContext {
                broadcast: broadcast.to_string(),
                track: track.to_string(),
                meta: bc.meta(),
            };

            for factory in &factories {
                let Some(agent) = factory.build(&ctx) else {
                    continue;
                };
                // Subscribe synchronously inside the callback so
                // no emit can race ahead of the drain loop. The
                // BroadcasterStream owns only the Receiver side
                // (not an `Arc<FragmentBroadcaster>`), so the
                // drain task does not extend the broadcaster's
                // lifetime past the producers' -- the recv loop
                // sees `Closed` once every ingest clone drops.
                let sub = bc.subscribe();
                let key: StatsKey = (factory.name().to_string(), broadcast.to_string(), track.to_string());
                let stat = Arc::clone(
                    stats_cb
                        .entry(key.clone())
                        .or_insert_with(|| Arc::new(AgentStats::default()))
                        .value(),
                );
                let agent_name = factory.name().to_string();
                let ctx_for_task = ctx.clone();
                let task = handle.spawn(drive(agent, agent_name, ctx_for_task, sub, stat));
                tasks_cb.lock().push(task);
            }
        });

        info!(
            factories = registry_callback_factory_count(&tasks),
            "AgentRunner installed on FragmentBroadcasterRegistry"
        );

        AgentRunnerHandle { stats, _tasks: tasks }
    }
}

/// Per-agent drain task. Runs until the broadcaster closes.
/// All trait dispatch is wrapped in `catch_unwind` so a panic
/// in any one of `on_start` / `on_fragment` / `on_stop` is
/// logged + counted but does not propagate to the spawning
/// runtime.
async fn drive(
    mut agent: Box<dyn Agent>,
    agent_name: String,
    ctx: AgentContext,
    mut sub: BroadcasterStream,
    stats: Arc<AgentStats>,
) {
    // on_start: a panic here means we abort the drain loop.
    // Running on_fragment / on_stop after a failed start would
    // hand the agent fragments it never had a chance to
    // initialise for, which is worse than skipping.
    let started = std::panic::catch_unwind(AssertUnwindSafe(|| agent.on_start(&ctx)));
    if started.is_err() {
        stats.panics.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lvqr_agent_panics_total",
            "agent" => agent_name.clone(),
            "phase" => "start",
        )
        .increment(1);
        warn!(
            agent = %agent_name,
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            "Agent::on_start panicked; skipping drain loop",
        );
        return;
    }

    while let Some(frag) = sub.next_fragment().await {
        stats.fragments_seen.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("lvqr_agent_fragments_total", "agent" => agent_name.clone()).increment(1);
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| agent.on_fragment(&frag)));
        if result.is_err() {
            stats.panics.fetch_add(1, Ordering::Relaxed);
            metrics::counter!(
                "lvqr_agent_panics_total",
                "agent" => agent_name.clone(),
                "phase" => "fragment",
            )
            .increment(1);
            warn!(
                agent = %agent_name,
                broadcast = %ctx.broadcast,
                track = %ctx.track,
                group_id = frag.group_id,
                object_id = frag.object_id,
                "Agent::on_fragment panicked; skipping fragment and continuing",
            );
        }
    }

    let stopped = std::panic::catch_unwind(AssertUnwindSafe(|| agent.on_stop()));
    if stopped.is_err() {
        stats.panics.fetch_add(1, Ordering::Relaxed);
        metrics::counter!(
            "lvqr_agent_panics_total",
            "agent" => agent_name.clone(),
            "phase" => "stop",
        )
        .increment(1);
        warn!(
            agent = %agent_name,
            broadcast = %ctx.broadcast,
            track = %ctx.track,
            "Agent::on_stop panicked",
        );
    }

    info!(
        agent = %agent_name,
        broadcast = %ctx.broadcast,
        track = %ctx.track,
        seen = stats.fragments_seen.load(Ordering::Relaxed),
        panics = stats.panics.load(Ordering::Relaxed),
        "AgentRunner: drain terminated",
    );
}

/// Snapshot the spawned-task count from inside the install
/// log line. Pulled into a tiny helper because the `tasks`
/// `Mutex` is acquired briefly and the locking pattern reads
/// better in a function than inline in a `info!` call.
fn registry_callback_factory_count(tasks: &Arc<Mutex<Vec<JoinHandle<()>>>>) -> usize {
    tasks.lock().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::AgentFactory;
    use bytes::Bytes;
    use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
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

    /// Records every fragment + every lifecycle call into
    /// shared state. The runner's drain task drops the agent
    /// when the broadcaster closes, so the captured state
    /// outlives the agent itself.
    #[derive(Default)]
    struct Capture {
        starts: PMutex<Vec<AgentContext>>,
        fragments: PMutex<Vec<Fragment>>,
        stops: PMutex<u32>,
    }

    struct CaptureAgent(Arc<Capture>);

    impl Agent for CaptureAgent {
        fn on_start(&mut self, ctx: &AgentContext) {
            self.0.starts.lock().push(ctx.clone());
        }
        fn on_fragment(&mut self, fragment: &Fragment) {
            self.0.fragments.lock().push(fragment.clone());
        }
        fn on_stop(&mut self) {
            *self.0.stops.lock() += 1;
        }
    }

    struct CaptureFactory {
        capture: Arc<Capture>,
        name: &'static str,
        accept_track: Option<&'static str>,
    }

    impl AgentFactory for CaptureFactory {
        fn name(&self) -> &str {
            self.name
        }
        fn build(&self, ctx: &AgentContext) -> Option<Box<dyn Agent>> {
            if let Some(want) = self.accept_track
                && ctx.track != want
            {
                return None;
            }
            Some(Box::new(CaptureAgent(Arc::clone(&self.capture))))
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agent_receives_every_emitted_fragment_then_stops() {
        let registry = FragmentBroadcasterRegistry::new();
        let capture = Arc::new(Capture::default());
        let _handle = AgentRunner::new()
            .with_factory(CaptureFactory {
                capture: Arc::clone(&capture),
                name: "capture",
                accept_track: None,
            })
            .install(&registry);

        let bc = registry.get_or_create("live", "0.mp4", meta());
        for i in 0..5 {
            bc.emit(frag(i));
        }
        // Drop the producer-side clone so the drain loop sees
        // Closed and on_stop fires.
        drop(bc);
        // Also remove the registry-side handle so no clone
        // keeps the broadcaster alive.
        registry.remove("live", "0.mp4");
        // Yield long enough for the drain task to finish.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let starts = capture.starts.lock().clone();
        assert_eq!(starts.len(), 1, "on_start fires exactly once");
        assert_eq!(starts[0].broadcast, "live");
        assert_eq!(starts[0].track, "0.mp4");
        assert_eq!(starts[0].meta.timescale, 90_000);

        let frags = capture.fragments.lock().clone();
        assert_eq!(frags.len(), 5, "agent saw all 5 emitted fragments");
        for (i, f) in frags.iter().enumerate() {
            assert_eq!(f.group_id, i as u64);
        }

        assert_eq!(*capture.stops.lock(), 1, "on_stop fires exactly once");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn factory_returning_none_is_skipped() {
        let registry = FragmentBroadcasterRegistry::new();
        let capture = Arc::new(Capture::default());
        // Only accepts the audio track; video gets skipped.
        let handle = AgentRunner::new()
            .with_factory(CaptureFactory {
                capture: Arc::clone(&capture),
                name: "audio_only",
                accept_track: Some("1.mp4"),
            })
            .install(&registry);

        let bc_video = registry.get_or_create("live", "0.mp4", meta());
        bc_video.emit(frag(0));
        let bc_audio = registry.get_or_create("live", "1.mp4", FragmentMeta::new("mp4a.40.2", 48_000));
        bc_audio.emit(frag(7));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let frags = capture.fragments.lock().clone();
        assert_eq!(frags.len(), 1, "agent only saw the audio fragment");
        assert_eq!(frags[0].group_id, 7);

        let tracked = handle.tracked();
        assert_eq!(tracked.len(), 1, "stats keyed only on the audio drain");
        assert_eq!(tracked[0], ("audio_only".into(), "live".into(), "1.mp4".into()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_in_on_fragment_is_caught_and_counted_loop_continues() {
        struct PanickyAgent {
            seen: Arc<AtomicU64>,
        }
        impl Agent for PanickyAgent {
            fn on_fragment(&mut self, fragment: &Fragment) {
                self.seen.fetch_add(1, Ordering::Relaxed);
                if fragment.group_id == 1 {
                    panic!("simulated agent fault at group 1");
                }
            }
        }
        struct PanickyFactory {
            seen: Arc<AtomicU64>,
        }
        impl AgentFactory for PanickyFactory {
            fn name(&self) -> &str {
                "panicky"
            }
            fn build(&self, _ctx: &AgentContext) -> Option<Box<dyn Agent>> {
                Some(Box::new(PanickyAgent {
                    seen: Arc::clone(&self.seen),
                }))
            }
        }

        let registry = FragmentBroadcasterRegistry::new();
        let seen = Arc::new(AtomicU64::new(0));
        let handle = AgentRunner::new()
            .with_factory(PanickyFactory {
                seen: Arc::clone(&seen),
            })
            .install(&registry);

        let bc = registry.get_or_create("live", "0.mp4", meta());
        // Also subscribe a downstream consumer to assert the
        // bad agent's panic does not perturb the underlying
        // broadcaster's emission to other subscribers.
        let mut downstream = bc.subscribe();
        for i in 0..3 {
            bc.emit(frag(i));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        // All 3 fragments were handed to on_fragment (the
        // counter increments before the panic fires).
        assert_eq!(seen.load(Ordering::Relaxed), 3);
        // Stats counter records the seen count and the panic.
        assert_eq!(handle.fragments_seen("panicky", "live", "0.mp4"), 3);
        assert_eq!(handle.panics("panicky", "live", "0.mp4"), 1);

        // Downstream subscriber still received every fragment.
        for expected in 0..3u64 {
            let f = downstream.next_fragment().await.expect("downstream frag");
            assert_eq!(f.group_id, expected);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn panic_in_on_start_skips_drain_loop() {
        struct PanicStartAgent;
        impl Agent for PanicStartAgent {
            fn on_start(&mut self, _ctx: &AgentContext) {
                panic!("simulated start failure");
            }
            fn on_fragment(&mut self, _fragment: &Fragment) {
                unreachable!("on_fragment must not run after on_start panics");
            }
        }
        struct PanicStartFactory;
        impl AgentFactory for PanicStartFactory {
            fn name(&self) -> &str {
                "panic_start"
            }
            fn build(&self, _ctx: &AgentContext) -> Option<Box<dyn Agent>> {
                Some(Box::new(PanicStartAgent))
            }
        }

        let registry = FragmentBroadcasterRegistry::new();
        let handle = AgentRunner::new().with_factory(PanicStartFactory).install(&registry);

        let bc = registry.get_or_create("live", "0.mp4", meta());
        bc.emit(frag(0));
        bc.emit(frag(1));
        tokio::time::sleep(Duration::from_millis(100)).await;

        // No fragments processed because on_start panicked.
        assert_eq!(handle.fragments_seen("panic_start", "live", "0.mp4"), 0);
        assert_eq!(handle.panics("panic_start", "live", "0.mp4"), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_runner_installs_callback_but_spawns_nothing() {
        let registry = FragmentBroadcasterRegistry::new();
        let handle = AgentRunner::new().install(&registry);

        let bc = registry.get_or_create("live", "0.mp4", meta());
        bc.emit(frag(0));
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(handle.tracked().is_empty(), "no factories means no drain tasks");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multiple_factories_each_get_their_own_drain_per_broadcast() {
        let registry = FragmentBroadcasterRegistry::new();
        let cap_a = Arc::new(Capture::default());
        let cap_b = Arc::new(Capture::default());
        let handle = AgentRunner::new()
            .with_factory(CaptureFactory {
                capture: Arc::clone(&cap_a),
                name: "alpha",
                accept_track: None,
            })
            .with_factory(CaptureFactory {
                capture: Arc::clone(&cap_b),
                name: "beta",
                accept_track: None,
            })
            .install(&registry);

        let bc = registry.get_or_create("live", "0.mp4", meta());
        bc.emit(frag(0));
        bc.emit(frag(1));
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(handle.fragments_seen("alpha", "live", "0.mp4"), 2);
        assert_eq!(handle.fragments_seen("beta", "live", "0.mp4"), 2);
        assert_eq!(cap_a.fragments.lock().len(), 2);
        assert_eq!(cap_b.fragments.lock().len(), 2);
    }

    #[test]
    fn agent_runner_default_is_empty() {
        let r = AgentRunner::default();
        assert_eq!(r.factory_count(), 0);
    }

    #[test]
    fn agent_runner_handle_debug_redacts_internals() {
        let stats: Arc<DashMap<StatsKey, Arc<AgentStats>>> = Arc::new(DashMap::new());
        stats.insert(("a".into(), "b".into(), "c".into()), Arc::new(AgentStats::default()));
        let handle = AgentRunnerHandle {
            stats,
            _tasks: Arc::new(Mutex::new(Vec::new())),
        };
        let printed = format!("{handle:?}");
        assert!(printed.contains("tracked_keys"));
        assert!(printed.contains("1"), "{printed}");
    }
}
