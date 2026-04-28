//! SRT ingest server: listener + per-connection MPEG-TS demux
//! + Fragment emission.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use lvqr_auth::{AuthDecision, NoopAuthProvider, SharedAuth, extract};
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, VideoInitParams, build_moof_mdat, write_aac_init_segment, write_avc_init_segment,
    write_hevc_init_segment,
};
use lvqr_codec::hevc::{self as hevc_codec, HevcNalType};
use lvqr_codec::ts::{PesPacket, StreamType, TsDemuxer};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags};
use lvqr_ingest::{publish_fragment, publish_init, publish_scte35};
use srt_tokio::access::{RejectReason, ServerRejectReason};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// SRT ingest server. Bind to a UDP port, accept SRT connections,
/// demux MPEG-TS, and emit Fragments.
pub struct SrtIngestServer {
    addr: SocketAddr,
    pre_bound: Option<tokio::net::UdpSocket>,
    registry: FragmentBroadcasterRegistry,
    auth: SharedAuth,
}

impl SrtIngestServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            pre_bound: None,
            registry: FragmentBroadcasterRegistry::new(),
            auth: Arc::new(NoopAuthProvider),
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
            auth: Arc::new(NoopAuthProvider),
        }
    }

    /// Install a shared auth provider. The provider is consulted on
    /// every accepted SRT connection request before the socket
    /// handshake completes. On `AuthDecision::Deny` the request is
    /// rejected with `ServerRejectReason::Unauthorized` (SRT code
    /// 2401) and no task spawns.
    pub fn with_auth(mut self, auth: SharedAuth) -> Self {
        self.auth = auth;
        self
    }

    /// Handle to the broadcaster registry. Consumers call
    /// `registry.subscribe(broadcast, track)` to receive a `FragmentStream`
    /// for any ingest-produced broadcast the SRT server has seen.
    pub fn registry(&self) -> FragmentBroadcasterRegistry {
        self.registry.clone()
    }

    /// Pre-bind the UDP socket and return the actual local address.
    /// Use this when the configured address has port 0 (ephemeral)
    /// and the caller needs to know the real port before `run` is
    /// called (e.g. in tests or for the `ServerHandle`).
    pub async fn bind(&mut self) -> Result<SocketAddr, std::io::Error> {
        let socket = tokio::net::UdpSocket::bind(self.addr).await?;
        let bound = socket.local_addr()?;
        self.addr = bound;
        self.pre_bound = Some(socket);
        Ok(bound)
    }

    /// Run the listener loop until `shutdown` fires.
    pub async fn run(self, events: EventBus, shutdown: CancellationToken) -> Result<(), std::io::Error> {
        let Self {
            addr,
            pre_bound,
            registry,
            auth,
        } = self;
        let builder = srt_tokio::SrtListener::builder();
        let (mut listener, mut incoming) = match pre_bound {
            Some(socket) => builder.socket(socket).bind(addr).await?,
            None => builder.bind(addr).await?,
        };

        info!(%addr, "SRT ingest bound");

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => {
                    listener.close().await;
                    break;
                }
                req = incoming.incoming().next() => {
                    let Some(req) = req else { break };
                    let stream_id = req.stream_id().map(|s: &srt_tokio::options::StreamId| s.to_string());
                    let remote = req.remote();
                    let streamid_raw = stream_id.clone().unwrap_or_default();

                    // Parse the streamid KV payload to uncover the
                    // target broadcast (`r=`) and bearer token (`t=`).
                    // `extract_srt` tolerates any KV order and ignores
                    // unknown keys; a blank / missing streamid yields
                    // an empty-token AuthContext that the provider
                    // then evaluates (Noop admits, Jwt / static
                    // denies).
                    let ctx = extract::extract_srt(&streamid_raw);
                    let broadcast = match &ctx {
                        lvqr_auth::AuthContext::Publish { broadcast: Some(b), .. } if !b.is_empty() => b.clone(),
                        _ => stream_id.clone().unwrap_or_else(|| "srt/default".into()),
                    };

                    if let AuthDecision::Deny { reason } = auth.check(&ctx) {
                        warn!(%remote, %broadcast, reason = %reason, "SRT connection rejected by auth");
                        if let Err(e) = req.reject(RejectReason::Server(ServerRejectReason::Unauthorized)).await {
                            warn!(%remote, error = %e, "SRT reject failed");
                        }
                        continue;
                    }

                    info!(%broadcast, %remote, "SRT connection request");

                    let socket = match req.accept(None).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "SRT accept failed");
                            continue;
                        }
                    };

                    let ev = events.clone();
                    let bc = broadcast.clone();
                    let conn_shutdown = shutdown.clone();
                    let conn_registry = registry.clone();
                    tokio::spawn(async move {
                        handle_connection(socket, &bc, &ev, &conn_registry, conn_shutdown).await;
                    });
                }
            }
        }

        Ok(())
    }
}

