/// RTMP ingest server.
///
/// Accepts RTMP connections from OBS/ffmpeg, extracts video/audio data,
/// and publishes them as MoQ tracks via an OriginProducer.
use crate::error::IngestError;
use bytes::Bytes;
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, error, info};

/// Configuration for the RTMP ingest server.
#[derive(Debug, Clone)]
pub struct RtmpConfig {
    /// Address to bind the TCP listener (default: 0.0.0.0:1935).
    pub bind_addr: SocketAddr,
}

impl Default for RtmpConfig {
    fn default() -> Self {
        Self {
            bind_addr: ([0, 0, 0, 0], 1935).into(),
        }
    }
}

/// Callback for video/audio data: (app_name, stream_key, data, timestamp).
pub type MediaCallback = Arc<dyn Fn(&str, &str, Bytes, u32) + Send + Sync>;

/// Callback for publish/unpublish events: (app_name, stream_key).
pub type StreamCallback = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// RTMP ingest server that translates RTMP streams to MoQ tracks.
pub struct RtmpServer {
    config: RtmpConfig,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
}

impl RtmpServer {
    /// Create a new RTMP server with callbacks for media events.
    ///
    /// - `on_video(app, key, data, timestamp)`: called when video data is received
    /// - `on_audio(app, key, data, timestamp)`: called when audio data is received
    /// - `on_publish(app, key)`: called when a publisher starts streaming
    /// - `on_unpublish(app, key)`: called when a publisher stops streaming
    pub fn new(
        config: RtmpConfig,
        on_video: impl Fn(&str, &str, Bytes, u32) + Send + Sync + 'static,
        on_audio: impl Fn(&str, &str, Bytes, u32) + Send + Sync + 'static,
        on_publish: impl Fn(&str, &str) + Send + Sync + 'static,
        on_unpublish: impl Fn(&str, &str) + Send + Sync + 'static,
    ) -> Self {
        Self {
            config,
            on_video: Arc::new(on_video),
            on_audio: Arc::new(on_audio),
            on_publish: Arc::new(on_publish),
            on_unpublish: Arc::new(on_unpublish),
        }
    }

    pub fn config(&self) -> &RtmpConfig {
        &self.config
    }

    /// Run the RTMP ingest server. Blocks until shutdown.
    pub async fn run(&self) -> Result<(), IngestError> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(addr = %self.config.bind_addr, "RTMP ingest listening");

        loop {
            let (stream, peer_addr) = listener.accept().await?;
            info!(%peer_addr, "RTMP connection accepted");

            let on_video = self.on_video.clone();
            let on_audio = self.on_audio.clone();
            let on_publish = self.on_publish.clone();
            let on_unpublish = self.on_unpublish.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_rtmp_connection(stream, on_video, on_audio, on_publish, on_unpublish).await {
                    error!(%peer_addr, error = %e, "RTMP session error");
                }
            });
        }
    }
}

/// Handle a single RTMP connection.
async fn handle_rtmp_connection(
    mut stream: TcpStream,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
) -> Result<(), IngestError> {
    // Phase 1: RTMP Handshake
    let mut handshake = Handshake::new(PeerType::Server);
    let mut buf = vec![0u8; 4096];

    // The handshake needs initial server bytes sent first
    let p0_and_p1 = handshake
        .generate_outbound_p0_and_p1()
        .map_err(|e| IngestError::Protocol(format!("handshake generate error: {e:?}")))?;
    stream.write_all(&p0_and_p1).await?;

    // Process incoming handshake bytes
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Err(IngestError::Protocol("connection closed during handshake".into()));
        }

        match handshake
            .process_bytes(&buf[..n])
            .map_err(|e| IngestError::Protocol(format!("handshake error: {e:?}")))?
        {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await?;
                }
                debug!("RTMP handshake complete");

                // Phase 2: RTMP Session
                return handle_rtmp_session(stream, remaining_bytes, on_video, on_audio, on_publish, on_unpublish)
                    .await;
            }
        }
    }
}

