use thiserror::Error;

#[derive(Debug, Error)]
pub enum SignalError {
    #[error("peer not found: {0}")]
    PeerNotFound(String),

    #[error("invalid message: {0}")]
    InvalidMessage(String),

    #[error("websocket error: {0}")]
    WebSocket(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
