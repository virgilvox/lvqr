/// RTMP ingest server.
///
/// Accepts RTMP connections from OBS/ffmpeg, extracts H.264 NALUs,
/// and publishes them as MoQ tracks via the Registry.
use lvqr_core::Registry;
use std::sync::Arc;

/// Configuration for the RTMP ingest server.
#[derive(Debug, Clone)]
pub struct RtmpConfig {
    /// TCP port to listen on (default: 1935).
    pub port: u16,
}

impl Default for RtmpConfig {
    fn default() -> Self {
        Self { port: 1935 }
    }
}

/// RTMP ingest server that translates RTMP streams to MoQ tracks.
pub struct RtmpServer {
    config: RtmpConfig,
    registry: Arc<Registry>,
}

impl RtmpServer {
    pub fn new(config: RtmpConfig, registry: Arc<Registry>) -> Self {
        Self { config, registry }
    }

    pub fn config(&self) -> &RtmpConfig {
        &self.config
    }

    /// Get a reference to the shared registry.
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }
}
