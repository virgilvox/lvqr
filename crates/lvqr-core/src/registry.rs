use crate::error::CoreError;
use crate::types::{Frame, SubscriberId, TrackName};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, trace, warn};

/// Default broadcast channel capacity per track.
/// At 30fps, 1024 frames is ~34 seconds of buffer.
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

/// The subscriber registry maps track names to broadcast channels.
///
/// When a publisher sends a frame, the registry fans it out to all subscribers
/// of that track via `tokio::sync::broadcast`. The broadcast channel uses
/// `bytes::Bytes` internally (via `Frame`), so each subscriber gets a cheap
/// ref-counted clone of the data -- zero copy on fanout.
///
/// Subscribers that fall behind (channel full) receive a `Lagged` error and
/// are expected to catch up from the GOP cache.
#[derive(Debug, Clone)]
pub struct Registry {
    /// Track name -> broadcast sender.
    /// Using DashMap for lock-free concurrent access.
    tracks: Arc<DashMap<TrackName, TrackState>>,
    /// Broadcast channel capacity per track.
    channel_capacity: usize,
}

#[derive(Debug)]
struct TrackState {
    sender: broadcast::Sender<Frame>,
    /// Number of active subscribers (for stats).
    subscriber_count: usize,
}

/// A subscription handle. Receives frames published to the track.
///
/// When dropped, the subscriber count is automatically decremented.
#[derive(Debug)]
pub struct Subscription {
    pub id: SubscriberId,
    pub track: TrackName,
    receiver: broadcast::Receiver<Frame>,
    tracks: Arc<DashMap<TrackName, TrackState>>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(mut entry) = self.tracks.get_mut(&self.track) {
            entry.subscriber_count = entry.subscriber_count.saturating_sub(1);
            debug!(subscriber = %self.id, track = %self.track, remaining = entry.subscriber_count, "subscription dropped");
        }
    }
}

impl Subscription {
    /// Receive the next frame. Blocks until a frame is available.
    ///
    /// Returns `Err(CoreError::SubscriberLagged)` if frames were dropped
    /// due to the subscriber being too slow.
    /// Returns `Err(CoreError::ChannelClosed)` if the publisher disconnected.
    pub async fn recv(&mut self) -> Result<Frame, CoreError> {
        loop {
            match self.receiver.recv().await {
                Ok(frame) => return Ok(frame),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        subscriber = %self.id,
                        track = %self.track,
                        skipped = n,
                        "subscriber lagged, skipped frames"
                    );
                    // Continue receiving from current position (auto-catch-up)
                    // The subscriber will get the next available frame.
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(CoreError::ChannelClosed);
                }
            }
        }
    }

    /// Try to receive the next frame without blocking.
    pub fn try_recv(&mut self) -> Result<Option<Frame>, CoreError> {
        match self.receiver.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                warn!(
                    subscriber = %self.id,
                    track = %self.track,
                    skipped = n,
                    "subscriber lagged on try_recv"
                );
                Ok(None)
            }
            Err(broadcast::error::TryRecvError::Closed) => Err(CoreError::ChannelClosed),
        }
    }
}

