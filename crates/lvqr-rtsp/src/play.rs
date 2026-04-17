//! RTSP PLAY egress: drain a broadcaster, packetize fragments into
//! RTP, and write interleaved frames back to the client socket.
//!
//! Composition of session-61 (RTP packetizers) + session-62 (fmp4
//! mdat extractor + parameter-set extractor + SDP builder). The
//! drain task owns only a [`BroadcasterStream`] receiver and an
//! mpsc sender to the connection's writer loop; it never holds a
//! strong `Arc<FragmentBroadcaster>`. That pins the invariant the
//! archive / HLS / DASH drains already document: a keepalive Arc
//! would keep the `broadcast::Sender` alive and `recv()` would
//! never see `Closed` after every ingest clone dropped.
//!
//! Scope of the first pass: H.264 video only. HEVC + audio land
//! once the LL-HLS + DASH conformance story gives the extra codec
//! surfaces a testable home. A non-H.264 broadcaster here just
//! produces no RTP (the drain exits without emitting).

use lvqr_fragment::{FragmentBroadcasterRegistry, FragmentStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::fmp4;
use crate::rtp::{H264Packetizer, HevcPacketizer};

/// Default dynamic RTP payload type for the video track. Both the
/// H.264 and HEVC SDP blocks emit PT 96 because only one of the two
/// is advertised per broadcast; a compliant client binds that track
/// to the negotiated interleaved channel on SETUP.
const VIDEO_PAYLOAD_TYPE: u8 = 96;
const H264_PAYLOAD_TYPE: u8 = VIDEO_PAYLOAD_TYPE;
const HEVC_PAYLOAD_TYPE: u8 = VIDEO_PAYLOAD_TYPE;

/// Sequence-number seed for the video RTP stream. RFC 3550 suggests
/// a random initial value; a constant is fine for tests and the
/// session identifier entropy makes the stream unique across
/// concurrent PLAYs in practice.
const INITIAL_RTP_SEQUENCE: u16 = 1000;

/// Opaque SSRC stamped on every RTP packet the drain emits. Per
/// RFC 3550 the SSRC must be unique per session on the same
/// network path; LVQR currently runs one-server-per-session so a
/// fixed value is acceptable. A follow-up change can derive this
/// from the session id if client-side SSRC collision detection
/// ever trips.
const DEFAULT_SSRC: u32 = 0xDEAD_BEEF;

/// Wrap a fully formed RTP packet in an RTSP interleaved TCP frame
/// (`$ channel length rtp_packet`) and push it onto the connection
/// writer. Asserts the packet fits in the 16-bit length field; a
/// larger packet is a bug in the caller (the packetizer enforces
/// MTU well below 65535 by default).
async fn send_interleaved(writer_tx: &mpsc::Sender<Vec<u8>>, channel: u8, rtp: &[u8]) -> Result<(), ()> {
    let len = rtp.len();
    assert!(len <= u16::MAX as usize, "RTP packet exceeds interleaved frame size");
    let mut frame = Vec::with_capacity(4 + len);
    frame.push(0x24);
    frame.push(channel);
    frame.extend_from_slice(&(len as u16).to_be_bytes());
    frame.extend_from_slice(rtp);
    writer_tx.send(frame).await.map_err(|_| ())
}

/// Drive one PLAY session over the registry's H.264 broadcaster.
///
/// The task:
/// 1. Looks up `(broadcast, "0.mp4")` on the registry. A miss logs
///    and exits; the client still gets an OK PLAY response, just no
///    media. A real client that PLAYs before any publisher will
///    observe a stalled stream and eventually TEARDOWN.
/// 2. Subscribes to the broadcaster and refreshes meta so the init
///    bytes are available.
/// 3. If the init bytes decode as AVC, extracts SPS + PPS and emits
///    them as single-NAL RTP packets at timestamp 0 before the first
///    real fragment. A client that started decoding on the first
///    keyframe needs these to construct its decoder.
/// 4. Loops on `next_fragment().await`: extracts the `mdat` body,
///    splits it into NALs via AVCC length prefixes, and runs each
///    NAL through the H.264 packetizer. The RTP timestamp is the
///    fragment's PTS cast to `u32`; RFC 3550 timestamp wrap is the
///    client's problem.
///
/// Terminates when any of the following fires:
/// * The connection cancel token is triggered (server shutdown or
///   TEARDOWN on the owning connection).
/// * The broadcaster closes (every producer clone dropped).
/// * `writer_tx` is closed (the connection writer task exited; the
///   socket is gone so further RTP writes would panic on TCP).
pub async fn play_drain_h264(
    broadcast: String,
    rtp_channel: u8,
    registry: FragmentBroadcasterRegistry,
    writer_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    let Some(bc) = registry.get(&broadcast, "0.mp4") else {
        debug!(%broadcast, "play_drain: no video broadcaster; exiting before first emit");
        return;
    };
    let mut sub = bc.subscribe();

    // Extract parameter sets once up-front. Refreshing meta here is
    // cheap and catches the common case where PLAY arrives after
    // publish_init (the producer set the init segment first).
    sub.refresh_meta();
    let params = sub
        .meta()
        .init_segment
        .as_ref()
        .and_then(|init| lvqr_cmaf::extract_avc_parameter_sets(init));

    let mut packetizer = H264Packetizer::new(DEFAULT_SSRC, H264_PAYLOAD_TYPE, INITIAL_RTP_SEQUENCE);

    if let Some(ref params) = params {
        // Each parameter set is a single NAL emitted as its own
        // single-NAL RTP packet. Marker bit stays clear; the real
        // access unit's last NAL (a few packets later) sets it.
        for sps in &params.sps_list {
            for pkt in packetizer.packetize(sps, 0, false) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
        for pps in &params.pps_list {
            for pkt in packetizer.packetize(pps, 0, false) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
    }

    info!(%broadcast, "play_drain: video egress started");

    loop {
        let fragment = tokio::select! {
            _ = cancel.cancelled() => break,
            f = sub.next_fragment() => f,
        };
        let Some(fragment) = fragment else {
            break; // broadcaster closed
        };

        let Some(body) = fmp4::extract_mdat_body(&fragment.payload) else {
            continue;
        };
        let nalus = fmp4::split_avcc_nalus(body);
        let rtp_ts = fragment.pts as u32;
        let last = nalus.len().saturating_sub(1);
        for (i, nal) in nalus.iter().enumerate() {
            let end_of_au = i == last;
            for pkt in packetizer.packetize(nal, rtp_ts, end_of_au) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
    }
    info!(%broadcast, "play_drain: video egress terminated");
}

/// Drive one PLAY session over the registry's HEVC broadcaster.
///
/// Same shape as [`play_drain_h264`]: subscribes, re-injects the
/// parameter sets before the first IDR, loops on fragments. HEVC
/// carries three parameter sets (VPS + SPS + PPS) instead of two;
/// the packetizer emits single-NAL packets for fragments that fit
/// the MTU and FU (type 49) otherwise. NAL type extraction is
/// handled by [`HevcPacketizer`] itself.
///
/// If the broadcaster's init segment does not decode as HEVC, the
/// drain exits without emitting. A caller that cannot tell which
/// video codec is on the broadcaster should try
/// [`play_drain_h264`] first and fall back to this function.
pub async fn play_drain_hevc(
    broadcast: String,
    rtp_channel: u8,
    registry: FragmentBroadcasterRegistry,
    writer_tx: mpsc::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    let Some(bc) = registry.get(&broadcast, "0.mp4") else {
        debug!(%broadcast, "play_drain (hevc): no video broadcaster; exiting before first emit");
        return;
    };
    let mut sub = bc.subscribe();
    sub.refresh_meta();
    let params = sub
        .meta()
        .init_segment
        .as_ref()
        .and_then(|init| lvqr_cmaf::extract_hevc_parameter_sets(init));

    let mut packetizer = HevcPacketizer::new(DEFAULT_SSRC, HEVC_PAYLOAD_TYPE, INITIAL_RTP_SEQUENCE);

    if let Some(ref params) = params {
        // VPS first, then SPS, then PPS. Each one is a single NAL
        // emitted as its own single-NAL RTP packet.
        for vps in &params.vps_list {
            for pkt in packetizer.packetize(vps, 0, false) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
        for sps in &params.sps_list {
            for pkt in packetizer.packetize(sps, 0, false) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
        for pps in &params.pps_list {
            for pkt in packetizer.packetize(pps, 0, false) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
    } else {
        debug!(%broadcast, "play_drain (hevc): init segment did not decode as HEVC; exiting");
        return;
    }

    info!(%broadcast, "play_drain: HEVC egress started");

    loop {
        let fragment = tokio::select! {
            _ = cancel.cancelled() => break,
            f = sub.next_fragment() => f,
        };
        let Some(fragment) = fragment else {
            break;
        };

        let Some(body) = fmp4::extract_mdat_body(&fragment.payload) else {
            continue;
        };
        let nalus = fmp4::split_avcc_nalus(body);
        let rtp_ts = fragment.pts as u32;
        let last = nalus.len().saturating_sub(1);
        for (i, nal) in nalus.iter().enumerate() {
            let end_of_au = i == last;
            for pkt in packetizer.packetize(nal, rtp_ts, end_of_au) {
                if send_interleaved(&writer_tx, rtp_channel, &pkt).await.is_err() {
                    return;
                }
            }
        }
    }
    info!(%broadcast, "play_drain: HEVC egress terminated");
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use lvqr_cmaf::{RawSample, VideoInitParams, build_moof_mdat, write_avc_init_segment};
    use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};

    /// Sample SPS + PPS taken from the lvqr-cmaf x264 corpus so the
    /// produced init segment parses cleanly back through
    /// extract_avc_parameter_sets.
    const SPS: &[u8] = &[0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00];
    const PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

    fn avcc_nal(nal: &[u8]) -> Vec<u8> {
        let mut v = (nal.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(nal);
        v
    }

    /// Drive a broadcaster with one IDR fragment through play_drain_h264
    /// and verify the drain writes SPS + PPS re-injection packets followed
    /// by the IDR, all wrapped as interleaved frames on the requested
    /// channel with H.264 RTP payload type.
    #[tokio::test]
    async fn play_drain_re_injects_params_and_packetizes_fragment() {
        use bytes::BytesMut;

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("live/test", "0.mp4", FragmentMeta::new("avc1", 90_000));

        // Populate the broadcaster with a real AVC init segment so the
        // drain can recover SPS / PPS.
        let mut init = BytesMut::new();
        write_avc_init_segment(
            &mut init,
            &VideoInitParams {
                sps: SPS.to_vec(),
                pps: PPS.to_vec(),
                width: 1280,
                height: 720,
                timescale: 90_000,
            },
        )
        .expect("write init");
        bc.set_init_segment(init.freeze());

        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(64);
        let cancel = CancellationToken::new();

        // Spawn the drain. The fixed SSRC and starting sequence make
        // the output deterministic.
        let drain_cancel = cancel.clone();
        let handle = tokio::spawn(play_drain_h264(
            "live/test".to_string(),
            0, // RTP channel
            registry.clone(),
            writer_tx,
            drain_cancel,
        ));

        // Give the drain a tick to subscribe + emit parameter sets.
        // The first two interleaved frames must be SPS and PPS.
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            let sps_frame = writer_rx.recv().await.expect("sps frame");
            let pps_frame = writer_rx.recv().await.expect("pps frame");
            (sps_frame, pps_frame)
        })
        .await
        .expect("drain emitted params");

        // Emit one IDR fragment through the broadcaster.
        let idr_nal = vec![0x65, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let sample = RawSample {
            track_id: 1,
            dts: 3000,
            cts_offset: 0,
            duration: 3000,
            payload: Bytes::from(avcc_nal(&idr_nal)),
            keyframe: true,
        };
        let fragment_payload = build_moof_mdat(1, 1, 3000, std::slice::from_ref(&sample));
        bc.emit(Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            3000,
            3000,
            3000,
            FragmentFlags::KEYFRAME,
            fragment_payload,
        ));

        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), writer_rx.recv())
            .await
            .expect("IDR frame arrives")
            .expect("channel not closed");

        // Interleaved frame header.
        assert_eq!(frame[0], 0x24, "RTSP interleaved magic");
        assert_eq!(frame[1], 0, "RTP channel");
        let rtp_len = u16::from_be_bytes([frame[2], frame[3]]) as usize;
        assert_eq!(rtp_len, frame.len() - 4, "interleaved length matches body");

        // RTP header.
        let rtp = &frame[4..];
        let hdr = crate::rtp::parse_rtp_header(rtp).expect("valid RTP header");
        assert_eq!(hdr.payload_type, H264_PAYLOAD_TYPE);
        assert_eq!(hdr.ssrc, DEFAULT_SSRC);
        assert_eq!(hdr.timestamp, 3000, "RTP timestamp matches fragment PTS");
        assert!(hdr.marker, "IDR is a single NAL -> marker on its only packet");

        // RTP payload is the IDR NAL verbatim (single-NAL packet, fits MTU).
        let rtp_payload = &rtp[hdr.header_len..];
        assert_eq!(rtp_payload, &idr_nal[..]);

        // Stop the drain cleanly.
        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn play_drain_exits_when_broadcaster_missing() {
        let registry = FragmentBroadcasterRegistry::new();
        let (writer_tx, _writer_rx) = mpsc::channel::<Vec<u8>>(4);
        let cancel = CancellationToken::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            play_drain_h264("no/such/broadcast".into(), 0, registry, writer_tx, cancel),
        )
        .await
        .expect("drain exits promptly when broadcaster is missing");
    }

    // --- HEVC drain tests ---

    /// Real x265 HEVC Main 3.0 NAL units. Same capture used by
    /// lvqr-cmaf's init.rs tests so the whole init-extract-reinjection
    /// round-trip is exercised against real bytes.
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

    fn hevc_sps_info() -> lvqr_codec::hevc::HevcSps {
        lvqr_codec::hevc::HevcSps {
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 1,
            general_profile_compatibility_flags: 0x60000000,
            general_level_idc: 60,
            chroma_format_idc: 1,
            pic_width_in_luma_samples: 320,
            pic_height_in_luma_samples: 240,
        }
    }

    /// Drive one HEVC IDR fragment through play_drain_hevc; verify
    /// the VPS + SPS + PPS preamble is emitted (three packets) and
    /// the IDR follows on the same channel with the right PT and
    /// marker bit.
    #[tokio::test]
    async fn play_drain_hevc_re_injects_vps_sps_pps_and_packetizes_idr() {
        use bytes::BytesMut;
        use lvqr_cmaf::{HevcInitParams, write_hevc_init_segment};

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("live/hevc", "0.mp4", FragmentMeta::new("hev1", 90_000));

        let mut init = BytesMut::new();
        write_hevc_init_segment(
            &mut init,
            &HevcInitParams {
                vps: HEVC_VPS.to_vec(),
                sps: HEVC_SPS.to_vec(),
                pps: HEVC_PPS.to_vec(),
                sps_info: hevc_sps_info(),
                timescale: 90_000,
            },
        )
        .expect("write hevc init");
        bc.set_init_segment(init.freeze());

        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(64);
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(play_drain_hevc(
            "live/hevc".to_string(),
            0,
            registry.clone(),
            writer_tx,
            cancel.clone(),
        ));

        // Read three param-set packets (VPS, SPS, PPS) and confirm
        // their contents match the input NALs.
        let vps_frame = tokio::time::timeout(std::time::Duration::from_secs(1), writer_rx.recv())
            .await
            .expect("vps timeout")
            .expect("vps channel open");
        let sps_frame = writer_rx.recv().await.expect("sps frame");
        let pps_frame = writer_rx.recv().await.expect("pps frame");

        for (expected, frame) in [(HEVC_VPS, vps_frame), (HEVC_SPS, sps_frame), (HEVC_PPS, pps_frame)] {
            assert_eq!(frame[0], 0x24);
            assert_eq!(frame[1], 0);
            let rtp = &frame[4..];
            let hdr = crate::rtp::parse_rtp_header(rtp).expect("rtp header");
            assert_eq!(hdr.payload_type, HEVC_PAYLOAD_TYPE);
            assert_eq!(hdr.ssrc, DEFAULT_SSRC);
            assert_eq!(hdr.timestamp, 0);
            assert!(!hdr.marker, "param-set packets clear marker bit");
            let payload = &rtp[hdr.header_len..];
            // Each parameter set fits within MTU so the packet is a
            // single-NAL packet whose payload is the NAL verbatim.
            assert_eq!(payload, expected);
        }

        // Emit an IDR HEVC NAL and verify depacketization round-trips.
        let mut idr_nal = vec![0x26, 0x01]; // IDR_W_RADL (type 19)
        idr_nal.extend_from_slice(&[0xA1, 0xB2, 0xC3, 0xD4]);
        let sample = RawSample {
            track_id: 1,
            dts: 3000,
            cts_offset: 0,
            duration: 3000,
            payload: Bytes::from(avcc_nal(&idr_nal)),
            keyframe: true,
        };
        let fragment_payload = build_moof_mdat(1, 1, 3000, std::slice::from_ref(&sample));
        bc.emit(Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            3000,
            3000,
            3000,
            FragmentFlags::KEYFRAME,
            fragment_payload,
        ));

        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), writer_rx.recv())
            .await
            .expect("idr timeout")
            .expect("idr channel open");
        let rtp = &frame[4..];
        let hdr = crate::rtp::parse_rtp_header(rtp).expect("rtp header");
        assert_eq!(hdr.timestamp, 3000);
        assert!(hdr.marker, "IDR is a single NAL -> marker set");
        let payload = &rtp[hdr.header_len..];
        assert_eq!(payload, &idr_nal[..]);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn play_drain_hevc_exits_on_avc_init() {
        // The HEVC drain on an AVC-only broadcaster must exit cleanly
        // rather than emit malformed RTP or panic. Mirrors the same
        // invariant play_drain_h264 pins for HEVC inits.
        use bytes::BytesMut;

        let registry = FragmentBroadcasterRegistry::new();
        let bc = registry.get_or_create("live/avc", "0.mp4", FragmentMeta::new("avc1", 90_000));
        let mut init = BytesMut::new();
        write_avc_init_segment(
            &mut init,
            &VideoInitParams {
                sps: SPS.to_vec(),
                pps: PPS.to_vec(),
                width: 1280,
                height: 720,
                timescale: 90_000,
            },
        )
        .expect("write init");
        bc.set_init_segment(init.freeze());

        let (writer_tx, _writer_rx) = mpsc::channel::<Vec<u8>>(4);
        let cancel = CancellationToken::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            play_drain_hevc("live/avc".into(), 0, registry, writer_tx, cancel),
        )
        .await
        .expect("hevc drain exits promptly on AVC init");
    }
}