/// Per-connection state: tracks init emission and fragment sequencing.
struct ConnectionState {
    demux: TsDemuxer,
    video_init_emitted: bool,
    audio_init_emitted: bool,
    video_seq: u64,
    audio_seq: u64,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    vps: Option<Vec<u8>>,
    /// Previous video DTS for frame duration computation. When
    /// `None` (first frame), a default of 3000 ticks (33 ms at
    /// 90 kHz) is used. Subsequent frames use the PTS/DTS delta.
    prev_video_dts: Option<u64>,
    /// Previous audio DTS for frame duration computation.
    prev_audio_dts: Option<u64>,
    /// Audio timescale captured from the ADTS header at init
    /// time. Needed for the default duration fallback (1024
    /// samples at the track's native rate).
    audio_timescale: u32,
}

async fn handle_connection(
    mut socket: srt_tokio::SrtSocket,
    broadcast: &str,
    events: &EventBus,
    registry: &FragmentBroadcasterRegistry,
    shutdown: CancellationToken,
) {
    let mut state = ConnectionState {
        demux: TsDemuxer::new(),
        video_init_emitted: false,
        audio_init_emitted: false,
        video_seq: 0,
        audio_seq: 0,
        sps: None,
        pps: None,
        vps: None,
        prev_video_dts: None,
        prev_audio_dts: None,
        audio_timescale: 44100,
    };

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            item = socket.next() => {
                match item {
                    Some(Ok((_instant, data))) => {
                        let pes_packets = state.demux.feed(&data);
                        for pes in pes_packets {
                            process_pes(&mut state, broadcast, &pes, registry);
                        }
                        for section in state.demux.take_scte35_sections() {
                            process_scte35(broadcast, &section, registry);
                        }
                    }
                    Some(Err(e)) => {
                        warn!(%broadcast, error = %e, "SRT recv error");
                        break;
                    }
                    None => {
                        info!(%broadcast, "SRT connection closed");
                        break;
                    }
                }
            }
        }
    }

    events.emit(RelayEvent::BroadcastStopped {
        name: broadcast.to_string(),
    });
    info!(%broadcast, "SRT session ended, BroadcastStopped emitted");
}

fn process_pes(state: &mut ConnectionState, broadcast: &str, pes: &PesPacket, registry: &FragmentBroadcasterRegistry) {
    match pes.stream_type {
        StreamType::H264 => process_h264(state, broadcast, pes, registry),
        StreamType::Aac => process_aac(state, broadcast, pes, registry),
        StreamType::H265 => process_hevc(state, broadcast, pes, registry),
        StreamType::Scte35 => {
            // SCTE-35 PIDs do not flow through the PES path; the
            // demuxer routes them to its private-section reassembler
            // and surfaces complete sections via
            // TsDemuxer::take_scte35_sections, which the connection
            // loop drains alongside this match. This arm exists only
            // to keep the match exhaustive.
        }
        StreamType::Unknown(st) => {
            debug!(%broadcast, stream_type = st, "SRT unknown stream type; dropping PES");
        }
    }
}

/// Decode one reassembled SCTE-35 section and emit it onto the
/// broadcast's reserved `"scte35"` track.
///
/// Sections that fail CRC verification or splice_command parsing are
/// counted (`lvqr_scte35_drops_total{reason=...}`) and dropped. The
/// passthrough contract is best-effort: if the publisher emits a
/// section we cannot parse, downstream HLS / DASH consumers do not
/// see it, but the publisher's other valid sections still flow.
fn process_scte35(broadcast: &str, section: &lvqr_codec::ts::Scte35Section, registry: &FragmentBroadcasterRegistry) {
    match lvqr_codec::parse_splice_info_section(&section.raw) {
        Ok(info) => {
            let pts = info.absolute_pts().unwrap_or(0);
            let duration = info.duration.unwrap_or(0);
            let event_id = info.event_id.unwrap_or(0) as u64;
            publish_scte35(
                registry,
                broadcast,
                event_id,
                pts,
                duration,
                Bytes::copy_from_slice(&section.raw),
            );
            metrics::counter!(
                "lvqr_scte35_events_total",
                "ingest" => "srt",
                "command" => format!("{:#04x}", info.command_type),
            )
            .increment(1);
        }
        Err(e) => {
            let reason = match &e {
                lvqr_codec::CodecError::Scte35BadCrc { .. } => "crc",
                lvqr_codec::CodecError::Scte35Malformed(_) => "malformed",
                lvqr_codec::CodecError::EndOfStream { .. } => "truncated",
                _ => "other",
            };
            warn!(%broadcast, pid = section.pid, error = %e, "SRT scte35 parse failure");
            metrics::counter!(
                "lvqr_scte35_drops_total",
                "ingest" => "srt",
                "reason" => reason,
            )
            .increment(1);
        }
    }
}

