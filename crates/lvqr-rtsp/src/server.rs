//! RTSP/1.0 TCP server with per-connection request handling.

use std::collections::HashMap;
use std::net::SocketAddr;

use lvqr_core::{EventBus, RelayEvent};
use lvqr_ingest::SharedFragmentObserver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::proto::{self, Method, Response, parse_transport};
use crate::rtp::{self, H264Depacketizer, parse_rtp_header};
use crate::session::{Session, SessionId, SessionMode, SessionState, generate_session_id, parse_sdp_tracks};

const SUPPORTED_METHODS: &str = "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, RECORD, TEARDOWN, GET_PARAMETER";

pub struct RtspServer {
    addr: SocketAddr,
}

impl RtspServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    pub async fn run(
        self,
        observer: Option<SharedFragmentObserver>,
        events: EventBus,
        shutdown: CancellationToken,
    ) -> Result<(), std::io::Error> {
        let listener = TcpListener::bind(self.addr).await?;
        let local_addr = listener.local_addr()?;
        info!(addr = %local_addr, "RTSP server bound");

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = listener.accept() => {
                    let (socket, remote) = match result {
                        Ok(v) => v,
                        Err(e) => {
                            warn!(error = %e, "RTSP accept failed");
                            continue;
                        }
                    };
                    info!(%remote, "RTSP connection accepted");
                    let obs = observer.clone();
                    let ev = events.clone();
                    let conn_shutdown = shutdown.clone();
                    let server_addr = local_addr;
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            socket, remote, server_addr, obs.as_ref(), &ev, conn_shutdown,
                        ).await {
                            debug!(%remote, error = %e, "RTSP connection ended with error");
                        }
                    });
                }
            }
        }

        Ok(())
    }
}

struct ConnectionState {
    sessions: HashMap<SessionId, Session>,
    server_addr: SocketAddr,
    h264_depack: H264Depacketizer,
    rtp_packet_count: u64,
}

