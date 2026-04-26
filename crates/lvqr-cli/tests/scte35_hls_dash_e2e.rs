//! End-to-end test for SCTE-35 ad-marker passthrough through the
//! LL-HLS + DASH egress surfaces (session 152).
//!
//! Spins up a `TestServer` with default config (HLS on, DASH off
//! by default), publishes a synthetic video fragment + a real
//! CRC-valid SCTE-35 splice_insert section directly onto the
//! shared `FragmentBroadcasterRegistry`'s reserved `"scte35"`
//! track (skipping the SRT / RTMP ingest paths), and drives raw-
//! TCP HTTP/1.1 GETs against the LL-HLS variant playlist to
//! assert it carries the `#EXT-X-DATERANGE` line per HLS spec
//! section 4.4.5.1.
//!
//! Mirrors the structure of `captions_hls_e2e.rs` (Tier 4 item
//! 4.5 session C). The HLS / DASH wire shapes are unit-tested in
//! `lvqr-hls` + `lvqr-dash`; the parser + bridge are unit-tested
//! in `lvqr-codec` + `lvqr-cli/src/scte35_bridge.rs`. This test
//! pins the contract that the full pipeline -- registry ->
//! BroadcasterScte35Bridge drain -> MultiHlsServer push ->
//! manifest render -> HTTP response -- delivers SCTE-35 markers
//! to a real HLS client end-to-end.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta, SCTE35_TRACK};
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};

const TIMEOUT: Duration = Duration::from_secs(10);

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

fn synthetic_video_fragment() -> Fragment {
    Fragment::new(
        "0.mp4",
        0,
        0,
        0,
        0,
        0,
        3000,
        FragmentFlags::KEYFRAME,
        Bytes::from_static(&[0u8; 16]),
    )
}

/// Build a real CRC-valid splice_insert splice_info_section with
/// a known event_id, splice_time PTS, break_duration, and
/// out_of_network=1 (so the egress emits SCTE35-OUT).
fn build_splice_insert_section(event_id: u32, pts_90k: u64, duration_90k: u64) -> Vec<u8> {
    use lvqr_codec::scte35::{CMD_SPLICE_INSERT, TABLE_ID};

    // 14-byte prefix per SCTE 35-2024 section 8.1: table_id,
    // section_length(2), protocol_version, encrypted/encryption_alg/pts_adj high
    // bit, pts_adj_lower(4), cw_index, tier_high(1), tier_low|scl_high,
    // scl_low, splice_command_type.
    let mut prefix = vec![
        TABLE_ID,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0x00,
        0xFF,
        0xF0,
        0x00,
        CMD_SPLICE_INSERT,
    ];

    // splice_insert body fields per SCTE 35-2024 section 9.7.3:
    // event_id(4) + flags(1: cancel=0, reserved=7) + flags(1: out=1, program=1,
    // duration=1, immediate=0, reserved=4) + splice_time(5) + break_duration(5) +
    // unique_program_id(2) + avail_num(1) + avails_expected(1).
    let body = vec![
        (event_id >> 24) as u8,
        (event_id >> 16) as u8,
        (event_id >> 8) as u8,
        event_id as u8,
        0x7F, // cancel=0, reserved=0x7F
        0xEF, // out=1, program=1, duration=1, immediate=0, reserved=1111
        0xFE | ((pts_90k >> 32) as u8 & 0x01),
        (pts_90k >> 24) as u8,
        (pts_90k >> 16) as u8,
        (pts_90k >> 8) as u8,
        pts_90k as u8,
        0xFE | ((duration_90k >> 32) as u8 & 0x01),
        (duration_90k >> 24) as u8,
        (duration_90k >> 16) as u8,
        (duration_90k >> 8) as u8,
        duration_90k as u8,
        0x00,
        0x01, // unique_program_id
        0x00, // avail_num
        0x00, // avails_expected
    ];

    let total_minus_crc = prefix.len() + body.len() + 2;
    let total = total_minus_crc + 4;
    let section_length = total - 3;

    prefix[1] = 0x30 | ((section_length >> 8) as u8 & 0x0F);
    prefix[2] = section_length as u8;
    prefix[11] = (prefix[11] & 0xF0) | ((body.len() >> 8) as u8 & 0x0F);
    prefix[12] = body.len() as u8;

    let mut section = Vec::with_capacity(total);
    section.extend_from_slice(&prefix);
    section.extend_from_slice(&body);
    section.push(0x00); // descriptor_loop_length high
    section.push(0x00); // descriptor_loop_length low

    // CRC-32/MPEG-2 over [0..total-4].
    let crc = {
        let mut c: u32 = 0xFFFF_FFFF;
        for &b in &section {
            c ^= (b as u32) << 24;
            for _ in 0..8 {
                c = if c & 0x8000_0000 != 0 {
                    (c << 1) ^ 0x04C1_1DB7
                } else {
                    c << 1
                };
            }
        }
        c
    };
    section.push((crc >> 24) as u8);
    section.push((crc >> 16) as u8);
    section.push((crc >> 8) as u8);
    section.push(crc as u8);
    section
}

