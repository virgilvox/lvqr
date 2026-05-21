//! In-process log capture for the admin live-tail endpoint.
//!
//! A [`CaptureLayer`] is composed into the global tracing subscriber by
//! [`crate::init`]. Every event that passes the `EnvFilter` is rendered into
//! a [`LogLine`] and (a) pushed onto a bounded ring buffer so a newly
//! connected client sees recent history, and (b) published on a
//! `tokio::sync::broadcast` channel so connected clients receive live lines.
//!
//! The broadcaster is process-global (mirroring tracing's own global
//! subscriber): [`init`](crate::init) installs it into a `OnceLock` that
//! [`log_broadcaster`] reads. The admin crate wires that handle into its
//! `GET /api/v1/logs` SSE route. When `init` was not called (e.g. tests using
//! a different subscriber), [`log_broadcaster`] returns `None` and the route
//! reports the feature unavailable.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

/// Default ring-buffer depth: the most recent N lines replayed to a new
/// subscriber as backlog.
const DEFAULT_RING_CAPACITY: usize = 512;
/// Default broadcast channel depth. A slow client that falls this far behind
/// gets a lagged signal and skips ahead rather than stalling the publisher.
const DEFAULT_CHANNEL_CAPACITY: usize = 256;

/// One captured log line in a transport-friendly shape. Serialized as JSON
/// on the SSE wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    /// Capture time in milliseconds since the Unix epoch.
    pub ts_ms: u64,
    /// Level: `"ERROR"`, `"WARN"`, `"INFO"`, `"DEBUG"`, or `"TRACE"`.
    pub level: String,
    /// Event target (usually the emitting module path).
    pub target: String,
    /// The rendered message plus any structured fields appended as
    /// `key=value`.
    pub message: String,
}

/// Cloneable handle to the process-global log capture. Cheap to clone (all
/// inner state is `Arc`-backed). Hand a clone to the admin route.
#[derive(Clone)]
pub struct LogBroadcaster {
    tx: broadcast::Sender<LogLine>,
    ring: Arc<Mutex<VecDeque<LogLine>>>,
    ring_capacity: usize,
}

impl LogBroadcaster {
    /// Build a broadcaster with the given ring + channel capacities.
    pub fn new(ring_capacity: usize, channel_capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(channel_capacity.max(1));
        Self {
            tx,
            ring: Arc::new(Mutex::new(VecDeque::with_capacity(ring_capacity))),
            ring_capacity: ring_capacity.max(1),
        }
    }

    /// Record a line: append to the ring (evicting the oldest past capacity)
    /// and publish to live subscribers. A send with no receivers is ignored.
    pub fn push(&self, line: LogLine) {
        if let Ok(mut ring) = self.ring.lock() {
            if ring.len() >= self.ring_capacity {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }
        let _ = self.tx.send(line);
    }

    /// Subscribe for live lines from this point forward.
    pub fn subscribe(&self) -> broadcast::Receiver<LogLine> {
        self.tx.subscribe()
    }

    /// Snapshot the current backlog (oldest first).
    pub fn snapshot(&self) -> Vec<LogLine> {
        self.ring
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Default for LogBroadcaster {
    fn default() -> Self {
        Self::new(DEFAULT_RING_CAPACITY, DEFAULT_CHANNEL_CAPACITY)
    }
}

static GLOBAL: OnceLock<LogBroadcaster> = OnceLock::new();

/// Install the process-global broadcaster. Called once by [`crate::init`].
/// Returns the installed handle (or the existing one if already set).
pub(crate) fn install_global() -> LogBroadcaster {
    GLOBAL.get_or_init(LogBroadcaster::default).clone()
}

/// The process-global log broadcaster, if [`crate::init`] installed one.
/// Returns `None` under a test subscriber that did not call `init`.
pub fn log_broadcaster() -> Option<LogBroadcaster> {
    GLOBAL.get().cloned()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Collects an event's `message` field plus any other fields into a single
/// string. The `message` is rendered first; remaining fields follow as
/// space-separated `key=value` pairs.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
}

impl Visit for LineVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            if !self.fields.is_empty() {
                self.fields.push(' ');
            }
            let _ = write!(self.fields, "{}={value:?}", field.name());
        }
    }
}

impl LineVisitor {
    fn into_message(self) -> String {
        match (self.message.is_empty(), self.fields.is_empty()) {
            (true, true) => String::new(),
            (true, false) => self.fields,
            (false, true) => self.message,
            (false, false) => format!("{} {}", self.message, self.fields),
        }
    }
}

/// Tracing layer that mirrors every passing event into a [`LogBroadcaster`].
pub struct CaptureLayer {
    broadcaster: LogBroadcaster,
}

impl CaptureLayer {
    pub(crate) fn new(broadcaster: LogBroadcaster) -> Self {
        Self { broadcaster }
    }
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        self.broadcaster.push(LogLine {
            ts_ms: now_ms(),
            level: meta.level().to_string(),
            target: meta.target().to_string(),
            message: visitor.into_message(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest_past_capacity() {
        let b = LogBroadcaster::new(2, 8);
        for i in 0..4 {
            b.push(LogLine {
                ts_ms: i,
                level: "INFO".into(),
                target: "t".into(),
                message: format!("m{i}"),
            });
        }
        let snap = b.snapshot();
        assert_eq!(snap.len(), 2, "ring capped at 2");
        assert_eq!(snap[0].message, "m2");
        assert_eq!(snap[1].message, "m3");
    }

    #[tokio::test]
    async fn subscriber_receives_pushed_lines() {
        let b = LogBroadcaster::new(8, 8);
        let mut rx = b.subscribe();
        b.push(LogLine {
            ts_ms: 1,
            level: "WARN".into(),
            target: "t".into(),
            message: "hello".into(),
        });
        let got = rx.recv().await.expect("recv");
        assert_eq!(got.level, "WARN");
        assert_eq!(got.message, "hello");
    }

    #[test]
    fn line_visitor_combines_message_and_fields() {
        // Construct directly (the real path goes through tracing's Visit
        // machinery, exercised by the layer in production).
        let v = LineVisitor {
            message: "started".into(),
            fields: "port=8080".into(),
        };
        assert_eq!(v.into_message(), "started port=8080");
    }
}