async fn handle_connection(
    mut socket: TcpStream,
    remote: SocketAddr,
    server_addr: SocketAddr,
    _observer: Option<&SharedFragmentObserver>,
    events: &EventBus,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 8192];
    let mut read_buf = Vec::with_capacity(8192);
    let mut conn = ConnectionState {
        sessions: HashMap::new(),
        server_addr,
        h264_depack: H264Depacketizer::new(),
        rtp_packet_count: 0,
    };

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            n = socket.read(&mut buf) => {
                let n = n?;
                if n == 0 {
                    debug!(%remote, "RTSP connection closed by peer");
                    break;
                }
                read_buf.extend_from_slice(&buf[..n]);

                // Process all complete messages in the buffer.
                // Interleaved frames start with '$' (0x24); RTSP
                // requests start with an ASCII method name.
                loop {
                    if read_buf.is_empty() {
                        break;
                    }
                    if read_buf[0] == 0x24 {
                        // Interleaved RTP/RTCP frame.
                        match rtp::parse_interleaved_frame(&read_buf) {
                            Some((frame, consumed)) => {
                                process_rtp_frame(&mut conn, &frame);
                                read_buf.drain(..consumed);
                            }
                            None => break, // incomplete
                        }
                    } else {
                        match proto::parse_request(&read_buf) {
                            Ok((req, consumed)) => {
                                debug!(%remote, method = %req.method, uri = %req.uri, "RTSP request");
                                let resp = handle_request(&mut conn, &req);
                                socket.write_all(&resp.serialize()).await?;
                                read_buf.drain(..consumed);
                            }
                            Err(proto::ParseError::Incomplete) => break,
                            Err(e) => {
                                warn!(%remote, error = %e, "RTSP parse error");
                                let resp = Response::bad_request().with_cseq(0);
                                socket.write_all(&resp.serialize()).await?;
                                read_buf.clear();
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Emit BroadcastStopped for any active sessions.
    for session in conn.sessions.values() {
        if session.state == SessionState::Playing || session.state == SessionState::Recording {
            events.emit(RelayEvent::BroadcastStopped {
                name: session.broadcast.clone(),
            });
            info!(broadcast = %session.broadcast, "RTSP session ended, BroadcastStopped emitted");
        }
    }

    Ok(())
}

fn process_rtp_frame(conn: &mut ConnectionState, frame: &rtp::InterleavedFrame) {
    // Odd channels are RTCP -- skip for now.
    if frame.channel % 2 != 0 {
        return;
    }

    let Some(header) = parse_rtp_header(&frame.payload) else {
        return;
    };
    let rtp_payload = &frame.payload[header.header_len..];

    if let Some(result) = conn.h264_depack.depacketize(rtp_payload, &header) {
        conn.rtp_packet_count += 1;
        debug!(
            channel = frame.channel,
            ts = header.timestamp,
            nalus = result.nalus.len(),
            keyframe = result.keyframe,
            marker = result.marker,
            count = conn.rtp_packet_count,
            "RTSP RTP depacketized H.264"
        );
        // TODO: wire to fragment observer -- build Annex B from NALs,
        // extract SPS/PPS, emit init segment + moof/mdat fragments.
    }
}

fn handle_request(conn: &mut ConnectionState, req: &proto::Request) -> Response {
    let cseq = req.cseq().unwrap_or(0);
    match req.method {
        Method::Options => handle_options(cseq),
        Method::Describe => handle_describe(conn, req, cseq),
        Method::Announce => handle_announce(conn, req, cseq),
        Method::Setup => handle_setup(conn, req, cseq),
        Method::Play => handle_play(conn, req, cseq),
        Method::Record => handle_record(conn, req, cseq),
        Method::Teardown => handle_teardown(conn, req, cseq),
        Method::GetParameter => handle_get_parameter(conn, req, cseq),
        _ => Response::method_not_allowed().with_cseq(cseq),
    }
}

fn handle_options(cseq: u32) -> Response {
    Response::ok().with_cseq(cseq).with_header("Public", SUPPORTED_METHODS)
}

fn handle_describe(conn: &ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let broadcast = extract_broadcast(&req.uri);
    // Build a minimal SDP describing available tracks.
    // In a full implementation this would query the fragment observer
    // for active broadcasts and their codec parameters.
    let sdp = format!(
        "v=0\r\n\
         o=- 0 0 IN IP4 {}\r\n\
         s={broadcast}\r\n\
         t=0 0\r\n\
         a=control:*\r\n\
         m=video 0 RTP/AVP 96\r\n\
         a=rtpmap:96 H264/90000\r\n\
         a=control:track1\r\n",
        conn.server_addr.ip()
    );
    Response::ok()
        .with_cseq(cseq)
        .with_header("Content-Base", &req.uri)
        .with_body("application/sdp", sdp.into_bytes())
}

fn handle_announce(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let broadcast = extract_broadcast(&req.uri);
    let body_str = std::str::from_utf8(&req.body).unwrap_or("");
    let tracks = parse_sdp_tracks(body_str);

    let session_id = generate_session_id();
    let mut session = Session::new(session_id.clone(), SessionMode::Ingest, broadcast);
    session.tracks = tracks;
    conn.sessions.insert(session_id.clone(), session);

    info!(session = %session_id, "RTSP ANNOUNCE accepted");
    Response::ok().with_cseq(cseq).with_header("Session", &session_id)
}

fn handle_setup(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let control = extract_track_control(&req.uri);
    let transport_header = req.headers.get("Transport").unwrap_or("");
    let transport = parse_transport(transport_header);

    // Find or create the session.
    let session_id = if let Some(id) = req.session_id() {
        id.to_string()
    } else if let Some((id, _)) = conn.sessions.iter().next() {
        id.clone()
    } else {
        // Create a playback session on first SETUP without ANNOUNCE.
        let broadcast = extract_broadcast(&req.uri);
        let id = generate_session_id();
        conn.sessions
            .insert(id.clone(), Session::new(id.clone(), SessionMode::Playback, broadcast));
        id
    };

    let Some(session) = conn.sessions.get_mut(&session_id) else {
        return Response::session_not_found().with_cseq(cseq);
    };

    let interleaved = transport.interleaved.or(Some((0, 1)));
    session.setup_track(&control, interleaved);

    let (ch_a, ch_b) = interleaved.unwrap_or((0, 1));
    let resp_transport = format!("RTP/AVP/TCP;unicast;interleaved={ch_a}-{ch_b}");
    info!(session = %session_id, %control, "RTSP SETUP complete");

    Response::ok()
        .with_cseq(cseq)
        .with_header("Session", &format!("{session_id};timeout=60"))
        .with_header("Transport", &resp_transport)
}

fn handle_play(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let Some(session_id) = req.session_id() else {
        return Response::session_not_found().with_cseq(cseq);
    };
    let Some(session) = conn.sessions.get_mut(session_id) else {
        return Response::session_not_found().with_cseq(cseq);
    };
    if let Err(e) = session.play() {
        warn!(error = %e, "RTSP PLAY rejected");
        return Response::method_not_allowed().with_cseq(cseq);
    }
    info!(session = %session_id, broadcast = %session.broadcast, "RTSP PLAY started");
    Response::ok().with_cseq(cseq).with_header("Session", session_id)
}

fn handle_record(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let Some(session_id) = req.session_id() else {
        return Response::session_not_found().with_cseq(cseq);
    };
    let Some(session) = conn.sessions.get_mut(session_id) else {
        return Response::session_not_found().with_cseq(cseq);
    };
    if let Err(e) = session.record() {
        warn!(error = %e, "RTSP RECORD rejected");
        return Response::method_not_allowed().with_cseq(cseq);
    }
    info!(session = %session_id, broadcast = %session.broadcast, "RTSP RECORD started");
    Response::ok().with_cseq(cseq).with_header("Session", session_id)
}

fn handle_teardown(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    let Some(session_id) = req.session_id() else {
        return Response::session_not_found().with_cseq(cseq);
    };
    let session_id = session_id.to_string();
    conn.sessions.remove(&session_id);
    info!(session = %session_id, "RTSP TEARDOWN");
    Response::ok().with_cseq(cseq).with_header("Session", &session_id)
}

fn handle_get_parameter(conn: &mut ConnectionState, req: &proto::Request, cseq: u32) -> Response {
    // GET_PARAMETER with empty body is used as a keepalive.
    let _ = conn;
    let _ = req;
    Response::ok().with_cseq(cseq)
}

/// Extract broadcast name from an RTSP URI.
/// "rtsp://host:port/live/cam1" -> "live/cam1"
/// "rtsp://host:port/live/cam1/track1" -> "live/cam1"
fn extract_broadcast(uri: &str) -> String {
    let path = if let Some(rest) = uri.strip_prefix("rtsp://") {
        rest.find('/').map(|i| &rest[i + 1..]).unwrap_or("")
    } else {
        uri.trim_start_matches('/')
    };
    // Remove trailing track control suffix if present.
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() > 2 {
        // e.g. ["live", "cam1", "track1"] -> "live/cam1"
        parts[..parts.len() - 1].join("/")
    } else {
        path.to_string()
    }
}

/// Extract the track control name from a SETUP URI.
/// "rtsp://host:port/live/cam1/track1" -> "track1"
fn extract_track_control(uri: &str) -> String {
    let path = if let Some(rest) = uri.strip_prefix("rtsp://") {
        rest.find('/').map(|i| &rest[i + 1..]).unwrap_or("")
    } else {
        uri.trim_start_matches('/')
    };
    path.rsplit('/').next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_broadcast_from_uri() {
        assert_eq!(extract_broadcast("rtsp://localhost:8554/live/cam1"), "live/cam1");
        assert_eq!(
            extract_broadcast("rtsp://192.168.1.1:554/live/cam1/track1"),
            "live/cam1"
        );
    }

    #[test]
    fn extract_track_control_from_uri() {
        assert_eq!(
            extract_track_control("rtsp://localhost:8554/live/cam1/track1"),
            "track1"
        );
    }

    #[test]
    fn options_lists_methods() {
        let resp = handle_options(1);
        assert_eq!(resp.status, 200);
        let data = resp.serialize();
        let text = std::str::from_utf8(&data).unwrap();
        assert!(text.contains("Public:"));
        assert!(text.contains("DESCRIBE"));
        assert!(text.contains("SETUP"));
        assert!(text.contains("PLAY"));
    }

    #[test]
    fn describe_returns_sdp() {
        let conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            h264_depack: H264Depacketizer::new(),
            rtp_packet_count: 0,
        };
        let req = proto::Request {
            method: Method::Describe,
            uri: "rtsp://localhost:8554/live/test".into(),
            version: proto::RtspVersion::V1_0,
            headers: proto::Headers::new(),
            body: Vec::new(),
        };
        let resp = handle_describe(&conn, &req, 2);
        assert_eq!(resp.status, 200);
        let body = std::str::from_utf8(&resp.body).unwrap();
        assert!(body.contains("v=0"));
        assert!(body.contains("H264/90000"));
    }

    #[test]
    fn full_playback_handshake() {
        let mut conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            h264_depack: H264Depacketizer::new(),
            rtp_packet_count: 0,
        };

        // SETUP creates a session.
        let mut headers = proto::Headers::new();
        headers.insert("Transport".into(), "RTP/AVP/TCP;unicast;interleaved=0-1".into());
        let setup_req = proto::Request {
            method: Method::Setup,
            uri: "rtsp://localhost:8554/live/test/track1".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        };
        let resp = handle_setup(&mut conn, &setup_req, 3);
        assert_eq!(resp.status, 200);
        assert_eq!(conn.sessions.len(), 1);

        let session_id = conn.sessions.keys().next().unwrap().clone();
        assert_eq!(conn.sessions[&session_id].state, SessionState::Ready);

        // PLAY transitions to Playing.
        let mut headers = proto::Headers::new();
        headers.insert("Session".into(), session_id.clone());
        let play_req = proto::Request {
            method: Method::Play,
            uri: "rtsp://localhost:8554/live/test".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        };
        let resp = handle_play(&mut conn, &play_req, 4);
        assert_eq!(resp.status, 200);
        assert_eq!(conn.sessions[&session_id].state, SessionState::Playing);

        // TEARDOWN removes the session.
        let mut headers = proto::Headers::new();
        headers.insert("Session".into(), session_id.clone());
        let teardown_req = proto::Request {
            method: Method::Teardown,
            uri: "rtsp://localhost:8554/live/test".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        };
        let resp = handle_teardown(&mut conn, &teardown_req, 5);
        assert_eq!(resp.status, 200);
        assert!(conn.sessions.is_empty());
    }

    #[test]
    fn announce_record_handshake() {
        let mut conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            h264_depack: H264Depacketizer::new(),
            rtp_packet_count: 0,
        };

        // ANNOUNCE with SDP body.
        let sdp = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=Test\r\n\
                   m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=control:track1\r\n";
        let mut headers = proto::Headers::new();
        headers.insert("Content-Type".into(), "application/sdp".into());
        headers.insert("Content-Length".into(), sdp.len().to_string());
        let announce_req = proto::Request {
            method: Method::Announce,
            uri: "rtsp://localhost:8554/publish/cam1".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: sdp.as_bytes().to_vec(),
        };
        let resp = handle_announce(&mut conn, &announce_req, 1);
        assert_eq!(resp.status, 200);
        assert_eq!(conn.sessions.len(), 1);

        let session_id = conn.sessions.keys().next().unwrap().clone();
        let session = &conn.sessions[&session_id];
        assert_eq!(session.mode, SessionMode::Ingest);
        assert_eq!(session.tracks.len(), 1);

        // SETUP the video track.
        let mut headers = proto::Headers::new();
        headers.insert("Session".into(), session_id.clone());
        headers.insert("Transport".into(), "RTP/AVP/TCP;unicast;interleaved=0-1".into());
        let setup_req = proto::Request {
            method: Method::Setup,
            uri: "rtsp://localhost:8554/publish/cam1/track1".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        };
        let resp = handle_setup(&mut conn, &setup_req, 2);
        assert_eq!(resp.status, 200);

        // RECORD starts ingest.
        let mut headers = proto::Headers::new();
        headers.insert("Session".into(), session_id.clone());
        let record_req = proto::Request {
            method: Method::Record,
            uri: "rtsp://localhost:8554/publish/cam1".into(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        };
        let resp = handle_record(&mut conn, &record_req, 3);
        assert_eq!(resp.status, 200);
        assert_eq!(conn.sessions[&session_id].state, SessionState::Recording);
    }
}
