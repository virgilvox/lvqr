use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("invalid token: {0}")]
    InvalidToken(String),
}
