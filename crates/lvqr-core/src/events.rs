//! Event bus for relay lifecycle hooks.
//!
//! `EventBus` wraps a `tokio::sync::broadcast` channel that emits high-level
//! events about broadcasts and viewers. Subscribers receive a stream of
//! `RelayEvent` values they can act on (e.g. start a recording, post to a
//! webhook, update a dashboard).
//!
//! This is a deliberately small surface so we can grow it without churn.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Default event channel capacity. Sized large enough that slow subscribers
/// can lag a few seconds without causing publishers to drop events.
pub const DEFAULT_EVENT_CAPACITY: usize = 256;

/// High-level events emitted by LVQR subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayEvent {
    /// A new broadcast started (publisher connected).
    BroadcastStarted { name: String },
    /// A broadcast ended (publisher disconnected).
    BroadcastStopped { name: String },
    /// A viewer connected to the relay.
    ViewerJoined { broadcast: String, viewer_id: String },
    /// A viewer disconnected.
    ViewerLeft { broadcast: String, viewer_id: String },
}

/// Channel-based event bus. Cheap to clone (just an `Arc` internally).
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<RelayEvent>,
}

impl EventBus {
    /// Create an event bus with a custom channel capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Emit an event. If there are no subscribers, the event is dropped.
    pub fn emit(&self, event: RelayEvent) {
        let _ = self.sender.send(event);
    }

    /// Subscribe to events. Each subscriber sees events emitted after the
    /// subscription is created.
    pub fn subscribe(&self) -> broadcast::Receiver<RelayEvent> {
        self.sender.subscribe()
    }

    /// Number of currently active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_EVENT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emit_and_receive() {
        let bus = EventBus::default();
        let mut rx = bus.subscribe();
        bus.emit(RelayEvent::BroadcastStarted {
            name: "live/test".into(),
        });
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, RelayEvent::BroadcastStarted { name } if name == "live/test"));
    }

    #[tokio::test]
    async fn multiple_subscribers_get_same_event() {
        let bus = EventBus::default();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        bus.emit(RelayEvent::ViewerJoined {
            broadcast: "live/x".into(),
            viewer_id: "v1".into(),
        });
        let _ = a.recv().await.unwrap();
        let _ = b.recv().await.unwrap();
    }

    #[tokio::test]
    async fn no_subscribers_does_not_panic() {
        let bus = EventBus::default();
        bus.emit(RelayEvent::BroadcastStopped {
            name: "live/test".into(),
        });
    }

    #[test]
    fn event_serialization_round_trip() {
        let event = RelayEvent::BroadcastStarted {
            name: "live/test".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("broadcast_started"));
        let parsed: RelayEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RelayEvent::BroadcastStarted { .. }));
    }
}
