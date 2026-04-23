//! SRT ingest -> MPEG-DASH HTTP egress end-to-end integration test.
//!
//! Session 114 (phase B row 3) closes the third cross-ingress test
//! gap from the 2026-04-13 audit. Publishes a minimal MPEG-TS H.264
//! stream over SRT and verifies the MPEG-DASH MPD + init segment +
//! first numbered media segment are all reachable off the
//! `/dash/{broadcast}/...` surface.
//!
//! Shape mirrors `srt_hls_e2e.rs` (publisher side) and
//! `rtmp_dash_e2e.rs` (subscriber side): real SRT caller publishes,
//! real `lvqr_cli::start`-driven server demuxes TS, remuxes to CMAF,
//! the shared `FragmentBroadcasterRegistry` drains through
//! `BroadcasterDashBridge` into `MultiDashServer`, and a raw HTTP/1.1
//! client reads the manifest + segments off the bound DASH listener.

use bytes::Bytes;
use futures_util::SinkExt;
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);
const SYNC_BYTE: u8 = 0x47;

// =====================================================================
// MPEG-TS + H.264 PES helpers (mirror crates/lvqr-cli/tests/srt_hls_e2e.rs)
// =====================================================================

fn make_ts_packet(pid: u16, pusi: bool, payload: &[u8]) -> Vec<u8> {
    let mut pkt = vec![0xFFu8; 188];
    pkt[0] = SYNC_BYTE;
    pkt[1] = if pusi { 0x40 } else { 0x00 } | ((pid >> 8) as u8 & 0x1F);
    pkt[2] = pid as u8;
    pkt[3] = 0x10;
    let copy_len = payload.len().min(184);
    pkt[4..4 + copy_len].copy_from_slice(&payload[..copy_len]);
    pkt
}

fn minimal_pat(pmt_pid: u16) -> Vec<u8> {
    let mut data = vec![0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
    data.push(0xE0 | ((pmt_pid >> 8) as u8 & 0x1F));
    data.push(pmt_pid as u8);
    data.extend_from_slice(&[0x00; 4]);
    data
}

fn minimal_pmt(video_pid: u16) -> Vec<u8> {
    vec![
        0x00,
        0x02,
        0xB0,
        0x12,
        0x00,
        0x01,
        0xC1,
        0x00,
        0x00,
        0xE1,
        0x00,
        0xF0,
        0x00,
        0x1B,
        0xE0 | ((video_pid >> 8) as u8 & 0x1F),
        video_pid as u8,
        0xF0,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
    ]
}

fn h264_pes(pts_90k: u64, keyframe: bool) -> Vec<u8> {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let nal_type: u8 = if keyframe { 0x65 } else { 0x41 };
    let slice = [nal_type, 0x88, 0x84, 0x00, 0xDE, 0xAD];

    let mut es = Vec::new();
    if keyframe {
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&sps);
        es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        es.extend_from_slice(&pps);
    }
    es.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    es.extend_from_slice(&slice);

    let pes_payload_len = (3 + 5 + es.len()) as u16;
    let mut data = vec![
        0x00,
        0x00,
        0x01,
        0xE0,
        (pes_payload_len >> 8) as u8,
        pes_payload_len as u8,
        0x80,
        0x80,
        0x05,
    ];
    let pts = pts_90k & 0x1_FFFF_FFFF;
    data.push(0x21 | ((pts >> 29) as u8 & 0x0E));
    data.push((pts >> 22) as u8);
    data.push(0x01 | ((pts >> 14) as u8 & 0xFE));
    data.push((pts >> 7) as u8);
    data.push(0x01 | ((pts << 1) as u8 & 0xFE));
    data.extend_from_slice(&es);
    data
}

// =====================================================================
// HTTP helper now lives in `lvqr_test_utils::http`; the 10s timeout
// wrapper below matches the one in `rtmp_dash_e2e.rs`.
// =====================================================================

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await
}