/// Handle the post-handshake RTMP session.
async fn handle_rtmp_session(
    mut stream: TcpStream,
    remaining_bytes: Vec<u8>,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
) -> Result<(), IngestError> {
    let config = ServerSessionConfig::new();
    let (mut session, initial_results) =
        ServerSession::new(config).map_err(|e| IngestError::Protocol(format!("session init error: {e:?}")))?;

    // Send initial server responses (chunk size, window ack, etc.)
    for result in initial_results {
        if let ServerSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await?;
        }
    }

    // Process any remaining bytes from the handshake
    if !remaining_bytes.is_empty() {
        let results = session
            .handle_input(&remaining_bytes)
            .map_err(|e| IngestError::Protocol(format!("session input error: {e:?}")))?;
        process_session_results(
            &mut stream,
            &session,
            &results,
            &on_video,
            &on_audio,
            &on_publish,
            &on_unpublish,
        )
        .await?;
    }

    // Main read loop
    let mut buf = vec![0u8; 65536]; // 64KB buffer for media data
    let mut current_app = String::new();
    let mut current_key = String::new();

    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            // Connection closed
            if !current_app.is_empty() && !current_key.is_empty() {
                (on_unpublish)(&current_app, &current_key);
            }
            return Ok(());
        }

        let results = session
            .handle_input(&buf[..n])
            .map_err(|e| IngestError::Protocol(format!("session input error: {e:?}")))?;

        for result in &results {
            match result {
                ServerSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await?;
                }
                ServerSessionResult::RaisedEvent(event) => match event {
                    ServerSessionEvent::ConnectionRequested { request_id, app_name } => {
                        info!(app = %app_name, "RTMP connection requested");
                        current_app = app_name.clone();
                        let accept_results = session
                            .accept_request(*request_id)
                            .map_err(|e| IngestError::Protocol(format!("accept error: {e:?}")))?;
                        for r in &accept_results {
                            if let ServerSessionResult::OutboundResponse(p) = r {
                                stream.write_all(&p.bytes).await?;
                            }
                        }
                    }
                    ServerSessionEvent::PublishStreamRequested {
                        request_id,
                        app_name,
                        stream_key,
                        ..
                    } => {
                        info!(app = %app_name, key = %stream_key, "RTMP publish requested");
                        current_key = stream_key.clone();
                        let accept_results = session
                            .accept_request(*request_id)
                            .map_err(|e| IngestError::Protocol(format!("accept error: {e:?}")))?;
                        for r in &accept_results {
                            if let ServerSessionResult::OutboundResponse(p) = r {
                                stream.write_all(&p.bytes).await?;
                            }
                        }
                        (on_publish)(app_name, stream_key);
                    }
                    ServerSessionEvent::VideoDataReceived {
                        app_name,
                        stream_key,
                        data,
                        timestamp,
                    } => {
                        (on_video)(app_name, stream_key, data.clone(), timestamp.value);
                    }
                    ServerSessionEvent::AudioDataReceived {
                        app_name,
                        stream_key,
                        data,
                        timestamp,
                    } => {
                        (on_audio)(app_name, stream_key, data.clone(), timestamp.value);
                    }
                    ServerSessionEvent::PublishStreamFinished { app_name, stream_key } => {
                        info!(app = %app_name, key = %stream_key, "RTMP publish finished");
                        (on_unpublish)(app_name, stream_key);
                        current_key.clear();
                    }
                    ServerSessionEvent::StreamMetadataChanged {
                        app_name,
                        stream_key,
                        metadata,
                    } => {
                        debug!(
                            app = %app_name,
                            key = %stream_key,
                            video_width = ?metadata.video_width,
                            video_height = ?metadata.video_height,
                            video_codec_id = ?metadata.video_codec_id,
                            audio_codec_id = ?metadata.audio_codec_id,
                            "stream metadata received"
                        );
                    }
                    _ => {
                        debug!(event = ?event, "unhandled RTMP event");
                    }
                },
                ServerSessionResult::UnhandleableMessageReceived(_) => {
                    debug!("received unhandleable RTMP message");
                }
            }
        }
    }
}

/// Process a batch of session results (helper to avoid deep nesting).
async fn process_session_results(
    stream: &mut TcpStream,
    _session: &ServerSession,
    results: &[ServerSessionResult],
    _on_video: &MediaCallback,
    _on_audio: &MediaCallback,
    _on_publish: &StreamCallback,
    _on_unpublish: &StreamCallback,
) -> Result<(), IngestError> {
    for result in results {
        if let ServerSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await?;
        }
    }
    Ok(())
}

/// Check if an FLV video tag represents a keyframe.
///
/// FLV video tag format: first byte contains frame type (upper nibble) and codec ID (lower nibble).
/// Frame type 1 = keyframe, codec ID 7 = AVC (H.264).
pub fn is_keyframe(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    (data[0] >> 4) == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_detection() {
        // FLV frame type 1 (keyframe), codec 7 (AVC) = 0x17
        assert!(is_keyframe(&[0x17, 0x01, 0x00, 0x00]));
        // FLV frame type 2 (inter frame), codec 7 (AVC) = 0x27
        assert!(!is_keyframe(&[0x27, 0x01, 0x00, 0x00]));
        // Empty data
        assert!(!is_keyframe(&[]));
        // Frame type 1, codec 4 (VP6) = 0x14
        assert!(is_keyframe(&[0x14, 0x00]));
    }

    #[test]
    fn default_config() {
        let config = RtmpConfig::default();
        assert_eq!(config.bind_addr.port(), 1935);
    }
}
