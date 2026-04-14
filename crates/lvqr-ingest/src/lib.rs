#[cfg(feature = "rtmp")]
pub mod rtmp;

#[cfg(feature = "rtmp")]
pub mod bridge;

pub mod observer;

pub mod error;
pub mod protocol;
pub mod remux;

pub use error::IngestError;
pub use observer::{FragmentObserver, NoopFragmentObserver, SharedFragmentObserver};
pub use protocol::IngestProtocol;

#[cfg(feature = "rtmp")]
pub use bridge::RtmpMoqBridge;

#[cfg(feature = "rtmp")]
pub use protocol::RtmpIngest;

#[cfg(feature = "rtmp")]
pub use rtmp::{RtmpConfig, RtmpServer};
