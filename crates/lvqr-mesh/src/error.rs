use thiserror::Error;

#[derive(Debug, Error)]
pub enum MeshError {
    #[error("peer not found: {0}")]
    PeerNotFound(String),

    #[error("mesh is full (max peers: {max})")]
    MeshFull { max: usize },

    #[error("tree depth exceeded (max: {max})")]
    TreeDepthExceeded { max: usize },

    #[error("no available parent for new peer")]
    NoAvailableParent,
}
