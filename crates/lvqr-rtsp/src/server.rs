//! RTSP/1.0 TCP server with per-connection request handling.

use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, RawSample, VideoInitParams, build_moof_mdat, write_aac_init_segment,
    write_avc_init_segment, write_hevc_init_segment,
};
use lvqr_codec::hevc as hevc_codec;
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags};
use lvqr_ingest::{publish_fragment, publish_init};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::proto::{self, Method, Response, parse_transport};
use crate::rtp::{self, AacDepacketizer, H264Depacketizer, HevcDepacketizer, parse_rtp_header};
use crate::session::{
    Session, SessionId, SessionMode, SessionState, TrackCodec, generate_session_id, parse_sdp_tracks,
};

const SUPPORTED_METHODS: &str = "OPTIONS, DESCRIBE, ANNOUNCE, SETUP, PLAY, RECORD, TEARDOWN, GET_PARAMETER";

/// Future returned by an [`OwnerResolver`]. Producing an owned
/// `String` keeps the callback object-safe without HRTBs.
pub type RedirectFuture = std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>;

/// Callback that resolves an unknown broadcast name to the base URL
/// of the owning node's RTSP endpoint (e.g.
/// `"rtsp://a.local:8554"`). Returning `Some(url)` on a DESCRIBE
/// or PLAY triggers an RTSP `302 Moved Temporarily` with
/// `Location: <url>/<broadcast>`; returning `None` lets the handler
/// fall through to its existing synthetic-empty-SDP or
/// not-found path.
///
/// Semantic mirror of `lvqr_hls::OwnerResolver` and
/// `lvqr_dash::OwnerResolver`. Typically backed by
/// `lvqr_cluster::Cluster::find_owner_endpoints` with each
/// protocol-specific wrapper extracting the matching slot from
/// the resolved `NodeEndpoints`.
pub type OwnerResolver = std::sync::Arc<dyn Fn(String) -> RedirectFuture + Send + Sync>;

pub struct RtspServer {
    addr: SocketAddr,
    pre_bound: Option<TcpListener>,
    registry: FragmentBroadcasterRegistry,
    owner_resolver: Option<OwnerResolver>,
}

