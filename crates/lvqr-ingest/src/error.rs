use thiserror::Error;

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("connection error: {0}")]
    Connection(#[from] std::io::Error),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("stream key not found: {0}")]
    StreamKeyNotFound(String),

    #[error(transparent)]
    Core(#[from] lvqr_core::CoreError),
}
