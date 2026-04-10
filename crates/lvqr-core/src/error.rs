use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("track not found: {0}")]
    TrackNotFound(String),

    #[error("subscriber lagged behind, dropped")]
    SubscriberLagged,

    #[error("channel closed")]
    ChannelClosed,

    #[error("ring buffer full (capacity: {capacity})")]
    RingBufferFull { capacity: usize },

    #[error("GOP cache miss: no complete GOP available")]
    GopCacheMiss,
}