impl RtspServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            pre_bound: None,
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: None,
        }
    }

    /// Construct with an externally-owned broadcaster registry. Used when
    /// multiple ingest protocols share one registry so consumers can
    /// subscribe to any broadcast regardless of which protocol fed it.
    pub fn with_registry(addr: SocketAddr, registry: FragmentBroadcasterRegistry) -> Self {
        Self {
            addr,
            pre_bound: None,
            registry,
            owner_resolver: None,
        }
    }

    /// Install an [`OwnerResolver`] so DESCRIBE / PLAY for a
    /// broadcast this node does not host emit an RTSP 302 to the
    /// owning cluster peer instead of the synthetic empty SDP.
    /// Returns self for chained construction.
    pub fn with_owner_resolver(mut self, resolver: OwnerResolver) -> Self {
        self.owner_resolver = Some(resolver);
        self
    }

    /// Handle to the broadcaster registry. Consumers call
    /// `registry.subscribe(broadcast, track)` to receive a `FragmentStream`
    /// for any ingest-produced broadcast the RTSP server has seen.
    pub fn registry(&self) -> FragmentBroadcasterRegistry {
        self.registry.clone()
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

    pub async fn run(self, events: EventBus, shutdown: CancellationToken) -> Result<(), std::io::Error> {
        let Self {
            addr,
            pre_bound,
            registry,
            owner_resolver,
        } = self;
        let listener = match pre_bound {
            Some(l) => l,
            None => TcpListener::bind(addr).await?,
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
                    let ev = events.clone();
                    let conn_shutdown = shutdown.clone();
                    let server_addr = local_addr;
                    let conn_registry = registry.clone();
                    let conn_resolver = owner_resolver.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(
                            socket,
                            remote,
                            server_addr,
                            &ev,
                            &conn_registry,
                            conn_resolver,
                            conn_shutdown,
                        )
                        .await
                        {
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
    /// Shared broadcaster registry. The DESCRIBE handler reads init
    /// bytes off broadcaster meta to synthesize an SDP response; the
    /// PLAY drain task subscribes through it to produce RTP.
    registry: FragmentBroadcasterRegistry,
    /// Optional cluster redirect resolver. Checked on DESCRIBE /
    /// PLAY when no local broadcaster hosts the requested name;
    /// on `Some(url)` the handler emits an RTSP 302 to that peer
    /// instead of the synthetic empty SDP. `None` disables the
    /// redirect path entirely (single-node deployments).
    owner_resolver: Option<OwnerResolver>,
    /// Sink for bytes the main loop should write to the TCP socket.
    /// RTSP responses and interleaved RTP frames from PLAY drain
    /// tasks both flow through this channel so the socket is
    /// never written to from more than one place.
    writer_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    /// Per-connection cancel token. Cancelled when the main loop
    /// exits; observed by drain tasks so they stop promptly.
    conn_cancel: CancellationToken,
    h264_depack: H264Depacketizer,
    hevc_depack: HevcDepacketizer,
    aac_depack: AacDepacketizer,
    rtp_packet_count: u64,
    // Video state
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    vps: Option<Vec<u8>>,
    video_init_emitted: bool,
    video_seq: u64,
    prev_video_dts: Option<u64>,
    // Audio state
    audio_init_emitted: bool,
    audio_seq: u64,
    audio_timescale: u32,
    prev_audio_dts: Option<u64>,
}

async fn handle_connection(
    mut socket: TcpStream,
    remote: SocketAddr,
    server_addr: SocketAddr,
    events: &EventBus,
    registry: &FragmentBroadcasterRegistry,
    owner_resolver: Option<OwnerResolver>,
    shutdown: CancellationToken,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = vec![0u8; 8192];
    let mut read_buf = Vec::with_capacity(8192);

    // Per-connection writer channel. RTSP responses and interleaved
    // RTP frames from any PLAY drain both fan through this channel
    // to the main loop, which owns the single writable end of the
    // TCP socket. Bounded so a slow client gives the drain natural
    // back-pressure via `writer_tx.send().await`.
    let (writer_tx, mut writer_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let conn_cancel = shutdown.child_token();

    let mut conn = ConnectionState {
        sessions: HashMap::new(),
        server_addr,
        registry: registry.clone(),
        owner_resolver,
        writer_tx: writer_tx.clone(),
        conn_cancel: conn_cancel.clone(),
        h264_depack: H264Depacketizer::new(),
        hevc_depack: HevcDepacketizer::new(),
        aac_depack: AacDepacketizer::new(),
        rtp_packet_count: 0,
        sps: None,
        pps: None,
        vps: None,
        video_init_emitted: false,
        video_seq: 0,
        prev_video_dts: None,
        audio_init_emitted: false,
        audio_seq: 0,
        audio_timescale: 44100,
        prev_audio_dts: None,
    };
    // Drop the local writer_tx so the only remaining senders are the
    // one on `conn` (cloned into drain tasks) plus any drains already
    // spawned. When the main loop exits, conn drops and the drain
    // tasks' remaining clones die on their next send, terminating
    // them naturally.
    drop(writer_tx);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(bytes) = writer_rx.recv() => {
                if socket.write_all(&bytes).await.is_err() {
                    debug!(%remote, "RTSP socket write failed; closing connection");
                    break;
                }
            }
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
                        // Interleaved RTP/RTCP frame (ingest path).
                        match rtp::parse_interleaved_frame(&read_buf) {
                            Some((frame, consumed)) => {
                                process_rtp_frame(&mut conn, &frame, registry);
                                read_buf.drain(..consumed);
                            }
                            None => break, // incomplete
                        }
                    } else {
                        match proto::parse_request(&read_buf) {
                            Ok((req, consumed)) => {
                                debug!(%remote, method = %req.method, uri = %req.uri, "RTSP request");
                                let resp = handle_request(&mut conn, &req).await;
                                // `send().await` over a bounded mpsc; on
                                // connection tear-down the receiver is
                                // dropped, the send errors, and we bail.
                                if conn.writer_tx.send(resp.serialize()).await.is_err() {
                                    debug!(%remote, "RTSP writer closed mid-response");
                                    return Ok(());
                                }
                                read_buf.drain(..consumed);
                            }
                            Err(proto::ParseError::Incomplete) => break,
                            Err(e) => {
                                warn!(%remote, error = %e, "RTSP parse error");
                                let resp = Response::bad_request().with_cseq(0);
                                let _ = conn.writer_tx.send(resp.serialize()).await;
                                read_buf.clear();
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // Cancel drain tasks spawned for any PLAY sessions on this
    // connection. Their writer_tx clones also fail once conn is
    // dropped below, but cancelling proactively stops any drain
    // parked in `next_fragment().await` from lingering.
    conn_cancel.cancel();

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
    registry: &FragmentBroadcasterRegistry,
) {
    // Odd channels are RTCP -- skip for now.
    if frame.channel % 2 != 0 {
        return;
    }

    let Some(header) = parse_rtp_header(&frame.payload) else {
        return;
    };
    let rtp_payload = &frame.payload[header.header_len..];

    let session = match conn.sessions.values().find(|s| s.state == SessionState::Recording) {
        Some(s) => s,
        None => return,
    };
    let broadcast = session.broadcast.clone();

    // Determine media type by matching the interleaved channel to the
    // track's transport setup. Channel 0/1 is typically the first track
    // (video), channel 2/3 the second (audio). Fall back to detecting
    // by track order when no explicit mapping is found.
    let rtp_channel = frame.channel;
    let media_type = find_media_type_for_channel(session, rtp_channel);

    match media_type {
        crate::session::MediaType::Audio => {
            process_audio_rtp(conn, &broadcast, rtp_payload, &header, registry);
        }
        crate::session::MediaType::Video => {
            process_video_rtp(conn, &broadcast, rtp_payload, &header, frame.channel, registry);
        }
    }
}

/// Determine the media type for a given interleaved RTP channel by
/// checking the session's track transports. Falls back to Video for
/// channel 0, Audio for channel 2, Video otherwise.
fn find_media_type_for_channel(session: &Session, channel: u8) -> crate::session::MediaType {
    use crate::session::MediaType;
    // Walk through tracks in order and check their transport's interleaved range.
    for track in &session.tracks {
        if let Some(transport) = session.transports.get(&track.control) {
            if let Some((rtp_ch, _rtcp_ch)) = transport.interleaved {
                if rtp_ch == channel {
                    return track.media_type;
                }
            }
        }
    }
    // Fallback: channel 0 -> video, channel 2 -> audio.
    if channel >= 2 {
        MediaType::Audio
    } else {
        MediaType::Video
    }
}

fn process_video_rtp(
    conn: &mut ConnectionState,
    broadcast: &str,
    rtp_payload: &[u8],
    header: &rtp::RtpHeader,
    channel: u8,
    registry: &FragmentBroadcasterRegistry,
) {
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
        TrackCodec::H265 => conn.hevc_depack.depacketize(rtp_payload, header),
        _ => conn.h264_depack.depacketize(rtp_payload, header),
    };
    let Some(result) = result else {
        return;
    };

    conn.rtp_packet_count += 1;
    let codec_label = if codec == TrackCodec::H265 { "H.265" } else { "H.264" };
    debug!(
        channel,
        ts = header.timestamp,
        nalus = result.nalus.len(),
        keyframe = result.keyframe,
        marker = result.marker,
        count = conn.rtp_packet_count,
        codec = codec_label,
        "RTSP RTP depacketized video"
    );

    match codec {
        TrackCodec::H265 => process_hevc_nalus(conn, broadcast, &result, registry),
        _ => process_h264_nalus(conn, broadcast, &result, registry),
    }
}

fn process_audio_rtp(
    conn: &mut ConnectionState,
    broadcast: &str,
    rtp_payload: &[u8],
    header: &rtp::RtpHeader,
    registry: &FragmentBroadcasterRegistry,
) {
    let Some(result) = conn.aac_depack.depacketize(rtp_payload, header) else {
        return;
    };

    conn.rtp_packet_count += 1;
    debug!(
        ts = header.timestamp,
        frames = result.frames.len(),
        count = conn.rtp_packet_count,
        "RTSP RTP depacketized AAC"
    );

    // Emit audio init segment on first AAC frame.
    if !conn.audio_init_emitted {
        // Try to get AudioSpecificConfig from the SDP fmtp line.
        let asc = conn
            .sessions
            .values()
            .find(|s| s.state == SessionState::Recording)
            .and_then(|s| s.tracks.iter().find(|t| t.codec == TrackCodec::Aac))
            .and_then(|t| t.fmtp.as_ref())
            .and_then(|fmtp| rtp::parse_aac_config_from_fmtp(fmtp));

        let Some(asc) = asc else {
            debug!(%broadcast, "RTSP: no AAC config in SDP, skipping audio init");
            return;
        };

        // Extract sample rate from the audio track's clock_rate.
        let timescale = conn
            .sessions
            .values()
            .find(|s| s.state == SessionState::Recording)
            .and_then(|s| s.tracks.iter().find(|t| t.codec == TrackCodec::Aac))
            .map(|t| t.clock_rate)
            .unwrap_or(44100);

        let params = AudioInitParams { asc, timescale };
        let mut buf = BytesMut::with_capacity(512);
        if let Err(e) = write_aac_init_segment(&mut buf, &params) {
            warn!(%broadcast, error = %e, "RTSP: failed to write AAC init segment");
            return;
        }
        publish_init(registry, broadcast, "1.mp4", "mp4a.40.2", timescale, buf.freeze());
        conn.audio_init_emitted = true;
        conn.audio_timescale = timescale;
        info!(%broadcast, %timescale, "RTSP: audio init emitted");
    }

    // Emit each AAC access unit as a fragment.
    let dts = result.timestamp as u64;
    for frame_data in &result.frames {
        let duration = match conn.prev_audio_dts {
            Some(prev) if dts > prev => (dts - prev) as u32,
            _ => 1024,
        };
        conn.prev_audio_dts = Some(dts);

        let raw = RawSample {
            track_id: 2,
            dts,
            cts_offset: 0,
            duration,
            payload: Bytes::copy_from_slice(frame_data),
            keyframe: true,
        };
        conn.audio_seq += 1;
        let moof_mdat = build_moof_mdat(conn.audio_seq as u32, 2, dts, &[raw]);
        let frag = Fragment::new(
            "1.mp4",
            conn.audio_seq,
            0,
            0,
            dts,
            dts,
            duration as u64,
            FragmentFlags::AUDIO,
            moof_mdat,
        );
        publish_fragment(registry, broadcast, "1.mp4", "mp4a.40.2", conn.audio_timescale, frag);
    }
}

fn process_h264_nalus(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    registry: &FragmentBroadcasterRegistry,
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
        publish_init(registry, broadcast, "0.mp4", "avc1", 90_000, buf.freeze());
        conn.video_init_emitted = true;
        info!(%broadcast, "RTSP: H.264 video init emitted");
    }

    let avcc = nals_to_length_prefixed(&result.nalus, NalFilter::H264);
    if avcc.is_empty() {
        return;
    }
    emit_video_fragment(conn, broadcast, result, avcc, "avc1", registry);
}

fn process_hevc_nalus(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    registry: &FragmentBroadcasterRegistry,
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
        publish_init(registry, broadcast, "0.mp4", "hev1", 90_000, buf.freeze());
        conn.video_init_emitted = true;
        info!(%broadcast, "RTSP: HEVC video init emitted");
    }

    let hvcc = nals_to_length_prefixed(&result.nalus, NalFilter::Hevc);
    if hvcc.is_empty() {
        return;
    }
    emit_video_fragment(conn, broadcast, result, hvcc, "hev1", registry);
}

fn emit_video_fragment(
    conn: &mut ConnectionState,
    broadcast: &str,
    result: &rtp::DepackResult,
    payload: Vec<u8>,
    codec: &str,
    registry: &FragmentBroadcasterRegistry,
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
    publish_fragment(registry, broadcast, "0.mp4", codec, 90_000, frag);
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

async fn handle_request(conn: &mut ConnectionState, req: &proto::Request) -> Response {
    let cseq = req.cseq().unwrap_or(0);

    // Cluster redirect check for DESCRIBE / PLAY when the broadcast
    // has no local publisher. We consult the resolver before
    // dispatching to `handle_describe` / `handle_play` because the
    // sync handlers otherwise fall back to a synthetic empty SDP
    // rather than an error, and we want peer redirects to take
    // precedence over that fallback.
    if matches!(req.method, Method::Describe | Method::Play)
        && let Some(resolver) = conn.owner_resolver.as_ref()
    {
        let broadcast = extract_broadcast(&req.uri);
        let local_known =
            conn.registry.get(&broadcast, "0.mp4").is_some() || conn.registry.get(&broadcast, "1.mp4").is_some();
        if !local_known && let Some(base) = resolver(broadcast.clone()).await {
            let base = base.trim_end_matches('/');
            let location = format!("{base}/{broadcast}");
            return Response::found(&location).with_cseq(cseq);
        }
    }

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

    // Session-level SDP. The video block is populated only when the
    // shared registry carries a video broadcaster for this broadcast
    // AND its meta has a parseable init segment. Try HEVC first
    // (its extractor returns None on an AVC init) so an HEVC
    // publisher is described correctly, then fall back to AVC. An
    // absent video block still yields a syntactically valid SDP so
    // clients that DESCRIBE before anyone publishes get a
    // well-formed empty stream description rather than a 404.
    let video_init = conn
        .registry
        .get(&broadcast, "0.mp4")
        .and_then(|bc| bc.meta().init_segment.clone());

    let video = video_init.as_ref().and_then(|init| {
        if let Some(params) = lvqr_cmaf::extract_hevc_parameter_sets(init) {
            Some(crate::sdp::VideoTrackDescription::Hevc(
                crate::sdp::HevcTrackDescription {
                    payload_type: 96,
                    clock_rate: 90_000,
                    control: "track1".to_string(),
                    params,
                },
            ))
        } else {
            lvqr_cmaf::extract_avc_parameter_sets(init).map(|params| {
                crate::sdp::VideoTrackDescription::H264(crate::sdp::H264TrackDescription {
                    payload_type: 96,
                    clock_rate: 90_000,
                    control: "track1".to_string(),
                    params,
                })
            })
        }
    });

    // Audio block comes from the `1.mp4` broadcaster if present.
    // Try Opus first (its extractor returns None on an AAC init) so a
    // WebRTC Opus publisher is described correctly, then fall back to
    // AAC. PT 98 keeps Opus distinct from AAC's PT 97 so a future
    // client-side PT-keyed dispatcher does not confuse them.
    let audio_init = conn
        .registry
        .get(&broadcast, "1.mp4")
        .and_then(|bc| bc.meta().init_segment.clone());

    let audio = audio_init.as_ref().and_then(|init| {
        if let Some(config) = lvqr_cmaf::extract_opus_config(init) {
            Some(crate::sdp::AudioTrackDescription::Opus(
                crate::sdp::OpusTrackDescription {
                    payload_type: 98,
                    control: "track2".to_string(),
                    config,
                },
            ))
        } else {
            lvqr_cmaf::extract_aac_config(init).map(|config| {
                crate::sdp::AudioTrackDescription::Aac(crate::sdp::AacTrackDescription {
                    payload_type: 97,
                    control: "track2".to_string(),
                    config,
                })
            })
        }
    });

    let sdp = crate::sdp::PlaySdp {
        session_name: broadcast,
        host_ip: conn.server_addr.ip(),
        video,
        audio,
    }
    .render();

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
    let session_id_owned = session_id.to_string();
    let broadcast = session.broadcast.clone();
    // Resolve the per-track interleaved channels the client
    // negotiated at SETUP. Clients that SETUPped only one track
    // (video or audio) get one channel resolved; the other drain
    // is skipped. SETUP currently stamps the transport under the
    // track control URI -- matching DESCRIBE's emitted "track1" /
    // "track2" URIs is how we route each drain.
    let video_channels = session
        .transports
        .get("track1")
        .and_then(|t| t.interleaved)
        .or_else(|| {
            // Clients that only setup one track (video) with no
            // explicit track URI get the first transport registered.
            if session.transports.len() == 1 && !session.transports.contains_key("track2") {
                session.transports.values().find_map(|t| t.interleaved)
            } else {
                None
            }
        });
    let audio_channels = session.transports.get("track2").and_then(|t| t.interleaved);

    // Pick the video drain variant that matches the broadcaster's
    // codec. Try HEVC first (its extractor returns None on an AVC
    // init) so an HEVC publisher gets the HEVC drain; fall back to
    // H.264 otherwise. If neither matches, default to H.264; the
    // drain will itself notice the absent broadcaster and exit.
    let video_init = conn
        .registry
        .get(&broadcast, "0.mp4")
        .and_then(|bc| bc.meta().init_segment.clone());
    let is_hevc = video_init
        .as_ref()
        .is_some_and(|bytes| lvqr_cmaf::extract_hevc_parameter_sets(bytes).is_some());

    // Dispatch the audio drain the same way: Opus first (its
    // extractor returns None on an AAC init), fall through to AAC.
    // A broadcaster with neither codec on the 1.mp4 init just
    // exits the drain on first subscribe.
    let audio_init = conn
        .registry
        .get(&broadcast, "1.mp4")
        .and_then(|bc| bc.meta().init_segment.clone());
    let is_opus = audio_init
        .as_ref()
        .is_some_and(|bytes| lvqr_cmaf::extract_opus_config(bytes).is_some());

    let registry = conn.registry.clone();
    let cancel = conn.conn_cancel.clone();

    if let Some((rtp_channel, rtcp_channel)) = video_channels {
        let writer_tx = conn.writer_tx.clone();
        let registry = registry.clone();
        let cancel = cancel.clone();
        let ctx = crate::play::PlayDrainCtx::new(broadcast.clone(), rtp_channel, rtcp_channel);
        if is_hevc {
            tokio::spawn(crate::play::play_drain_hevc(ctx, registry, writer_tx, cancel));
        } else {
            tokio::spawn(crate::play::play_drain_h264(ctx, registry, writer_tx, cancel));
        }
    }

    if let Some((rtp_channel, rtcp_channel)) = audio_channels {
        let writer_tx = conn.writer_tx.clone();
        let registry = registry.clone();
        let cancel = cancel.clone();
        let ctx = crate::play::PlayDrainCtx::new(broadcast.clone(), rtp_channel, rtcp_channel);
        if is_opus {
            tokio::spawn(crate::play::play_drain_opus(ctx, registry, writer_tx, cancel));
        } else {
            tokio::spawn(crate::play::play_drain_aac(ctx, registry, writer_tx, cancel));
        }
    }

    info!(
        session = %session_id_owned,
        %broadcast,
        ?video_channels,
        ?audio_channels,
        video_codec = if is_hevc { "H265" } else { "H264" },
        audio_codec = if is_opus { "Opus" } else { "AAC" },
        "RTSP PLAY started, drains spawned"
    );
    Response::ok().with_cseq(cseq).with_header("Session", &session_id_owned)
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
    fn describe_without_broadcaster_returns_empty_sdp() {
        // Before anyone publishes, DESCRIBE returns a valid session
        // header but no media m= line. A video m= line only appears
        // after the registry carries an H.264 broadcaster with a
        // decodable init segment.
        let conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: None,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
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
        assert!(body.contains("s=live/test"));
        assert!(!body.contains("m=video"), "no video m= when no broadcaster");
    }

    #[test]
    fn describe_with_broadcaster_emits_h264_media_block() {
        use bytes::BytesMut;
        use lvqr_cmaf::{VideoInitParams, write_avc_init_segment};
        use lvqr_fragment::FragmentMeta;

        let registry = FragmentBroadcasterRegistry::new();

        // Pre-populate the broadcaster with a real AVC init segment so
        // DESCRIBE can extract SPS/PPS.
        let sps = [0x67u8, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00];
        let pps = [0x68u8, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];
        let mut init = BytesMut::new();
        write_avc_init_segment(
            &mut init,
            &VideoInitParams {
                sps: sps.to_vec(),
                pps: pps.to_vec(),
                width: 1280,
                height: 720,
                timescale: 90_000,
            },
        )
        .expect("write init");
        let bc = registry.get_or_create("live/test", "0.mp4", FragmentMeta::new("avc1", 90_000));
        bc.set_init_segment(init.freeze());

        let conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            registry,
            owner_resolver: None,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
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
        assert!(body.contains("m=video 0 RTP/AVP 96"));
        assert!(body.contains("a=rtpmap:96 H264/90000"));
        assert!(body.contains("packetization-mode=1"));
        assert!(body.contains("sprop-parameter-sets="));
        assert!(body.contains("profile-level-id=42001F"), "profile pulled from SPS");
        assert!(body.contains("a=control:track1"));
    }

    #[tokio::test]
    async fn full_playback_handshake() {
        let mut conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: None,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
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
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: None,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
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

    #[tokio::test]
    async fn fragment_emission_from_rtp() {
        use lvqr_fragment::{FragmentMeta, FragmentStream};

        let mut conn = ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: None,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
        };

        // Set up a recording session so process_rtp_frame has a broadcast name.
        let session_id = generate_session_id();
        let mut session = Session::new(session_id.clone(), SessionMode::Ingest, "test/cam1".into());
        session.setup_track("track1", Some((0, 1)));
        session.record().unwrap();
        conn.sessions.insert(session_id, session);

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("test/cam1", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut sub = bc.subscribe();

        // Build a STAP-A packet with SPS + PPS.
        let sps = vec![0x67, 0x42, 0x00, 0x1F];
        let pps = vec![0x68, 0xCE, 0x38, 0x80];
        let mut stap_payload = vec![24u8]; // STAP-A
        stap_payload.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        stap_payload.extend_from_slice(&sps);
        stap_payload.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        stap_payload.extend_from_slice(&pps);

        let stap_frame = make_interleaved_rtp(0, 96, 1, 90000, false, &stap_payload);
        process_rtp_frame(&mut conn, &stap_frame, &registry);
        // SPS/PPS stored, init emitted. No fragment yet -- STAP-A only
        // carried parameter sets.
        assert!(conn.sps.is_some());
        assert!(conn.pps.is_some());
        assert!(conn.video_init_emitted);
        assert!(bc.meta().init_segment.is_some(), "registry carries AVC init bytes");

        // Send an IDR slice (keyframe). Drains into the subscriber.
        let idr_payload = vec![0x65, 0xAA, 0xBB, 0xCC];
        let idr_frame = make_interleaved_rtp(0, 96, 2, 93000, true, &idr_payload);
        process_rtp_frame(&mut conn, &idr_frame, &registry);
        let frag0 = sub.next_fragment().await.expect("keyframe delivered");
        assert!(frag0.flags.keyframe);

        // Send a P-frame.
        let p_payload = vec![0x41, 0xDD, 0xEE];
        let p_frame = make_interleaved_rtp(0, 96, 3, 96000, true, &p_payload);
        process_rtp_frame(&mut conn, &p_frame, &registry);
        let frag1 = sub.next_fragment().await.expect("delta delivered");
        assert!(!frag1.flags.keyframe);
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

    // --- Cluster redirect (session F2b) ------------------------------

    fn empty_conn_with_resolver(resolver: Option<OwnerResolver>) -> ConnectionState {
        ConnectionState {
            sessions: HashMap::new(),
            server_addr: "127.0.0.1:8554".parse().unwrap(),
            registry: FragmentBroadcasterRegistry::new(),
            owner_resolver: resolver,
            writer_tx: tokio::sync::mpsc::channel(1).0,
            conn_cancel: CancellationToken::new(),
            h264_depack: H264Depacketizer::new(),
            hevc_depack: HevcDepacketizer::new(),
            aac_depack: AacDepacketizer::new(),
            rtp_packet_count: 0,
            sps: None,
            pps: None,
            vps: None,
            video_init_emitted: false,
            video_seq: 0,
            prev_video_dts: None,
            audio_init_emitted: false,
            audio_seq: 0,
            audio_timescale: 44100,
            prev_audio_dts: None,
        }
    }

    fn describe_req(uri: &str, cseq: u32) -> proto::Request {
        let mut headers = proto::Headers::new();
        headers.insert("CSeq".to_string(), cseq.to_string());
        proto::Request {
            method: Method::Describe,
            uri: uri.to_string(),
            version: proto::RtspVersion::V1_0,
            headers,
            body: Vec::new(),
        }
    }

    #[tokio::test]
    async fn describe_unknown_broadcast_redirects_when_resolver_hits() {
        let resolver: OwnerResolver = std::sync::Arc::new(|broadcast| {
            Box::pin(async move {
                if broadcast == "live/test" {
                    Some("rtsp://a.local:8554".to_string())
                } else {
                    None
                }
            })
        });
        let mut conn = empty_conn_with_resolver(Some(resolver));
        let req = describe_req("rtsp://localhost:8554/live/test", 7);
        let resp = handle_request(&mut conn, &req).await;
        assert_eq!(resp.status, 302);
        assert_eq!(resp.headers.get("Location"), Some("rtsp://a.local:8554/live/test"));
        assert_eq!(resp.headers.get("CSeq"), Some("7"));
    }

    #[tokio::test]
    async fn describe_unknown_broadcast_falls_through_when_resolver_misses() {
        let resolver: OwnerResolver = std::sync::Arc::new(|_b| Box::pin(async move { None }));
        let mut conn = empty_conn_with_resolver(Some(resolver));
        let req = describe_req("rtsp://localhost:8554/live/test", 3);
        let resp = handle_request(&mut conn, &req).await;
        // Falls through to the empty-SDP path, which returns 200.
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn describe_known_local_broadcast_skips_resolver() {
        use bytes::BytesMut;
        use lvqr_fragment::FragmentMeta;

        // Install a broadcaster so the redirect guard sees the
        // broadcast is locally hosted and must NOT consult the
        // resolver.
        let resolver: OwnerResolver =
            std::sync::Arc::new(|_b| Box::pin(async move { Some("rtsp://elsewhere.invalid".to_string()) }));
        let mut conn = empty_conn_with_resolver(Some(resolver));
        let bc = conn
            .registry
            .get_or_create("live/local", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let init = BytesMut::from(&b"\x00\x00\x00\x18ftypiso5"[..]);
        bc.set_init_segment(init.freeze());

        let req = describe_req("rtsp://localhost:8554/live/local", 9);
        let resp = handle_request(&mut conn, &req).await;
        // 200 means the resolver was skipped in favor of the
        // local empty/synthetic SDP path.
        assert_eq!(resp.status, 200);
        assert!(resp.headers.get("Location").is_none());
    }

    #[tokio::test]
    async fn redirect_response_trims_trailing_slash_on_base_url() {
        let resolver: OwnerResolver =
            std::sync::Arc::new(|_b| Box::pin(async move { Some("rtsp://a.local:8554/".to_string()) }));
        let mut conn = empty_conn_with_resolver(Some(resolver));
        let req = describe_req("rtsp://localhost:8554/live/test", 1);
        let resp = handle_request(&mut conn, &req).await;
        assert_eq!(resp.status, 302);
        assert_eq!(resp.headers.get("Location"), Some("rtsp://a.local:8554/live/test"));
    }
}