/// Real end-to-end: one SRT caller publishes H.264 over MPEG-TS ->
/// `SrtIngestServer` -> `TsDemuxer` -> remux to CMAF fragments ->
/// shared `FragmentBroadcasterRegistry` -> `BroadcasterDashBridge`
/// drain task -> `MultiDashServer` -> axum HTTP. Verifies the
/// `/dash/srt/default/manifest.mpd` endpoint renders a syntactically
/// plausible live-profile MPD, `/init-video.m4s` serves the init
/// segment with the expected `ftyp` prefix, and at least one numbered
/// `seg-video-1.m4s` URI resolves to a non-empty `moof`-prefixed body.
#[tokio::test]
async fn srt_publish_reaches_dash_router() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_srt().with_dash())
        .await
        .expect("start TestServer with SRT + DASH");
    let srt_addr = server.srt_addr();
    let dash_addr = server.dash_addr();

    // Connect as an SRT caller to the server's listener. The call
    // API resolves the handshake synchronously so publish can begin
    // as soon as the future resolves.
    let mut srt: srt_tokio::SrtSocket = srt_tokio::SrtSocket::builder()
        .call(srt_addr, None)
        .await
        .expect("SRT connect");

    // Minimal MPEG-TS stream: PAT + PMT + 2 H.264 keyframes spaced
    // 2 s apart so the DASH segmenter closes a full segment on the
    // second keyframe. The 180_000 tick spacing at 90 kHz = 2 s.
    let video_pid = 0x100u16;
    let pmt_pid = 0x1000u16;
    let mut ts_data = Vec::new();
    ts_data.extend_from_slice(&make_ts_packet(0, true, &minimal_pat(pmt_pid)));
    ts_data.extend_from_slice(&make_ts_packet(pmt_pid, true, &minimal_pmt(video_pid)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &h264_pes(0, true)));
    ts_data.extend_from_slice(&make_ts_packet(video_pid, true, &h264_pes(180_000, true)));

    // Hold the SRT socket open for the rest of the test so the DASH
    // broadcast stays in the live (`type="dynamic"`) state. Closing
    // immediately after the send would cascade through BroadcastStopped
    // into MultiDashServer::finalize_broadcast which flips the manifest
    // to `type="static"` with the on-demand profile.
    let now = Instant::now();
    srt.send((now, Bytes::from(ts_data))).await.unwrap();

    // SRT ingest + TS demux + CMAF remux all happen on tokio tasks;
    // give the dispatch side a tick to land samples in the DASH
    // server state before reading.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let manifest = http_get(dash_addr, "/dash/srt/default/manifest.mpd").await;
    assert_eq!(
        manifest.status,
        200,
        "manifest GET status for SRT broadcast (body: {:?})",
        std::str::from_utf8(&manifest.body).unwrap_or("<binary>")
    );
    let body = std::str::from_utf8(&manifest.body).expect("manifest body utf-8");
    eprintln!("--- srt dash manifest.mpd ---\n{body}\n--- end ---");
    assert!(body.contains("<MPD"), "manifest must contain an <MPD> element:\n{body}");
    assert!(
        body.contains("type=\"dynamic\""),
        "live-profile MPD must start as dynamic:\n{body}",
    );
    assert!(
        body.contains("<AdaptationSet"),
        "manifest must advertise at least one AdaptationSet:\n{body}",
    );
    assert!(
        body.contains("seg-video-$Number$.m4s"),
        "manifest must reference the numbered segment template:\n{body}",
    );

    let init = http_get(dash_addr, "/dash/srt/default/init-video.m4s").await;
    assert_eq!(init.status, 200, "init-video GET status");
    assert!(
        init.body.len() >= 8,
        "init-video body too short: {} bytes",
        init.body.len()
    );
    assert_eq!(&init.body[4..8], b"ftyp", "init-video segment did not start with ftyp");

    let seg = http_get(dash_addr, "/dash/srt/default/seg-video-1.m4s").await;
    assert_eq!(
        seg.status,
        200,
        "seg-video-1 GET status for SRT broadcast (body bytes: {})",
        seg.body.len()
    );
    assert!(
        seg.body.len() >= 8,
        "seg-video-1 body too short: {} bytes",
        seg.body.len()
    );
    assert_eq!(
        &seg.body[4..8],
        b"moof",
        "expected seg-video-1 to start with a moof box",
    );

    // Negative: unknown broadcast returns 404 off the DASH router.
    let unknown = http_get(dash_addr, "/dash/srt/ghost/manifest.mpd").await;
    assert_eq!(unknown.status, 404, "unknown SRT broadcast must 404");

    // Close the SRT socket only now that the DASH assertions have
    // captured the live-state manifest. The natural test teardown
    // below will also drop it, but being explicit keeps the ingest
    // teardown path symmetric with the assertions.
    srt.close().await.unwrap();

    server.shutdown().await.expect("shutdown");
}