fn process_h264(state: &mut ConnectionState, broadcast: &str, pes: &PesPacket, registry: &FragmentBroadcasterRegistry) {
    let payload = &pes.payload;
    let nalus = split_annex_b(payload);

    for nalu in &nalus {
        if nalu.is_empty() {
            continue;
        }
        let nal_type = nalu[0] & 0x1F;
        match nal_type {
            7 => state.sps = Some(nalu.to_vec()),
            8 => state.pps = Some(nalu.to_vec()),
            _ => {}
        }
    }

    if !state.video_init_emitted {
        let (Some(sps), Some(pps)) = (&state.sps, &state.pps) else {
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
            warn!(%broadcast, error = %e, "SRT: failed to write AVC init segment");
            return;
        }
        publish_init(registry, broadcast, "0.mp4", "avc1", 90_000, buf.freeze());
        state.video_init_emitted = true;
        info!(%broadcast, "SRT: video init emitted");
    }

    let avcc = annex_b_to_avcc(payload);
    if avcc.is_empty() {
        return;
    }

    let dts = pes.dts.or(pes.pts).unwrap_or(0);
    let pts = pes.pts.unwrap_or(dts);
    let keyframe = nalus.iter().any(|n| !n.is_empty() && (n[0] & 0x1F) == 5);
    let duration = match state.prev_video_dts {
        Some(prev) if dts > prev => (dts - prev) as u32,
        _ => 3000,
    };
    state.prev_video_dts = Some(dts);

    let raw = lvqr_cmaf::RawSample {
        track_id: 1,
        dts,
        cts_offset: pts.wrapping_sub(dts) as i32,
        duration,
        payload: Bytes::from(avcc),
        keyframe,
    };
    state.video_seq += 1;
    let moof_mdat = build_moof_mdat(state.video_seq as u32, 1, dts, &[raw]);
    let frag = Fragment::new(
        "0.mp4",
        state.video_seq,
        0,
        0,
        dts,
        pts,
        duration as u64,
        if keyframe {
            FragmentFlags::KEYFRAME
        } else {
            FragmentFlags::DELTA
        },
        moof_mdat,
    );
    publish_fragment(registry, broadcast, "0.mp4", "avc1", 90_000, frag);
}

