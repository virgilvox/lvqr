use thiserror::Error;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("transport error: {0}")]
    Transport(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("TLS configuration error: {0}")]
    Tls(String),

    #[error("bind error: {0}")]
    Bind(#[from] std::io::Error),

    #[error(transparent)]
    Core(#[from] lvqr_core::CoreError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
