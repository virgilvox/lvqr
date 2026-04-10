use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STREAM_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(1);

/// Unique identifier for a stream (publisher session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamId(u64);

impl StreamId {
    pub fn new() -> Self {
        Self(NEXT_STREAM_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl Default for StreamId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream-{}", self.0)
    }
}

/// Unique identifier for a subscriber connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubscriberId(u64);

impl SubscriberId {
    pub fn new() -> Self {
        Self(NEXT_SUBSCRIBER_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl Default for SubscriberId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SubscriberId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sub-{}", self.0)
    }
}

/// A track name identifies a media track within a broadcast.
/// e.g., "live/mystream" or "live/mystream/video"
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackName(String);

impl TrackName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TrackName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TrackName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TrackName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A single media frame with metadata.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Monotonically increasing sequence number within the track.
    pub sequence: u64,
    /// Presentation timestamp in timebase units (90kHz typical for video).
    pub timestamp: u64,
    /// Whether this frame is a keyframe (starts a new GOP).
    pub keyframe: bool,
    /// The frame payload. Uses `bytes::Bytes` for ref-counted zero-copy sharing.
    pub payload: Bytes,
}

impl Frame {
    pub fn new(sequence: u64, timestamp: u64, keyframe: bool, payload: Bytes) -> Self {
        Self {
            sequence,
            timestamp,
            keyframe,
            payload,
        }
    }

    /// Size of the payload in bytes.
    pub fn size(&self) -> usize {
        self.payload.len()
    }
}

/// A Group of Pictures: a keyframe followed by its dependent frames.
#[derive(Debug, Clone)]
pub struct Gop {
    /// The sequence number of this GOP (the keyframe's group sequence).
    pub sequence: u64,
    /// Frames in this GOP, starting with the keyframe.
    pub frames: Vec<Frame>,
}

impl Gop {
    pub fn new(sequence: u64) -> Self {
        Self {
            sequence,
            frames: Vec::new(),
        }
    }

    /// Add a frame to this GOP.
    pub fn push(&mut self, frame: Frame) {
        self.frames.push(frame);
    }

    /// Total payload size across all frames.
    pub fn total_size(&self) -> usize {
        self.frames.iter().map(|f| f.size()).sum()
    }

    /// Number of frames in this GOP.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Snapshot of relay statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelayStats {
    /// Number of active publishers.
    pub publishers: u64,
    /// Number of active subscribers.
    pub subscribers: u64,
    /// Number of active tracks.
    pub tracks: u64,
    /// Total bytes received from publishers.
    pub bytes_received: u64,
    /// Total bytes sent to subscribers.
    pub bytes_sent: u64,
    /// Uptime in seconds.
    pub uptime_secs: u64,
}