fn process_hevc(state: &mut ConnectionState, broadcast: &str, pes: &PesPacket, registry: &FragmentBroadcasterRegistry) {
    let payload = &pes.payload;
    let nalus = split_annex_b(payload);

    for nalu in &nalus {
        if nalu.len() < 2 {
            continue;
        }
        let nal_type = (nalu[0] >> 1) & 0x3F;
        match nal_type {
            32 => state.vps = Some(nalu.to_vec()),
            33 => state.sps = Some(nalu.to_vec()),
            34 => state.pps = Some(nalu.to_vec()),
            _ => {}
        }
    }

    if !state.video_init_emitted {
        let (Some(vps), Some(sps), Some(pps)) = (&state.vps, &state.sps, &state.pps) else {
            return;
        };
        let sps_info = match hevc_codec::parse_sps(sps) {
            Ok(v) => v,
            Err(e) => {
                debug!(%broadcast, error = ?e, "SRT: HEVC SPS parse failed; waiting for valid params");
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
            warn!(%broadcast, error = %e, "SRT: failed to write HEVC init segment");
            return;
        }
        publish_init(registry, broadcast, "0.mp4", "hev1", 90_000, buf.freeze());
        state.video_init_emitted = true;
        info!(%broadcast, "SRT: HEVC video init emitted");
    }

    let hvcc = annex_b_to_hvcc(payload);
    if hvcc.is_empty() {
        return;
    }

    let dts = pes.dts.or(pes.pts).unwrap_or(0);
    let pts = pes.pts.unwrap_or(dts);
    let keyframe = nalus.iter().any(|n| {
        if n.len() < 2 {
            return false;
        }
        let t = HevcNalType::from_u8((n[0] >> 1) & 0x3F);
        t.is_keyframe()
    });
    let duration = match state.prev_video_dts {
        Some(prev) if dts > prev => (dts - prev) as u32,
        _ => 3000,
    };
    state.prev_video_dts = Some(dts);

    let raw = lvqr_cmaf::RawSample {
        track_id: 1,
        dts,
        cts_offset: pts.wrapping_sub(dts) as i32,
        duration,
        payload: Bytes::from(hvcc),
        keyframe,
    };
    state.video_seq += 1;
    let moof_mdat = build_moof_mdat(state.video_seq as u32, 1, dts, &[raw]);
    let frag = Fragment::new(
        "0.mp4",
        state.video_seq,
        0,
        0,
        dts,
        pts,
        duration as u64,
        if keyframe {
            FragmentFlags::KEYFRAME
        } else {
            FragmentFlags::DELTA
        },
        moof_mdat,
    );
    publish_fragment(registry, broadcast, "0.mp4", "hev1", 90_000, frag);
}

fn process_aac(state: &mut ConnectionState, broadcast: &str, pes: &PesPacket, registry: &FragmentBroadcasterRegistry) {
    if !state.audio_init_emitted {
        let payload = &pes.payload;
        if payload.len() < 7 {
            return;
        }
        // Try to parse ADTS header for AAC config.
        if payload[0] != 0xFF || (payload[1] & 0xF0) != 0xF0 {
            return;
        }
        let profile = ((payload[2] >> 6) & 0x03) + 1;
        let freq_idx = (payload[2] >> 2) & 0x0F;
        let channel = ((payload[2] & 0x01) << 2) | ((payload[3] >> 6) & 0x03);
        let sample_rate = match freq_idx {
            0 => 96000,
            1 => 88200,
            2 => 64000,
            3 => 48000,
            4 => 44100,
            5 => 32000,
            6 => 24000,
            7 => 22050,
            8 => 16000,
            9 => 12000,
            10 => 11025,
            11 => 8000,
            _ => 44100,
        };
        let asc_b0 = (profile << 3) | (freq_idx >> 1);
        let asc_b1 = ((freq_idx & 1) << 7) | (channel << 3);
        let aac_params = AudioInitParams {
            asc: vec![asc_b0, asc_b1],
            timescale: sample_rate,
        };
        let mut buf = BytesMut::with_capacity(512);
        if let Err(e) = write_aac_init_segment(&mut buf, &aac_params) {
            warn!(%broadcast, error = %e, "SRT: failed to write AAC init segment");
            return;
        }
        publish_init(registry, broadcast, "1.mp4", "mp4a.40.2", sample_rate, buf.freeze());
        state.audio_init_emitted = true;
        state.audio_timescale = sample_rate;
        info!(%broadcast, %sample_rate, "SRT: audio init emitted");
    }

    // Strip ADTS header(s) and emit raw AAC frame(s).
    let payload = &pes.payload;
    let mut offset = 0;
    while offset + 7 <= payload.len() {
        if payload[offset] != 0xFF || (payload[offset + 1] & 0xF0) != 0xF0 {
            break;
        }
        let header_len = if payload[offset + 1] & 0x01 == 0 { 9 } else { 7 };
        let frame_len = (((payload[offset + 3] & 0x03) as usize) << 11)
            | ((payload[offset + 4] as usize) << 3)
            | ((payload[offset + 5] >> 5) as usize);
        if frame_len < header_len || offset + frame_len > payload.len() {
            break;
        }
        let aac_data = &payload[offset + header_len..offset + frame_len];
        let dts = pes.dts.or(pes.pts).unwrap_or(0);
        let pts = pes.pts.unwrap_or(dts);
        let duration = match state.prev_audio_dts {
            Some(prev) if dts > prev => (dts - prev) as u32,
            _ => 1024,
        };
        state.prev_audio_dts = Some(dts);

        let raw = lvqr_cmaf::RawSample {
            track_id: 2,
            dts,
            cts_offset: 0,
            duration,
            payload: Bytes::copy_from_slice(aac_data),
            keyframe: true,
        };
        state.audio_seq += 1;
        let moof_mdat = build_moof_mdat(state.audio_seq as u32, 2, dts, &[raw]);
        let frag = Fragment::new(
            "1.mp4",
            state.audio_seq,
            0,
            0,
            dts,
            pts,
            duration as u64,
            FragmentFlags::AUDIO,
            moof_mdat,
        );
        publish_fragment(registry, broadcast, "1.mp4", "mp4a.40.2", state.audio_timescale, frag);
        offset += frame_len;
    }
}

/// Split an Annex B byte stream into individual NAL units.
fn split_annex_b(data: &[u8]) -> Vec<&[u8]> {
    let mut nalus = Vec::new();
    let mut i = 0;
    while i < data.len() {
        // Find start code (0x00 0x00 0x01 or 0x00 0x00 0x00 0x01).
        let sc_len;
        if i + 3 <= data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            sc_len = 3;
        } else if i + 4 <= data.len() && data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
            sc_len = 4;
        } else {
            i += 1;
            continue;
        }
        let nalu_start = i + sc_len;
        // Find next start code or end of data.
        let mut end = nalu_start;
        while end < data.len() {
            if end + 3 <= data.len()
                && data[end] == 0
                && data[end + 1] == 0
                && (data[end + 2] == 1 || (data[end + 2] == 0 && end + 4 <= data.len() && data[end + 3] == 1))
            {
                break;
            }
            end += 1;
        }
        if end > nalu_start {
            nalus.push(&data[nalu_start..end]);
        }
        i = end;
    }
    nalus
}

/// Convert Annex B framed NALUs to AVCC (length-prefixed) format.
fn annex_b_to_avcc(data: &[u8]) -> Vec<u8> {
    let nalus = split_annex_b(data);
    let mut out = Vec::with_capacity(data.len());
    for nalu in nalus {
        if nalu.is_empty() {
            continue;
        }
        let nal_type = nalu[0] & 0x1F;
        if nal_type == 7 || nal_type == 8 {
            continue;
        }
        let len = nalu.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nalu);
    }
    out
}

