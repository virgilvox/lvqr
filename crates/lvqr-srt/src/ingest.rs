//! SRT ingest server: listener + per-connection MPEG-TS demux
//! + Fragment emission.

use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use lvqr_cmaf::{
    AudioInitParams, HevcInitParams, VideoInitParams, build_moof_mdat, write_aac_init_segment, write_avc_init_segment,
    write_hevc_init_segment,
};
use lvqr_codec::hevc::{self as hevc_codec, HevcNalType};
use lvqr_codec::ts::{PesPacket, StreamType, TsDemuxer};
use lvqr_core::{EventBus, RelayEvent};
use lvqr_fragment::{Fragment, FragmentFlags};
use lvqr_ingest::SharedFragmentObserver;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// SRT ingest server. Bind to a UDP port, accept SRT connections,
/// demux MPEG-TS, and emit Fragments.
pub struct SrtIngestServer {
    addr: SocketAddr,
    pre_bound: Option<tokio::net::UdpSocket>,
}

impl SrtIngestServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr, pre_bound: None }
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
    pub async fn run(
        self,
        observer: Option<SharedFragmentObserver>,
        events: EventBus,
        shutdown: CancellationToken,
    ) -> Result<(), std::io::Error> {
        let builder = srt_tokio::SrtListener::builder();
        let (mut listener, mut incoming) = match self.pre_bound {
            Some(socket) => builder.socket(socket).bind(self.addr).await?,
            None => builder.bind(self.addr).await?,
        };

        info!(addr = %self.addr, "SRT ingest bound");

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
                    let broadcast = stream_id.unwrap_or_else(|| "srt/default".into());
                    let remote = req.remote();
                    info!(%broadcast, %remote, "SRT connection request");

                    let socket = match req.accept(None).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!(error = %e, "SRT accept failed");
                            continue;
                        }
                    };

                    let obs = observer.clone();
                    let ev = events.clone();
                    let bc = broadcast.clone();
                    let conn_shutdown = shutdown.clone();
                    tokio::spawn(async move {
                        handle_connection(socket, &bc, obs.as_ref(), &ev, conn_shutdown).await;
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
    observer: Option<&SharedFragmentObserver>,
    events: &EventBus,
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
                            process_pes(&mut state, broadcast, &pes, observer);
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

fn process_pes(
    state: &mut ConnectionState,
    broadcast: &str,
    pes: &PesPacket,
    observer: Option<&SharedFragmentObserver>,
) {
    match pes.stream_type {
        StreamType::H264 => process_h264(state, broadcast, pes, observer),
        StreamType::Aac => process_aac(state, broadcast, pes, observer),
        StreamType::H265 => process_hevc(state, broadcast, pes, observer),
        StreamType::Unknown(st) => {
            debug!(%broadcast, stream_type = st, "SRT unknown stream type; dropping PES");
        }
    }
}

fn process_h264(
    state: &mut ConnectionState,
    broadcast: &str,
    pes: &PesPacket,
    observer: Option<&SharedFragmentObserver>,
) {
    let obs = match observer {
        Some(o) => o,
        None => return,
    };
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
        let init = buf.freeze();
        obs.on_init(broadcast, "0.mp4", 90_000, init);
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
    obs.on_fragment(broadcast, "0.mp4", &frag);
}

fn process_hevc(
    state: &mut ConnectionState,
    broadcast: &str,
    pes: &PesPacket,
    observer: Option<&SharedFragmentObserver>,
) {
    let obs = match observer {
        Some(o) => o,
        None => return,
    };
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
        let init = buf.freeze();
        obs.on_init(broadcast, "0.mp4", 90_000, init);
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
    obs.on_fragment(broadcast, "0.mp4", &frag);
}

fn process_aac(
    state: &mut ConnectionState,
    broadcast: &str,
    pes: &PesPacket,
    observer: Option<&SharedFragmentObserver>,
) {
    let obs = match observer {
        Some(o) => o,
        None => return,
    };

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
        let init = buf.freeze();
        obs.on_init(broadcast, "1.mp4", sample_rate, init);
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
        obs.on_fragment(broadcast, "1.mp4", &frag);
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
}