impl Registry {
    /// Create a new registry with default channel capacity.
    pub fn new() -> Self {
        Self {
            tracks: Arc::new(DashMap::new()),
            channel_capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }

    /// Create a new registry with custom channel capacity.
    pub fn with_capacity(channel_capacity: usize) -> Self {
        Self {
            tracks: Arc::new(DashMap::new()),
            channel_capacity,
        }
    }

    /// Publish a frame to a track. Creates the track if it doesn't exist.
    ///
    /// Returns the number of subscribers that received the frame.
    pub fn publish(&self, track: &TrackName, frame: Frame) -> usize {
        let entry = self.tracks.entry(track.clone()).or_insert_with(|| {
            let (sender, _) = broadcast::channel(self.channel_capacity);
            debug!(track = %track, "created new track");
            TrackState {
                sender,
                subscriber_count: 0,
            }
        });
        let sent = entry.sender.send(frame).unwrap_or(0);
        trace!(track = %track, receivers = sent, "published frame");
        sent
    }

    /// Subscribe to a track. Creates the track if it doesn't exist.
    pub fn subscribe(&self, track: &TrackName) -> Subscription {
        let id = SubscriberId::new();
        let mut entry = self.tracks.entry(track.clone()).or_insert_with(|| {
            let (sender, _) = broadcast::channel(self.channel_capacity);
            debug!(track = %track, "created new track for subscriber");
            TrackState {
                sender,
                subscriber_count: 0,
            }
        });
        entry.subscriber_count += 1;
        let receiver = entry.sender.subscribe();
        debug!(subscriber = %id, track = %track, "new subscription");
        Subscription {
            id,
            track: track.clone(),
            receiver,
            tracks: self.tracks.clone(),
        }
    }

    /// Unsubscribe from a track. Decrements the subscriber count.
    pub fn unsubscribe(&self, track: &TrackName, _id: SubscriberId) {
        if let Some(mut entry) = self.tracks.get_mut(track) {
            entry.subscriber_count = entry.subscriber_count.saturating_sub(1);
            debug!(track = %track, remaining = entry.subscriber_count, "unsubscribed");
        }
    }

    /// Remove a track entirely. Called when a publisher disconnects.
    pub fn remove_track(&self, track: &TrackName) {
        if self.tracks.remove(track).is_some() {
            debug!(track = %track, "removed track");
        }
    }

    /// Check if a track exists.
    pub fn has_track(&self, track: &TrackName) -> bool {
        self.tracks.contains_key(track)
    }

    /// Get the number of subscribers for a track.
    pub fn subscriber_count(&self, track: &TrackName) -> usize {
        self.tracks.get(track).map(|entry| entry.subscriber_count).unwrap_or(0)
    }

    /// Get the number of active tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// List all active track names.
    pub fn track_names(&self) -> Vec<TrackName> {
        self.tracks
            .iter()
            .map(|entry: dashmap::mapref::multiple::RefMulti<'_, TrackName, TrackState>| entry.key().clone())
            .collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_frame(seq: u64, keyframe: bool) -> Frame {
        Frame::new(seq, seq * 3000, keyframe, Bytes::from(vec![seq as u8; 100]))
    }

    #[tokio::test]
    async fn publish_and_subscribe() {
        let registry = Registry::new();
        let track = TrackName::new("live/test");

        let mut sub = registry.subscribe(&track);

        registry.publish(&track, make_frame(0, true));
        registry.publish(&track, make_frame(1, false));

        let f0 = sub.recv().await.unwrap();
        assert_eq!(f0.sequence, 0);
        assert!(f0.keyframe);

        let f1 = sub.recv().await.unwrap();
        assert_eq!(f1.sequence, 1);
        assert!(!f1.keyframe);
    }

    #[tokio::test]
    async fn fanout_to_multiple_subscribers() {
        let registry = Registry::new();
        let track = TrackName::new("live/multi");

        let mut sub1 = registry.subscribe(&track);
        let mut sub2 = registry.subscribe(&track);
        let mut sub3 = registry.subscribe(&track);

        let frame = make_frame(0, true);
        let sent = registry.publish(&track, frame);
        assert_eq!(sent, 3);

        // All three should receive the same data
        let f1 = sub1.recv().await.unwrap();
        let f2 = sub2.recv().await.unwrap();
        let f3 = sub3.recv().await.unwrap();

        assert_eq!(f1.payload, f2.payload);
        assert_eq!(f2.payload, f3.payload);
    }

    #[tokio::test]
    async fn subscriber_receives_only_after_subscribe() {
        let registry = Registry::new();
        let track = TrackName::new("live/timing");

        // Publish before subscribing
        registry.publish(&track, make_frame(0, true));

        // Subscribe after
        let mut sub = registry.subscribe(&track);

        // Publish another
        registry.publish(&track, make_frame(1, false));

        // Should only get frame 1 (subscribed after frame 0)
        let f = sub.recv().await.unwrap();
        assert_eq!(f.sequence, 1);
    }

    #[tokio::test]
    async fn channel_closed_on_track_removal() {
        let registry = Registry::new();
        let track = TrackName::new("live/remove");

        let mut sub = registry.subscribe(&track);
        registry.remove_track(&track);

        let result = sub.recv().await;
        assert!(result.is_err());
    }

    #[test]
    fn track_management() {
        let registry = Registry::new();
        let track = TrackName::new("live/manage");

        assert!(!registry.has_track(&track));
        assert_eq!(registry.track_count(), 0);

        let _sub = registry.subscribe(&track);
        assert!(registry.has_track(&track));
        assert_eq!(registry.track_count(), 1);
        assert_eq!(registry.subscriber_count(&track), 1);

        let _sub2 = registry.subscribe(&track);
        assert_eq!(registry.subscriber_count(&track), 2);

        registry.remove_track(&track);
        assert!(!registry.has_track(&track));
        assert_eq!(registry.track_count(), 0);
    }

    #[test]
    fn publish_to_empty_track_returns_zero() {
        let registry = Registry::new();
        let track = TrackName::new("live/empty");

        // Publishing to a track with no subscribers should work (creates track)
        let sent = registry.publish(&track, make_frame(0, true));
        assert_eq!(sent, 0);
        assert!(registry.has_track(&track));
    }

    #[tokio::test]
    async fn lagged_subscriber_continues() {
        let registry = Registry::with_capacity(4); // tiny buffer
        let track = TrackName::new("live/lag");

        let mut sub = registry.subscribe(&track);

        // Flood the channel way beyond capacity
        for i in 0..100 {
            registry.publish(&track, make_frame(i, i % 30 == 0));
        }

        // The subscriber should still be able to recv (skipping lagged frames)
        let frame = sub.recv().await.unwrap();
        // Should get some frame (not necessarily frame 0 due to lag)
        assert!(frame.sequence > 0);
    }

    #[tokio::test]
    async fn try_recv_empty() {
        let registry = Registry::new();
        let track = TrackName::new("live/tryrecv");

        let mut sub = registry.subscribe(&track);
        let result = sub.try_recv().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_track_names() {
        let registry = Registry::new();
        let _s1 = registry.subscribe(&TrackName::new("live/a"));
        let _s2 = registry.subscribe(&TrackName::new("live/b"));
        let _s3 = registry.subscribe(&TrackName::new("live/c"));

        let mut names: Vec<String> = registry.track_names().iter().map(|t| t.as_str().to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["live/a", "live/b", "live/c"]);
    }

    #[test]
    fn subscriber_count_decrements_on_drop() {
        let registry = Registry::new();
        let track = TrackName::new("live/drop");

        let sub = registry.subscribe(&track);
        assert_eq!(registry.subscriber_count(&track), 1);

        drop(sub);
        assert_eq!(registry.subscriber_count(&track), 0);
    }

    #[test]
    fn multiple_subscribers_drop_independently() {
        let registry = Registry::new();
        let track = TrackName::new("live/multi_drop");

        let sub1 = registry.subscribe(&track);
        let sub2 = registry.subscribe(&track);
        let sub3 = registry.subscribe(&track);
        assert_eq!(registry.subscriber_count(&track), 3);

        drop(sub1);
        assert_eq!(registry.subscriber_count(&track), 2);

        drop(sub2);
        assert_eq!(registry.subscriber_count(&track), 1);

        drop(sub3);
        assert_eq!(registry.subscriber_count(&track), 0);
    }
}
