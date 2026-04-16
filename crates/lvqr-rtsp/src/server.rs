//! RTSP/1.0 TCP server with per-connection request handling.

use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{
    HevcInitParams, RawSample, VideoInitParams, build_moof_mdat, write_avc_init_segment, write_hevc_init_segment,
};
use lvqr_codec::hevc as hevc_codec;
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentFlags};
use lvqr_ingest::SharedFragmentObserver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::proto::{self, Method, Response, parse_transport};
use crate::rtp::{self, H264Depacketizer, HevcDepacketizer, parse_rtp_header};
use crate::session::{
    Session, SessionId, SessionMode, SessionState, TrackCodec, generate_session_id, parse_sdp_tracks,
};

const SUPPORTED_METHODS: &str = "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, RECORD, TEARDOWN, GET_PARAMETER";

pub struct RtspServer {
    addr: SocketAddr,
    pre_bound: Option<TcpListener>,
}

impl RtspServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr, pre_bound: None }
    }

    /// Pre-bind the TCP listener and return the actual local address.
    /// Use this when the configured address has port 0 (ephemeral)
    /// and the caller needs to know the real port before `run` is
    /// called (e.g. in tests or for the `ServerHandle`).
    pub async fn bind(&mut self) -> Result<SocketAddr, std::io::Error> {
        let listener = TcpListener::bind(self.addr).await?;
        let bound = listener.local_addr()?;
        self.addr = bound;
        self.pre_bound = Some(listener);
        Ok(bound)
    }

    pub async fn run(
        self,
        observer: Option<SharedFragmentObserver>,
        events: EventBus,
        shutdown: CancellationToken,
    ) -> Result<(), std::io::Error> {
        let listener = match self.pre_bound {
            Some(l) => l,
            None => TcpListener::bind(self.addr).await?,
        };
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
    hevc_depack: HevcDepacketizer,
    rtp_packet_count: u64,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    vps: Option<Vec<u8>>,
    video_init_emitted: bool,
    video_seq: u64,
    prev_video_dts: Option<u64>,
}

