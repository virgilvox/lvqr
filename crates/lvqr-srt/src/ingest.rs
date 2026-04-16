//! SRT ingest server: listener + per-connection MPEG-TS demux
//! + Fragment emission.

use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use futures_util::StreamExt;
use lvqr_cmaf::{AudioInitParams, VideoInitParams, build_moof_mdat, write_aac_init_segment, write_avc_init_segment};
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
}

impl SrtIngestServer {
    pub fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    /// Run the listener loop until `shutdown` fires. Each accepted
    /// connection spawns a task that feeds TS bytes through a
    /// `TsDemuxer` and converts PES packets to Fragments.
    pub async fn run(
        &self,
        observer: Option<SharedFragmentObserver>,
        events: EventBus,
        shutdown: CancellationToken,
    ) -> Result<(), std::io::Error> {
        let (mut listener, mut incoming) = srt_tokio::SrtListener::builder().bind(self.addr).await?;

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
    /// Cached SPS bytes for H.264 init segment generation.
    sps: Option<Vec<u8>>,
    /// Cached PPS bytes.
    pps: Option<Vec<u8>>,
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
        StreamType::H265 => {
            debug!(%broadcast, "SRT HEVC not yet wired; dropping PES");
        }
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

    let raw = lvqr_cmaf::RawSample {
        track_id: 1,
        dts,
        cts_offset: pts.wrapping_sub(dts) as i32,
        duration: 3000,
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
        3000,
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

        let raw = lvqr_cmaf::RawSample {
            track_id: 2,
            dts,
            cts_offset: 0,
            duration: 1024,
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
            1024,
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