fn synthetic_scte35_fragment(event_id: u32, pts_90k: u64, duration_90k: u64) -> Fragment {
    let section = build_splice_insert_section(event_id, pts_90k, duration_90k);
    Fragment::new(
        SCTE35_TRACK,
        event_id as u64,
        0,
        0,
        pts_90k,
        pts_90k,
        duration_90k,
        FragmentFlags::KEYFRAME,
        Bytes::from(section),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scte35_section_renders_as_hls_daterange_in_variant_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let hls_addr = server.hls_addr();
    let registry = server.fragment_registry().clone();

    // Publish a synthetic video fragment so the HLS bridge creates
    // the per-broadcast video rendition; the variant playlist
    // returns 404 until the rendition exists.
    let video_meta = FragmentMeta::new("avc1.640028", 90_000);
    let video_bc = registry.get_or_create("live/cam1", "0.mp4", video_meta);
    video_bc.emit(synthetic_video_fragment());

    // Publish a SCTE-35 splice_insert directly onto the reserved
    // scte35 track. The BroadcasterScte35Bridge installed by
    // lvqr-cli's start() picks it up, parses + verifies CRC, and
    // pushes a DateRange into the per-broadcast PlaylistBuilder.
    let scte35_meta = FragmentMeta::new("scte35", 90_000);
    let scte35_bc = registry.get_or_create("live/cam1", SCTE35_TRACK, scte35_meta);
    scte35_bc.emit(synthetic_scte35_fragment(0xCAFE_BABE, 8_100_000, 2_700_000));

    // Hold producer-side clones alive long enough for the
    // bridge's drain task to spawn + drain. Mirrors the captions
    // test's wait shape.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Variant playlist must carry the DATERANGE entry with the
    // expected attributes per HLS spec section 4.4.5.1.
    let resp = http_get(hls_addr, "/hls/live/cam1/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "playlist status");
    let body = std::str::from_utf8(&resp.body).expect("playlist body utf-8");
    eprintln!("--- /hls/live/cam1/playlist.m3u8 ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U");
    assert!(
        body.contains("#EXT-X-DATERANGE:"),
        "variant playlist must carry an EXT-X-DATERANGE entry: {body}"
    );
    assert!(
        body.contains("ID=\"splice-3405691582\""),
        "DATERANGE ID must be derived from event_id 0xCAFEBABE = 3405691582: {body}"
    );
    assert!(
        body.contains("CLASS=\"urn:scte:scte35:2014:bin\""),
        "DATERANGE must carry the SCTE-35 CLASS attribute: {body}"
    );
    assert!(
        body.contains("SCTE35-OUT="),
        "splice_insert with out_of_network=1 renders SCTE35-OUT: {body}"
    );
    assert!(
        body.contains("DURATION=30.000"),
        "break_duration 2_700_000 / 90_000 = 30.000s: {body}"
    );

    drop(video_bc);
    drop(scte35_bc);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scte35_section_renders_as_dash_event_in_period_event_stream() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default().with_dash())
        .await
        .expect("start TestServer");
    let dash_addr = server.dash_addr();
    let registry = server.fragment_registry().clone();

    // Publish synthetic video so the DASH server has enough state
    // to render a non-None MPD; without this the manifest 503s.
    let video_meta = FragmentMeta::new("avc1.640028", 90_000);
    let video_bc = registry.get_or_create("live/cam1", "0.mp4", video_meta);
    video_bc.emit(synthetic_video_fragment());

    // Publish the SCTE-35 splice_insert. The bridge drains it +
    // pushes a DashEvent into the per-broadcast EventStream.
    let scte35_meta = FragmentMeta::new("scte35", 90_000);
    let scte35_bc = registry.get_or_create("live/cam1", SCTE35_TRACK, scte35_meta);
    scte35_bc.emit(synthetic_scte35_fragment(0xCAFE_BABE, 8_100_000, 2_700_000));

    tokio::time::sleep(Duration::from_millis(300)).await;

    // DASH MPD must contain the Period-level EventStream + Event.
    let resp = http_get(dash_addr, "/dash/live/cam1/manifest.mpd").await;
    assert_eq!(resp.status, 200, "MPD status");
    let body = std::str::from_utf8(&resp.body).expect("MPD body utf-8");
    eprintln!("--- /dash/live/cam1/manifest.mpd ---\n{body}\n--- end ---");
    assert!(
        body.contains("<EventStream "),
        "MPD must carry an EventStream element: {body}"
    );
    assert!(
        body.contains("schemeIdUri=\"urn:scte:scte35:2014:xml+bin\""),
        "EventStream must declare the SCTE-35 xml+bin scheme: {body}"
    );
    assert!(
        body.contains("timescale=\"90000\""),
        "EventStream timescale must be 90 kHz: {body}"
    );
    assert!(
        body.contains("id=\"3405691582\""),
        "Event id must come from the splice_event_id 0xCAFEBABE: {body}"
    );
    assert!(
        body.contains("presentationTime=\"8100000\""),
        "Event presentationTime must come from the splice_insert PTS: {body}"
    );
    assert!(
        body.contains("duration=\"2700000\""),
        "Event duration must come from the splice_insert break_duration: {body}"
    );
    assert!(
        body.contains("<Signal xmlns=\"http://www.scte.org/schemas/35/2016\">"),
        "Event body must wrap the binary in a SCTE-35 Signal element: {body}"
    );
    assert!(body.contains("<Binary>"), "Event body must include Binary: {body}");

    // EventStream must render BEFORE the AdaptationSet siblings
    // per ISO/IEC 23009-1 section 5.3.2.1 ordering. Shaka and
    // dash.js both rely on this.
    let es_pos = body.find("<EventStream ").expect("EventStream present");
    let as_pos = body.find("<AdaptationSet ").expect("AdaptationSet present");
    assert!(
        es_pos < as_pos,
        "EventStream must precede AdaptationSet inside the Period: {body}"
    );

    drop(video_bc);
    drop(scte35_bc);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn variant_playlist_omits_daterange_when_no_scte35_track() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let hls_addr = server.hls_addr();
    let registry = server.fragment_registry().clone();

    // Publish only video; never touch the scte35 track.
    let video_meta = FragmentMeta::new("avc1.640028", 90_000);
    let video_bc = registry.get_or_create("live/cam1", "0.mp4", video_meta);
    video_bc.emit(synthetic_video_fragment());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = http_get(hls_addr, "/hls/live/cam1/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "playlist status");
    let body = std::str::from_utf8(&resp.body).expect("playlist body utf-8");
    assert!(body.starts_with("#EXTM3U"));
    assert!(
        !body.contains("#EXT-X-DATERANGE"),
        "no SCTE-35 events were published; playlist must not emit DATERANGE: {body}"
    );

    drop(video_bc);
    server.shutdown().await.expect("shutdown");
}
