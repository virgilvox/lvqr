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
use tokio_util::sync::CancellationToken;
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

/// Authentication callback: (app, stream_key) -> bool. Returns true to accept.
pub type AuthCallback = Arc<dyn Fn(&str, &str) -> bool + Send + Sync>;

/// SCTE-35 callback: (app, stream_key, raw splice_info_section bytes).
/// Fires for every `onCuePoint` AMF0 Data message whose `name` property
/// is `"scte35-bin64"` and whose `data` property base64-decodes to a
/// non-empty byte vector. The callee is responsible for further
/// parse / dispatch (typically [`crate::publish_scte35`] onto the
/// shared FragmentBroadcasterRegistry's `"scte35"` track).
///
/// Wired through the patched rml_rtmp `Amf0DataReceived` event variant
/// (see `vendor/rml_rtmp` and the session 152 close block).
pub type Scte35Callback = Arc<dyn Fn(&str, &str, Bytes) + Send + Sync>;

/// RTMP ingest server that translates RTMP streams to MoQ tracks.
pub struct RtmpServer {
    config: RtmpConfig,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
    /// Optional authentication: returns true to accept the publish stream key.
    /// `None` means open access.
    validate_publish: Option<AuthCallback>,
    /// Optional SCTE-35 onCuePoint scte35-bin64 callback. `None` means
    /// SCTE-35 ad markers are silently dropped (back-compat with
    /// session 151 and earlier callers).
    on_scte35: Option<Scte35Callback>,
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
            validate_publish: None,
            on_scte35: None,
        }
    }

    /// Create a new RTMP server from pre-wrapped callbacks.
    pub fn from_callbacks(
        config: RtmpConfig,
        on_video: MediaCallback,
        on_audio: MediaCallback,
        on_publish: StreamCallback,
        on_unpublish: StreamCallback,
    ) -> Self {
        Self {
            config,
            on_video,
            on_audio,
            on_publish,
            on_unpublish,
            validate_publish: None,
            on_scte35: None,
        }
    }

    /// Install an optional callback that validates publish requests by stream
    /// key. When the callback returns `false`, the publish is rejected and the
    /// connection is closed.
    pub fn set_validate_publish(&mut self, validate: AuthCallback) {
        self.validate_publish = Some(validate);
    }

    /// Install an optional callback for SCTE-35 onCuePoint scte35-bin64
    /// AMF0 Data messages. Each invocation receives the
    /// (app, stream_key, raw splice_info_section bytes) triple; the
    /// callee typically parses via [`lvqr_codec::parse_splice_info_section`]
    /// and emits onto the shared FragmentBroadcasterRegistry's
    /// `"scte35"` track via [`crate::publish_scte35`].
    pub fn set_scte35_callback(&mut self, cb: Scte35Callback) {
        self.on_scte35 = Some(cb);
    }

    pub fn config(&self) -> &RtmpConfig {
        &self.config
    }

    /// Run the RTMP ingest server. Blocks until the cancellation token fires.
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), IngestError> {
        let listener = TcpListener::bind(self.config.bind_addr).await?;
        info!(addr = %self.config.bind_addr, "RTMP ingest listening");
        self.run_with_listener(listener, shutdown).await
    }

    /// Run the RTMP ingest server on an already-bound `TcpListener`. Useful
    /// for tests that need to know the bound port before the server starts
    /// accepting connections (pre-bind at port 0, read `local_addr`, hand
    /// the listener to the server).
    pub async fn run_with_listener(
        &self,
        listener: TcpListener,
        shutdown: CancellationToken,
    ) -> Result<(), IngestError> {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    let (stream, peer_addr) = result?;
                    info!(%peer_addr, "RTMP connection accepted");
                    metrics::counter!("lvqr_rtmp_connections_total").increment(1);

                    let on_video = self.on_video.clone();
                    let on_audio = self.on_audio.clone();
                    let on_publish = self.on_publish.clone();
                    let on_unpublish = self.on_unpublish.clone();
                    let validate_publish = self.validate_publish.clone();
                    let on_scte35 = self.on_scte35.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_rtmp_connection(
                            stream,
                            on_video,
                            on_audio,
                            on_publish,
                            on_unpublish,
                            validate_publish,
                            on_scte35,
                        )
                        .await
                        {
                            error!(%peer_addr, error = %e, "RTMP session error");
                        }
                    });
                }
                _ = shutdown.cancelled() => {
                    info!("RTMP shutdown signal received");
                    break;
                }
            }
        }

        Ok(())
    }
}

/// Handle a single RTMP connection.
#[allow(clippy::too_many_arguments)]
async fn handle_rtmp_connection(
    mut stream: TcpStream,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
    validate_publish: Option<AuthCallback>,
    on_scte35: Option<Scte35Callback>,
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
                return handle_rtmp_session(
                    stream,
                    remaining_bytes,
                    on_video,
                    on_audio,
                    on_publish,
                    on_unpublish,
                    validate_publish,
                    on_scte35,
                )
                .await;
            }
        }
    }
}

