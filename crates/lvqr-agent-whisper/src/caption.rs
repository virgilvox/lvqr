//! [`TranscribedCaption`] + [`CaptionStream`].
//!
//! These types are always available regardless of the `whisper`
//! feature so downstream consumers (session 99 C's HLS subtitle
//! rendition wiring) can subscribe to a typed channel that
//! compiles whether or not whisper.cpp is linked in. Without the
//! `whisper` feature the agent never publishes any captions; the
//! channel is just empty.

use std::sync::Arc;
use tokio::sync::broadcast;

/// One transcribed caption segment.
///
/// Timestamps are in the source audio track's timescale (see
/// `lvqr_fragment::FragmentMeta::timescale`; for AAC-LC that is
/// the AAC sample rate). `start_ts` and `end_ts` are inclusive
/// bounds derived from the underlying fragment DTS values plus
/// whisper's per-segment timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribedCaption {
    /// Broadcast name in `<app>/<name>` form, e.g. `"live/cam1"`.
    pub broadcast: String,
    /// Caption start timestamp in the source track's timescale.
    pub start_ts: u64,
    /// Caption end timestamp in the source track's timescale.
    pub end_ts: u64,
    /// Transcribed text. UTF-8, no leading / trailing whitespace.
    pub text: String,
}

/// Default capacity of the `tokio::sync::broadcast` channel that
/// fan-outs [`TranscribedCaption`] values from the agent's
/// worker thread. Sized generously because captions are small
/// (a few hundred bytes max) and a slow subscriber should fall
/// back to lossy `Lagged` skips rather than block the worker.
pub const DEFAULT_CAPTION_CHANNEL_CAPACITY: usize = 256;

/// Public output channel the [`crate::WhisperCaptionsAgent`]
/// publishes captions onto.
///
/// Cheaply cloneable: the inner `broadcast::Sender` is
/// reference-counted. Subscribe via [`CaptionStream::subscribe`]
/// to receive every future caption from the moment of subscribe
/// onward. Subscribers that connect after a caption was emitted
/// do not see prior captions; that matches the
/// [`lvqr_fragment::BroadcasterStream`] semantics for the audio
/// fragment stream the agent is sourced from.
#[derive(Clone)]
pub struct CaptionStream {
    inner: Arc<broadcast::Sender<TranscribedCaption>>,
}

impl CaptionStream {
    /// Construct a new stream with [`DEFAULT_CAPTION_CHANNEL_CAPACITY`].
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPTION_CHANNEL_CAPACITY)
    }

    /// Construct a new stream with an explicit ring-buffer capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity.max(1));
        Self { inner: Arc::new(tx) }
    }

    /// Subscribe to every future caption.
    pub fn subscribe(&self) -> broadcast::Receiver<TranscribedCaption> {
        self.inner.subscribe()
    }

    /// Number of currently active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.inner.receiver_count()
    }

    /// Publish a caption to every subscriber. Returns the count
    /// of subscribers that received it (zero is normal when no
    /// downstream is connected yet; never an error).
    pub fn publish(&self, caption: TranscribedCaption) -> usize {
        self.inner.send(caption).unwrap_or_default()
    }
}

impl Default for CaptionStream {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_then_subscribe_receives_future_captions_only() {
        let stream = CaptionStream::new();
        let _early = stream.publish(TranscribedCaption {
            broadcast: "live/cam1".into(),
            start_ts: 0,
            end_ts: 1000,
            text: "before subscribe".into(),
        });
        let mut sub = stream.subscribe();
        let count = stream.publish(TranscribedCaption {
            broadcast: "live/cam1".into(),
            start_ts: 1000,
            end_ts: 2000,
            text: "after subscribe".into(),
        });
        assert_eq!(count, 1, "one live subscriber");
        let got = sub.recv().await.expect("caption");
        assert_eq!(got.text, "after subscribe");
    }

    #[test]
    fn publish_with_no_subscribers_is_a_no_op() {
        let stream = CaptionStream::new();
        let received = stream.publish(TranscribedCaption {
            broadcast: "live/cam1".into(),
            start_ts: 0,
            end_ts: 1000,
            text: "into the void".into(),
        });
        assert_eq!(received, 0, "no subscribers, no error");
    }

    #[test]
    fn clone_shares_state() {
        let a = CaptionStream::new();
        let b = a.clone();
        let _sub = b.subscribe();
        assert_eq!(a.subscriber_count(), 1, "clones share the underlying sender");
    }
}