async fn handle_connection(
    mut socket: TcpStream,
    remote: SocketAddr,
    server_addr: SocketAddr,
    observer: Option<&SharedFragmentObserver>,
    events: &EventBus,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 8192];
    let mut read_buf = Vec::with_capacity(8192);
    let mut conn = ConnectionState {
        sessions: HashMap::new(),
        server_addr,
        h264_depack: H264Depacketizer::new(),
        hevc_depack: HevcDepacketizer::new(),
        rtp_packet_count: 0,
        sps: None,
        pps: None,
        vps: None,
        video_init_emitted: false,
        video_seq: 0,
        prev_video_dts: None,
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
                                process_rtp_frame(&mut conn, &frame, observer);
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

fn process_rtp_frame(
    conn: &mut ConnectionState,
    frame: &rtp::InterleavedFrame,
    observer: Option<&SharedFragmentObserver>,
) {
    // Odd channels are RTCP -- skip for now.
    if frame.channel % 2 != 0 {
        return;
    }

    let Some(header) = parse_rtp_header(&frame.payload) else {
        return;
    };
    let rtp_payload = &frame.payload[header.header_len..];

    // Determine codec from the session's SDP tracks. Default to H.264
    // when no ANNOUNCE was received (e.g. playback SETUP without SDP).
    let codec = conn
        .sessions
        .values()
        .find(|s| s.state == SessionState::Recording)
        .and_then(|s| {
            s.tracks
                .iter()
                .find(|t| t.media_type == crate::session::MediaType::Video)
        })
        .map(|t| t.codec)
        .unwrap_or(TrackCodec::H264);

    let result = match codec {
        TrackCodec::H265 => conn.hevc_depack.depacketize(rtp_payload, &header),
        _ => conn.h264_depack.depacketize(rtp_payload, &header),
    };
    let Some(result) = result else {
        return;
    };

    conn.rtp_packet_count += 1;
    let codec_label = if codec == TrackCodec::H265 { "H.265" } else { "H.264" };
    debug!(
        channel = frame.channel,
        ts = header.timestamp,
        nalus = result.nalus.len(),
        keyframe = result.keyframe,
        marker = result.marker,
        count = conn.rtp_packet_count,
        codec = codec_label,
        "RTSP RTP depacketized"
    );

    let obs = match observer {
        Some(o) => o,
        None => return,
    };

    let broadcast = match conn
        .sessions
        .values()
        .find(|s| s.state == SessionState::Recording)
        .map(|s| s.broadcast.clone())
    {
        Some(b) => b,
        None => return,
    };

    match codec {
        TrackCodec::H265 => process_hevc_nalus(conn, &broadcast, &result, obs),
        _ => process_h264_nalus(conn, &broadcast, &result, obs),
    }
}

fn process_h264_nalus(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    obs: &SharedFragmentObserver,
) {
    for nalu in &result.nalus {
        if nalu.is_empty() {
            continue;
        }
        let nal_type = nalu[0] & 0x1F;
        match nal_type {
            7 => conn.sps = Some(nalu.clone()),
            8 => conn.pps = Some(nalu.clone()),
            _ => {}
        }
    }

    if !conn.video_init_emitted {
        let (Some(sps), Some(pps)) = (&conn.sps, &conn.pps) else {
            return;
        };
        let params = VideoInitParams {
            width: 0,
            height: 0,
            sps: sps.clone(),
            pps: pps.clone(),
            timescale: 90_000,
        };
        let mut buf = BytesMut::with_capacity(512);
        if let Err(e) = write_avc_init_segment(&mut buf, &params) {
            warn!(%broadcast, error = %e, "RTSP: failed to write AVC init segment");
            return;
        }
        obs.on_init(broadcast, "0.mp4", 90_000, buf.freeze());
        conn.video_init_emitted = true;
        info!(%broadcast, "RTSP: H.264 video init emitted");
    }

    let avcc = nals_to_length_prefixed(&result.nalus, NalFilter::H264);
    if avcc.is_empty() {
        return;
    }
    emit_video_fragment(conn, broadcast, result, avcc, obs);
}

fn process_hevc_nalus(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    obs: &SharedFragmentObserver,
) {
    for nalu in &result.nalus {
        if nalu.len() < 2 {
            continue;
        }
        let nal_type = (nalu[0] >> 1) & 0x3F;
        match nal_type {
            32 => conn.vps = Some(nalu.clone()),
            33 => conn.sps = Some(nalu.clone()),
            34 => conn.pps = Some(nalu.clone()),
            _ => {}
        }
    }

    if !conn.video_init_emitted {
        let (Some(vps), Some(sps), Some(pps)) = (&conn.vps, &conn.sps, &conn.pps) else {
            return;
        };
        let sps_info = match hevc_codec::parse_sps(sps) {
            Ok(info) => info,
            Err(e) => {
                warn!(%broadcast, error = %e, "RTSP: failed to parse HEVC SPS");
                return;
            }
        };
        let params = HevcInitParams {
            vps: vps.clone(),
            sps: sps.clone(),
            pps: pps.clone(),
            sps_info,
            timescale: 90_000,
        };
        let mut buf = BytesMut::with_capacity(512);
        if let Err(e) = write_hevc_init_segment(&mut buf, &params) {
            warn!(%broadcast, error = %e, "RTSP: failed to write HEVC init segment");
            return;
        }
        obs.on_init(broadcast, "0.mp4", 90_000, buf.freeze());
        conn.video_init_emitted = true;
        info!(%broadcast, "RTSP: HEVC video init emitted");
    }

    let hvcc = nals_to_length_prefixed(&result.nalus, NalFilter::Hevc);
    if hvcc.is_empty() {
        return;
    }
    emit_video_fragment(conn, broadcast, result, hvcc, obs);
}

fn emit_video_fragment(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    payload: Vec<u8>,
    obs: &SharedFragmentObserver,
) {
    let dts = result.timestamp as u64;
    let keyframe = result.keyframe;
    let duration = match conn.prev_video_dts {
        Some(prev) if dts > prev => (dts - prev) as u32,
        _ => 3000,
    };
    conn.prev_video_dts = Some(dts);

    let raw = RawSample {
        track_id: 1,
        dts,
        cts_offset: 0,
        duration,
        payload: Bytes::from(payload),
        keyframe,
    };
    conn.video_seq += 1;
    let moof_mdat = build_moof_mdat(conn.video_seq as u32, 1, dts, &[raw]);
    let frag = Fragment::new(
        "0.mp4",
        conn.video_seq,
        0,
        0,
        dts,
        dts,
        duration as u64,
        if keyframe {
            FragmentFlags::KEYFRAME
        } else {
            FragmentFlags::DELTA
        },
        moof_mdat,
    );
    obs.on_fragment(broadcast, "0.mp4", &frag);
}

#[derive(Clone, Copy)]
enum NalFilter {
    H264,
    Hevc,
}

/// Convert depacketized NAL units to length-prefixed format, stripping
/// parameter sets that belong in the init segment.
fn nals_to_length_prefixed(nalus: &[Vec<u8>], filter: NalFilter) -> Vec<u8> {
    let total: usize = nalus.iter().map(|n| n.len() + 4).sum();
    let mut out = Vec::with_capacity(total);
    for nalu in nalus {
        if nalu.is_empty() {
            continue;
        }
        let skip = match filter {
            NalFilter::H264 => {
                let t = nalu[0] & 0x1F;
                t == 7 || t == 8 // SPS, PPS
            }
            NalFilter::Hevc => {
                if nalu.len() < 2 {
                    true
                } else {
                    let t = (nalu[0] >> 1) & 0x3F;
                    t == 32 || t == 33 || t == 34 // VPS, SPS, PPS
                }
            }
        };
        if skip {
            continue;
        }
        let len = nalu.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nalu);
    }
    out
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
            hevc_depack: HevcDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
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
            hevc_depack: HevcDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
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
            hevc_depack: HevcDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
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

    #[test]
    fn nals_to_length_prefixed_strips_sps_pps() {
        let sps = vec![0x67, 0x42, 0x00, 0x1F]; // NAL type 7
        let pps = vec![0x68, 0xCE, 0x38, 0x80]; // NAL type 8
        let idr = vec![0x65, 0xAA, 0xBB]; // NAL type 5 (IDR)
        let nalus = vec![sps, pps, idr.clone()];
        let avcc = nals_to_length_prefixed(&nalus, NalFilter::H264);
        // Only the IDR should remain, length-prefixed.
        assert_eq!(avcc.len(), 4 + 3);
        assert_eq!(&avcc[0..4], &3u32.to_be_bytes());
        assert_eq!(&avcc[4..7], &idr[..]);
    }

    #[test]
    fn fragment_emission_from_rtp() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::{Arc, Mutex};

        struct SpyObserver {
            init_count: AtomicU32,
            fragments: Mutex<Vec<Fragment>>,
        }
        impl lvqr_ingest::FragmentObserver for SpyObserver {
            fn on_init(&self, _broadcast: &str, _track: &str, _timescale: u32, _init: Bytes) {
                self.init_count.fetch_add(1, Ordering::Relaxed);
            }
            fn on_fragment(&self, _broadcast: &str, _track: &str, fragment: &Fragment) {
                self.fragments.lock().unwrap().push(fragment.clone());
            }
        }

        let spy = Arc::new(SpyObserver {
            init_count: AtomicU32::new(0),
            fragments: Mutex::new(Vec::new()),
        });
        let obs: SharedFragmentObserver = spy.clone();

        let mut conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
        };

        // Set up a recording session so process_rtp_frame has a broadcast name.
        let session_id = generate_session_id();
        let mut session = Session::new(session_id.clone(), SessionMode::Ingest, "test/cam1".into());
        session.setup_track("track1", Some((0, 1)));
        session.record().unwrap();
        conn.sessions.insert(session_id, session);

        // Build a STAP-A packet with SPS + PPS.
        let sps = vec![0x67, 0x42, 0x00, 0x1F];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut stap_payload = vec![24u8]; // STAP-A
        stap_payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        stap_payload.extend_from_slice(&sps);
        stap_payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        stap_payload.extend_from_slice(&pps);

        let stap_frame = make_interleaved_rtp(0, 96, 1, 90000, false, &stap_payload);
        process_rtp_frame(&mut conn, &stap_frame, Some(&obs));
        // SPS/PPS stored, init emitted (SPS+PPS are enough to build init segment).
        assert!(conn.sps.is_some());
        assert!(conn.pps.is_some());
        assert!(conn.video_init_emitted);
        assert_eq!(spy.init_count.load(Ordering::Relaxed), 1);
        // No fragment yet -- STAP-A only had param sets, no VCL NALs.
        assert_eq!(spy.fragments.lock().unwrap().len(), 0);

        // Send an IDR slice (keyframe).
        let idr_payload = vec![0x65, 0xAA, 0xBB, 0xCC]; // NAL type 5
        let idr_frame = make_interleaved_rtp(0, 96, 2, 93000, true, &idr_payload);
        process_rtp_frame(&mut conn, &idr_frame, Some(&obs));
        assert!(conn.video_init_emitted);
        assert_eq!(spy.init_count.load(Ordering::Relaxed), 1);
        assert_eq!(spy.fragments.lock().unwrap().len(), 1);
        assert!(spy.fragments.lock().unwrap()[0].flags.keyframe);

        // Send a P-frame (non-keyframe).
        let p_payload = vec![0x41, 0xDD, 0xEE]; // NAL type 1
        let p_frame = make_interleaved_rtp(0, 96, 3, 96000, true, &p_payload);
        process_rtp_frame(&mut conn, &p_frame, Some(&obs));
        assert_eq!(spy.fragments.lock().unwrap().len(), 2);
        assert!(!spy.fragments.lock().unwrap()[1].flags.keyframe);
    }

    /// Build an interleaved RTP frame for testing.
    fn make_interleaved_rtp(
        channel: u8,
        pt: u8,
        seq: u16,
        ts: u32,
        marker: bool,
        payload: &[u8],
    ) -> rtp::InterleavedFrame {
        let mut rtp_pkt = vec![0u8; 12 + payload.len()];
        rtp_pkt[0] = 0x80; // version=2
        rtp_pkt[1] = pt | if marker { 0x80 } else { 0x00 };
        rtp_pkt[2..4].copy_from_slice(&seq.to_be_bytes());
        rtp_pkt[4..8].copy_from_slice(&ts.to_be_bytes());
        rtp_pkt[8..12].copy_from_slice(&0x12345678u32.to_be_bytes());
        rtp_pkt[12..].copy_from_slice(payload);
        rtp::InterleavedFrame {
            channel,
            payload: rtp_pkt,
        }
    }
}