/// Handle the post-handshake RTMP session.
#[allow(clippy::too_many_arguments)]
async fn handle_rtmp_session(
    mut stream: TcpStream,
    remaining_bytes: Vec<u8>,
    on_video: MediaCallback,
    on_audio: MediaCallback,
    on_publish: StreamCallback,
    on_unpublish: StreamCallback,
    validate_publish: Option<AuthCallback>,
    on_scte35: Option<Scte35Callback>,
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
                        // Authenticate publish stream key.
                        if let Some(ref validate) = validate_publish {
                            if !(validate)(app_name, stream_key) {
                                info!(
                                    app = %app_name,
                                    "RTMP publish rejected by auth provider"
                                );
                                metrics::counter!("lvqr_auth_failures_total", "entry" => "rtmp").increment(1);
                                return Ok(());
                            }
                        }
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
                    ServerSessionEvent::Amf0DataReceived {
                        app_name,
                        stream_key,
                        data,
                    } => {
                        if let Some(section) = parse_oncuepoint_scte35(data) {
                            metrics::counter!(
                                "lvqr_scte35_events_total",
                                "ingest" => "rtmp",
                                "command" => "oncuepoint",
                            )
                            .increment(1);
                            if let Some(ref cb) = on_scte35 {
                                (cb)(app_name, stream_key, section);
                            } else {
                                debug!(
                                    app = %app_name,
                                    key = %stream_key,
                                    "RTMP scte35-bin64 onCuePoint received but no callback installed; dropping"
                                );
                            }
                        } else {
                            debug!(
                                app = %app_name,
                                key = %stream_key,
                                first = ?data.first(),
                                "RTMP AMF0 data not an scte35-bin64 onCuePoint; ignoring"
                            );
                        }
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

/// Parse an `onCuePoint` AMF0 Data payload looking for a SCTE-35
/// `scte35-bin64` carriage. The Adobe convention for in-band SCTE-35
/// over RTMP is:
///
/// ```text
/// amf0_string("onCuePoint")
/// amf0_object {
///     "name" => "scte35-bin64",
///     "data" => "<base64-encoded splice_info_section>",
///     ... (optional "type", "time", "duration" keys)
/// }
/// ```
///
/// Returns the base64-decoded splice_info_section as `Bytes` when the
/// shape matches, or `None` for any other AMF0 Data carriage (which
/// the caller logs at debug and drops).
fn parse_oncuepoint_scte35(values: &[rml_amf0::Amf0Value]) -> Option<Bytes> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use rml_amf0::Amf0Value;

    if values.len() < 2 {
        return None;
    }
    let method = match &values[0] {
        Amf0Value::Utf8String(s) => s,
        _ => return None,
    };
    if method != "onCuePoint" {
        return None;
    }
    let obj = match &values[1] {
        Amf0Value::Object(props) => props,
        _ => return None,
    };
    let name = obj.get("name").and_then(|v| match v {
        Amf0Value::Utf8String(s) => Some(s.as_str()),
        _ => None,
    });
    if name != Some("scte35-bin64") {
        return None;
    }
    let b64 = obj.get("data").and_then(|v| match v {
        Amf0Value::Utf8String(s) => Some(s.as_str()),
        _ => None,
    })?;
    let decoded = STANDARD.decode(b64).ok()?;
    if decoded.is_empty() {
        return None;
    }
    Some(Bytes::from(decoded))
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

    fn mk_scte35_oncuepoint(b64: &str) -> Vec<rml_amf0::Amf0Value> {
        use rml_amf0::Amf0Value;
        use std::collections::HashMap;
        let mut obj = HashMap::new();
        obj.insert("name".into(), Amf0Value::Utf8String("scte35-bin64".into()));
        obj.insert("data".into(), Amf0Value::Utf8String(b64.into()));
        obj.insert("type".into(), Amf0Value::Utf8String("scte-35".into()));
        vec![Amf0Value::Utf8String("onCuePoint".into()), Amf0Value::Object(obj)]
    }

    #[test]
    fn parses_well_formed_oncuepoint_scte35_bin64() {
        // base64 of "FCsection..." -- shape only, parser does not validate
        // splice_info_section here (lvqr-codec does that).
        let b64 = "/DARAA=="; // [0xFC, 0x30, 0x11, 0x00] base64
        let values = mk_scte35_oncuepoint(b64);
        let raw = parse_oncuepoint_scte35(&values).expect("parses");
        assert_eq!(&raw[..], &[0xFC, 0x30, 0x11, 0x00]);
    }

    #[test]
    fn rejects_oncuepoint_without_scte35_name() {
        use rml_amf0::Amf0Value;
        use std::collections::HashMap;
        let mut obj = HashMap::new();
        obj.insert("name".into(), Amf0Value::Utf8String("other-cue".into()));
        obj.insert("data".into(), Amf0Value::Utf8String("/DARAA==".into()));
        let values = vec![Amf0Value::Utf8String("onCuePoint".into()), Amf0Value::Object(obj)];
        assert!(parse_oncuepoint_scte35(&values).is_none());
    }

    #[test]
    fn rejects_non_oncuepoint_method() {
        use rml_amf0::Amf0Value;
        let values = vec![Amf0Value::Utf8String("onMetaData".into()), Amf0Value::Null];
        assert!(parse_oncuepoint_scte35(&values).is_none());
    }

    #[test]
    fn rejects_empty_base64_payload() {
        let values = mk_scte35_oncuepoint("");
        assert!(parse_oncuepoint_scte35(&values).is_none());
    }

    #[test]
    fn rejects_invalid_base64() {
        let values = mk_scte35_oncuepoint("!!!not-valid-base64!!!");
        assert!(parse_oncuepoint_scte35(&values).is_none());
    }
}
