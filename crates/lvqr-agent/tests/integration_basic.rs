//! End-to-end test of the [`lvqr_agent::AgentRunner`] against a
//! real [`lvqr_fragment::FragmentBroadcasterRegistry`].
//!
//! Slot-3 of the test contract: while the inline `#[cfg(test)]`
//! module in `runner.rs` covers panic isolation, opt-out, and
//! multi-factory wiring through the same registry, this
//! integration test runs the agent against an out-of-crate
//! caller that mirrors how `lvqr_cli::start` will install the
//! runner in session 98. The assertions are deliberately
//! conservative: prove that the `(broadcast, track)` lifecycle
//! shape -- start, drain, stop -- holds when driven by the
//! real registry across an `Arc` boundary, with the fragments
//! on a different runtime worker than the emit thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use lvqr_agent::{Agent, AgentContext, AgentFactory, AgentRunner};
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use parking_lot::Mutex;

/// Simple ordered-event log so the test can assert lifecycle
/// ordering (`start -> fragments -> stop`) end-to-end.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
}

struct LoggingAgent {
    recorder: Arc<Recorder>,
}

impl Agent for LoggingAgent {
    fn on_start(&mut self, ctx: &AgentContext) {
        self.recorder
            .events
            .lock()
            .push(format!("start broadcast={} track={}", ctx.broadcast, ctx.track));
    }
    fn on_fragment(&mut self, fragment: &Fragment) {
        self.recorder
            .events
            .lock()
            .push(format!("frag group={} dts={}", fragment.group_id, fragment.dts));
    }
    fn on_stop(&mut self) {
        self.recorder.events.lock().push("stop".into());
    }
}

struct LoggingFactory {
    recorder: Arc<Recorder>,
    builds: Arc<AtomicUsize>,
}

impl AgentFactory for LoggingFactory {
    fn name(&self) -> &str {
        "logging"
    }
    fn build(&self, _ctx: &AgentContext) -> Option<Box<dyn Agent>> {
        self.builds.fetch_add(1, Ordering::Relaxed);
        Some(Box::new(LoggingAgent {
            recorder: Arc::clone(&self.recorder),
        }))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_lifecycle_under_real_registry() {
    let registry = FragmentBroadcasterRegistry::new();
    let recorder = Arc::new(Recorder::default());
    let builds = Arc::new(AtomicUsize::new(0));

    // Hold the handle for the test's lifetime; drop at the end
    // (after assertions) so per-broadcast drain tasks finish
    // their on_stop work before any read of the recorder.
    let handle = AgentRunner::new()
        .with_factory(LoggingFactory {
            recorder: Arc::clone(&recorder),
            builds: Arc::clone(&builds),
        })
        .install(&registry);

    let meta = FragmentMeta::new("avc1.640028", 90_000);
    let bc = registry.get_or_create("live/cam1", "0.mp4", meta);

    // Emit two keyframes; the agent should observe both before
    // we drop the producer-side clone.
    let payload = Bytes::from_static(&[0x42; 64]);
    for i in 0..2u64 {
        bc.emit(Fragment::new(
            "0.mp4",
            i,
            0,
            0,
            i * 3000,
            i * 3000,
            3000,
            FragmentFlags::KEYFRAME,
            payload.clone(),
        ));
    }

    // Drop both producer-side clones so the BroadcasterStream
    // sees Closed and on_stop fires.
    drop(bc);
    registry.remove("live/cam1", "0.mp4");

    // Wait for the drain task to finish on_stop. 200ms is
    // generous on localhost.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let events = recorder.events.lock().clone();
    assert_eq!(builds.load(Ordering::Relaxed), 1, "factory built one agent");
    assert_eq!(events.len(), 4, "events: start + 2 frags + stop, got {events:?}");
    assert_eq!(events[0], "start broadcast=live/cam1 track=0.mp4");
    assert_eq!(events[1], "frag group=0 dts=0");
    assert_eq!(events[2], "frag group=1 dts=3000");
    assert_eq!(events[3], "stop");

    assert_eq!(handle.fragments_seen("logging", "live/cam1", "0.mp4"), 2);
    assert_eq!(handle.panics("logging", "live/cam1", "0.mp4"), 0);
}