/// Convert Annex B framed HEVC NALUs to length-prefixed format,
/// stripping VPS/SPS/PPS (stored in the init segment).
fn annex_b_to_hvcc(data: &[u8]) -> Vec<u8> {
    let nalus = split_annex_b(data);
    let mut out = Vec::with_capacity(data.len());
    for nalu in nalus {
        if nalu.len() < 2 {
            continue;
        }
        let nal_type = (nalu[0] >> 1) & 0x3F;
        if nal_type == 32 || nal_type == 33 || nal_type == 34 {
            continue;
        }
        let len = nalu.len() as u32;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(nalu);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annex_b_to_hvcc_strips_param_sets() {
        // VPS (type 32 = 0x40>>1), SPS (type 33 = 0x42>>1), PPS (type 34 = 0x44>>1)
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x40, 0x01, 0xAA]); // VPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x42, 0x01, 0xBB]); // SPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x44, 0x01, 0xCC]); // PPS
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x26, 0x01, 0xDD]); // IDR slice
        let out = annex_b_to_hvcc(&data);
        // Only the IDR slice should remain, length-prefixed.
        assert_eq!(out.len(), 4 + 3); // 4-byte length + 3-byte NAL
        assert_eq!(&out[0..4], &3u32.to_be_bytes());
        assert_eq!(&out[4..7], &[0x26, 0x01, 0xDD]);
    }

    #[test]
    fn annex_b_to_avcc_strips_sps_pps() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0xAA]); // SPS (type 7)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xBB]); // PPS (type 8)
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xCC]); // IDR slice (type 5)
        let out = annex_b_to_avcc(&data);
        assert_eq!(out.len(), 4 + 2);
        assert_eq!(&out[4..6], &[0x65, 0xCC]);
    }

    /// Synthetic H.264 PES packets driven through `process_h264`
    /// publish one init segment + one keyframe fragment through the
    /// shared `FragmentBroadcasterRegistry`. Pins the SRT -> broadcaster
    /// emit contract: broadcaster-side subscribers see the same bytes
    /// the archive + HLS + DASH drains see at runtime.
    #[tokio::test]
    async fn h264_pes_publishes_init_and_keyframe_fragment_on_registry() {
        use lvqr_codec::ts::PesPacket;
        use lvqr_fragment::{FragmentMeta, FragmentStream};

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("srt_registry", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut sub = bc.subscribe();

        let mut state = ConnectionState {
            demux: TsDemuxer::new(),
            video_init_emitted: false,
            audio_init_emitted: false,
            video_seq: 0,
            audio_seq: 0,
            sps: None,
            pps: None,
            vps: None,
            prev_video_dts: None,
            prev_audio_dts: None,
            audio_timescale: 44100,
        };

        let mut annex_b = Vec::new();
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1F]); // SPS
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xCE, 0x38, 0x80]); // PPS
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65, 0xAA, 0xBB, 0xCC]); // IDR

        let pes = PesPacket {
            pid: 256,
            stream_type: StreamType::H264,
            pts: Some(90000),
            dts: Some(90000),
            payload: annex_b,
        };

        process_h264(&mut state, "srt_registry", &pes, &registry);

        assert!(bc.meta().init_segment.is_some(), "broadcaster carries AVC init bytes");
        let frag = sub.next_fragment().await.expect("keyframe fragment delivered");
        assert!(frag.flags.keyframe);
        assert_eq!(frag.track_id, "0.mp4");
    }

    #[test]
    fn split_annex_b_handles_3_and_4_byte_start_codes() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0x00, 0x00, 0x01, 0xAA, 0xBB]); // 3-byte SC
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0xCC]); // 4-byte SC
        let nalus = split_annex_b(&data);
        assert_eq!(nalus.len(), 2);
        assert_eq!(nalus[0], &[0xAA, 0xBB]);
        assert_eq!(nalus[1], &[0xCC]);
    }

    /// Synthetic HEVC PES driven through `process_hevc` publishes an
    /// init segment + one keyframe fragment through the shared
    /// `FragmentBroadcasterRegistry`. Mirrors the H.264 sibling test
    /// at `h264_pes_publishes_init_and_keyframe_fragment_on_registry`.
    /// Pins the SRT -> broadcaster contract for the HEVC ingest path
    /// session 152's PMT 0x86 SCTE-35 work landed in the same
    /// dispatch loop, so the contract here is load-bearing.
    #[tokio::test]
    async fn hevc_pes_publishes_init_and_keyframe_fragment_on_registry() {
        use lvqr_codec::ts::PesPacket;
        use lvqr_fragment::{FragmentMeta, FragmentStream};

        // Real x265 parameter sets (320x240, Main profile, level 2.0).
        // Same fixtures as the lvqr-cli SRT->HLS HEVC e2e at
        // `crates/lvqr-cli/tests/srt_hls_e2e.rs:193-202`.
        const HEVC_VPS: &[u8] = &[
            0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00,
            0x03, 0x00, 0x3c, 0x95, 0x94, 0x09,
        ];
        const HEVC_SPS: &[u8] = &[
            0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c,
            0xa0, 0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02,
            0x00, 0x00, 0x03, 0x00, 0x3c, 0x10,
        ];
        const HEVC_PPS: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("srt_hevc", "0.mp4", FragmentMeta::new("hev1", 90_000));
        let mut sub = bc.subscribe();

        let mut state = ConnectionState {
            demux: TsDemuxer::new(),
            video_init_emitted: false,
            audio_init_emitted: false,
            video_seq: 0,
            audio_seq: 0,
            sps: None,
            pps: None,
            vps: None,
            prev_video_dts: None,
            prev_audio_dts: None,
            audio_timescale: 44100,
        };

        // Build an Annex-B HEVC access unit: VPS + SPS + PPS + IDR slice.
        // IDR_W_RADL nal_unit_type = 19 -> first byte (19 << 1) | 0 = 0x26.
        let mut annex_b = Vec::new();
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annex_b.extend_from_slice(HEVC_VPS);
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annex_b.extend_from_slice(HEVC_SPS);
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annex_b.extend_from_slice(HEVC_PPS);
        annex_b.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        annex_b.extend_from_slice(&[0x26, 0x01, 0xAF, 0x09, 0x40, 0xDE, 0xAD]);

        let pes = PesPacket {
            pid: 256,
            stream_type: StreamType::H265,
            pts: Some(90_000),
            dts: Some(90_000),
            payload: annex_b,
        };

        process_hevc(&mut state, "srt_hevc", &pes, &registry);

        assert!(bc.meta().init_segment.is_some(), "broadcaster carries HEVC init bytes");
        let frag = sub.next_fragment().await.expect("HEVC keyframe fragment delivered");
        assert!(frag.flags.keyframe, "first emitted HEVC fragment should be a keyframe");
        assert_eq!(frag.track_id, "0.mp4");
        assert!(
            state.video_init_emitted,
            "video_init_emitted flag flips after first keyframe"
        );
    }

    /// Synthetic AAC ADTS PES driven through `process_aac` publishes
    /// an audio init segment + one audio fragment through the
    /// `FragmentBroadcasterRegistry`. Pins the SRT -> broadcaster
    /// contract for the AAC ingest path: ADTS header parse, frame
    /// extraction, init emission gated on first ADTS, fragment
    /// emission per ADTS frame.
    #[tokio::test]
    async fn aac_adts_publishes_init_and_audio_fragment_on_registry() {
        use lvqr_codec::ts::PesPacket;
        use lvqr_fragment::{FragmentMeta, FragmentStream};

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("srt_aac", "1.mp4", FragmentMeta::new("mp4a.40.2", 44_100));
        let mut sub = bc.subscribe();

        let mut state = ConnectionState {
            demux: TsDemuxer::new(),
            video_init_emitted: false,
            audio_init_emitted: false,
            video_seq: 0,
            audio_seq: 0,
            sps: None,
            pps: None,
            vps: None,
            prev_video_dts: None,
            prev_audio_dts: None,
            audio_timescale: 0,
        };

        // Build a minimal ADTS frame: header (7 bytes) + 8 bytes of
        // raw AAC payload. AAC profile 2 (LC), freq_idx 4 (44100 Hz),
        // channel_config 2 (stereo). frame_length must include the
        // header, so frame_length = 7 + 8 = 15.
        // ADTS layout per ISO/IEC 13818-7:
        //   syncword: 12 bits = 0xFFF
        //   ID: 1 bit (MPEG-4 = 0)
        //   layer: 2 bits = 0
        //   protection_absent: 1 bit = 1 (no CRC -> 7-byte header)
        //   profile: 2 bits = 1 (LC = profile 2 minus 1)
        //   sampling_frequency_index: 4 bits = 4 (44100)
        //   private_bit: 1 bit = 0
        //   channel_configuration: 3 bits = 2 (stereo)
        //   original_copy: 1 bit = 0
        //   home: 1 bit = 0
        //   copyright_id_bit: 1 bit = 0
        //   copyright_id_start: 1 bit = 0
        //   frame_length: 13 bits = 15
        //   buffer_fullness: 11 bits = 0x7FF (VBR)
        //   number_of_raw_data_blocks_in_frame: 2 bits = 0
        let frame_len: u16 = 15;
        let header = [
            0xFF,
            0xF1,
            // (profile << 6) | (freq_idx << 2) | (channel_high)
            // profile=1 (LC), freq_idx=4 (44100), channel high bit=0
            (1 << 6) | (4 << 2),
            // (channel_low << 6) | (frame_len_top << 5)
            (2 << 6) | ((frame_len >> 11) as u8 & 0x03),
            (frame_len >> 3) as u8,
            (((frame_len & 0x07) as u8) << 5) | 0x1F,
            0xFC,
        ];
        let mut adts = Vec::new();
        adts.extend_from_slice(&header);
        adts.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03, 0x04]);

        let pes = PesPacket {
            pid: 257,
            stream_type: StreamType::Aac,
            pts: Some(0),
            dts: Some(0),
            payload: adts,
        };

        process_aac(&mut state, "srt_aac", &pes, &registry);

        assert!(
            state.audio_init_emitted,
            "audio_init_emitted flag flips after first ADTS"
        );
        assert_eq!(state.audio_timescale, 44_100, "ADTS freq_idx 4 maps to 44100 Hz");
        assert!(bc.meta().init_segment.is_some(), "broadcaster carries AAC init bytes");
        let frag = sub.next_fragment().await.expect("AAC fragment delivered");
        assert_eq!(frag.track_id, "1.mp4");
        assert_eq!(state.audio_seq, 1, "one ADTS frame in -> one audio fragment out");
    }

    /// A CRC-valid splice_info_section flowing through `process_scte35`
    /// publishes one SCTE-35 event onto the broadcast's reserved
    /// `"scte35"` track via `lvqr_ingest::publish_scte35`. The wire
    /// shape mirrors the canonical fixture used by
    /// `crates/lvqr-cli/tests/scte35_hls_dash_e2e.rs` and the session
    /// 155 `scte35-rtmp-push` bin.
    #[tokio::test]
    async fn scte35_section_with_valid_crc_publishes_event_on_registry() {
        use lvqr_codec::ts::Scte35Section;
        use lvqr_fragment::{FragmentMeta, FragmentStream, SCTE35_TRACK};

        let registry = FragmentBroadcasterRegistry::new();
        // The reserved scte35 track is keyed by SCTE35_TRACK; the
        // bridge calls `publish_scte35` which auto-creates the
        // broadcaster on first emit. Pre-create it here to attach a
        // subscriber before `process_scte35` fires.
        let bc = registry.get_or_create("srt_scte35", SCTE35_TRACK, FragmentMeta::new("scte35", 90_000));
        let mut sub = bc.subscribe();

        let section_bytes = canonical_splice_insert_section();
        let section = Scte35Section {
            pid: 0x1FFB,
            raw: section_bytes,
        };

        process_scte35("srt_scte35", &section, &registry);

        let frag = sub.next_fragment().await.expect("SCTE-35 event published");
        assert_eq!(frag.track_id, SCTE35_TRACK);
        assert!(
            !frag.payload.is_empty(),
            "scte35 fragment carries the raw section bytes"
        );
    }

    /// A splice_info_section with a corrupted CRC drops silently from
    /// `process_scte35` (the broadcaster sees no fragment). The
    /// `lvqr_scte35_drops_total{reason="crc"}` counter increments;
    /// asserting on the metrics surface is out of scope for this
    /// in-crate test (covered by `crates/lvqr-cli/tests` integration).
    #[tokio::test]
    async fn scte35_section_with_invalid_crc_drops() {
        use lvqr_codec::ts::Scte35Section;
        use lvqr_fragment::{FragmentMeta, FragmentStream, SCTE35_TRACK};

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("srt_scte35_bad", SCTE35_TRACK, FragmentMeta::new("scte35", 90_000));
        let mut sub = bc.subscribe();

        let mut bytes = canonical_splice_insert_section();
        // Flip a CRC byte (last 4 bytes are the CRC-32/MPEG-2 trailer).
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;

        let section = Scte35Section {
            pid: 0x1FFB,
            raw: bytes,
        };

        process_scte35("srt_scte35_bad", &section, &registry);

        // No fragment should be published. Race the subscriber's
        // next_fragment against a short timeout to assert silence.
        let timed = tokio::time::timeout(std::time::Duration::from_millis(50), sub.next_fragment()).await;
        assert!(timed.is_err(), "invalid-CRC SCTE-35 must not publish a fragment");
    }

    /// Build a CRC-valid splice_insert section equivalent to the
    /// canonical `lvqr-test-utils::scte35::splice_insert_section_bytes`
    /// fixture (event_id 0xCAFEBABE, pts 8_100_000, duration 2_700_000).
    /// Inlined here so lvqr-srt does not pull lvqr-test-utils as a
    /// dev-dep (would create a circular dev-dep through lvqr-cli).
    fn canonical_splice_insert_section() -> Vec<u8> {
        // splice_info_section per ANSI/SCTE 35-2024 section 8.1
        // built up by hand: 14-byte prefix, 20-byte splice_insert
        // command body (event_id u32 + cancel + flags + pts + 5-byte
        // splice_time + 5-byte break_duration + program_id +
        // avail_num + avails_expected), no descriptors, 4-byte
        // CRC-32/MPEG-2 trailer.
        // table_id, section_syntax_indicator + private_indicator +
        // reserved + section_length top
        let mut bytes = vec![
            0xFC, // table_id
            0x30, // section_syntax_indicator=0, private_indicator=0, reserved=11, section_length top
            0x25, // section_length low (37 bytes after this point)
            0x00, // protocol_version
            0x00, // encrypted_packet=0, encryption_algorithm=0, pts_adjustment top
            0x00, 0x00, 0x00, 0x00, // pts_adjustment rest (0)
            0x00, // cw_index
            0x00, // tier top
            0x00, // tier low + splice_command_length top
            0x14, // splice_command_length low (20 = splice_insert size)
            0x05, // splice_command_type = splice_insert (0x05)
        ];
        // splice_insert body: event_id (u32 BE), cancel flag, flags,
        // splice_time, break_duration, program_id, avail_num,
        // avails_expected.
        bytes.extend_from_slice(&0xCAFE_BABE_u32.to_be_bytes()); // event_id
        bytes.push(0x00); // splice_event_cancel_indicator=0 + reserved
        // out_of_network=1, program_splice=1, duration=1, immediate=0,
        // event_id_compliance=1, reserved
        bytes.push(0xF0);
        // splice_time: time_specified_flag=1, reserved=0x3F, pts (33 bits split)
        let pts: u64 = 8_100_000;
        bytes.push(0xF0 | ((pts >> 32) as u8 & 0x01) << 1 | 0x01);
        bytes.push((pts >> 24) as u8);
        bytes.push((pts >> 16) as u8);
        bytes.push((pts >> 8) as u8);
        bytes.push(pts as u8);
        // break_duration: auto_return=1, reserved=0x3F, duration (33 bits)
        let dur: u64 = 2_700_000;
        bytes.push(0xFE | ((dur >> 32) as u8 & 0x01));
        bytes.push((dur >> 24) as u8);
        bytes.push((dur >> 16) as u8);
        bytes.push((dur >> 8) as u8);
        bytes.push(dur as u8);
        // unique_program_id (u16 BE), avail_num, avails_expected
        bytes.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        // descriptor_loop_length (u16 BE) = 0
        bytes.extend_from_slice(&[0x00, 0x00]);

        // CRC-32/MPEG-2 over [..len-4]; section_length already
        // committed to a layout that includes the 4-byte CRC.
        let mut c: u32 = 0xFFFF_FFFF;
        for &b in &bytes {
            c ^= (b as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04C1_1DB7
                } else {
                    c << 1
                };
            }
        }
        bytes.push((c >> 24) as u8);
        bytes.push((c >> 16) as u8);
        bytes.push((c >> 8) as u8);
        bytes.push(c as u8);

        // Round-trip sanity: the canonical lvqr_codec parser must
        // accept what we just built. Crashes here would indicate a
        // hand-rolled-builder bug, not a parser regression.
        debug_assert!(
            lvqr_codec::parse_splice_info_section(&bytes).is_ok(),
            "canonical_splice_insert_section produced bytes the parser rejected"
        );

        bytes
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases: 256, ..Default::default() })]

        /// `split_annex_b` is the lowest-level parser surface in the
        /// SRT ingest path. It is fed arbitrary bytes from the wire on
        /// every PES extraction, so panic-freedom on adversarial input
        /// is load-bearing. The proptest generates random byte slices
        /// up to 4 KB and asserts (a) the call does not panic, and
        /// (b) each emitted NAL is a contiguous slice of the input.
        #[test]
        fn split_annex_b_is_panic_free_and_slices_within_input(input in proptest::collection::vec(0u8..=255u8, 0..4096)) {
            let nalus = split_annex_b(&input);
            for nalu in &nalus {
                // Every emitted NAL slice must be a subset of `input`.
                // We assert this by checking the slice's address range.
                let nalu_start = nalu.as_ptr() as usize;
                let input_start = input.as_ptr() as usize;
                let input_end = input_start + input.len();
                proptest::prop_assert!(
                    nalu_start >= input_start && nalu_start + nalu.len() <= input_end,
                    "emitted NAL slice escapes input buffer"
                );
                proptest::prop_assert!(!nalu.is_empty(), "split_annex_b never emits an empty NAL");
            }
        }

        /// `annex_b_to_avcc` runs after `split_annex_b` on the H.264
        /// path. Panic-freedom matters because a publisher with a
        /// malformed stream cannot kill the per-connection task.
        #[test]
        fn annex_b_to_avcc_is_panic_free(input in proptest::collection::vec(0u8..=255u8, 0..4096)) {
            let avcc = annex_b_to_avcc(&input);
            // AVCC output is length-prefixed: every NAL is preceded by a
            // u32 BE length. Walk the output to verify well-formedness.
            let mut offset = 0;
            while offset + 4 <= avcc.len() {
                let len = u32::from_be_bytes([
                    avcc[offset],
                    avcc[offset + 1],
                    avcc[offset + 2],
                    avcc[offset + 3],
                ]) as usize;
                proptest::prop_assert!(
                    offset + 4 + len <= avcc.len(),
                    "AVCC length prefix overruns buffer"
                );
                offset += 4 + len;
            }
            proptest::prop_assert!(offset == avcc.len(), "AVCC output has trailing bytes");
        }

        /// `annex_b_to_hvcc` is the HEVC sibling. Same panic-freedom
        /// + length-prefix integrity contract.
        #[test]
        fn annex_b_to_hvcc_is_panic_free(input in proptest::collection::vec(0u8..=255u8, 0..4096)) {
            let hvcc = annex_b_to_hvcc(&input);
            let mut offset = 0;
            while offset + 4 <= hvcc.len() {
                let len = u32::from_be_bytes([
                    hvcc[offset],
                    hvcc[offset + 1],
                    hvcc[offset + 2],
                    hvcc[offset + 3],
                ]) as usize;
                proptest::prop_assert!(
                    offset + 4 + len <= hvcc.len(),
                    "HVCC length prefix overruns buffer"
                );
                offset += 4 + len;
            }
            proptest::prop_assert!(offset == hvcc.len(), "HVCC output has trailing bytes");
        }
    }
}
