#[cfg(feature = "rtmp")]
pub mod rtmp;

pub mod error;

pub use error::IngestError;

#[cfg(feature = "rtmp")]
pub use rtmp::{RtmpConfig, RtmpServer};
