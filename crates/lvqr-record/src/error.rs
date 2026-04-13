use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecordError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("track not found: {0}")]
    TrackNotFound(String),

    #[error("recording stopped")]
    Stopped,
}
