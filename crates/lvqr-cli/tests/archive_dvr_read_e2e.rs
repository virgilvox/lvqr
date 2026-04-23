//! Archive DVR read-side end-to-end tests.
//!
//! Companion to `rtmp_archive_e2e.rs`, which covers the
//! happy-path read shape (status + JSON/body + auth gates).
//! This file targets three scenarios that the write-side test
//! does not exercise:
//!
//! 1. **Multi-keyframe scrub window arithmetic.** A real DVR
//!    scrub selects a subset of segments out of a multi-segment
//!    stream. `rtmp_archive_e2e.rs` only publishes two keyframes
//!    (so at most two closed segments exist), which cannot
//!    distinguish a half-window scan from a full-window scan.
//!    This test publishes five keyframes spaced 2 s apart and
//!    asserts the `[from, to)` semantics documented on
//!    [`lvqr_archive::SegmentIndex::find_range`]: every segment
//!    whose `[start_dts, end_dts)` overlaps the query window is
//!    returned, ordered by `start_dts` ascending.
//!
//! 2. **Live-DVR scrub.** An operator scrubbing a DVR of a
//!    still-active broadcast is the actual production scenario;
//!    the write-side test runs every assertion after the
//!    publisher finishes, so the redb exclusive-file lock is
//!    quiescent when the HTTP scan runs. This test keeps the
//!    RTMP publisher open across several HTTP scans and asserts
//!    the reader does not block (the handlers run the sync redb
//!    scan on `spawn_blocking` precisely for this reason).
//!
//! 3. **Content-Type headers.** Every handler hard-codes
//!    `application/json` or `application/octet-stream`, but the
//!    write-side test's raw TCP HTTP client reads status + body
//!    only. A drop-in handler swap that returned plain text
//!    would pass `rtmp_archive_e2e.rs`. The extended HTTP client
//!    in this file parses every header so an assertion can guard
//!    the wire contract.
//!
//! No mocks: real RTMP handshake, real bridge observer path,
//! real on-disk writes, real tokio::net TCP for every HTTP call.

use bytes::Bytes;
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

// =====================================================================
// FLV tag helpers (mirror rtmp_archive_e2e.rs).
// =====================================================================

fn flv_video_seq_header() -> Bytes {
    let sps = [0x67, 0x64, 0x00, 0x1F, 0xAC, 0xD9];
    let pps = [0x68, 0xEE, 0x3C, 0x80];
    let mut tag = vec![0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x00, 0x1F, 0xFF, 0xE1];
    tag.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&sps);
    tag.push(0x01);
    tag.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    tag.extend_from_slice(&pps);
    Bytes::from(tag)
}

fn flv_video_nalu(keyframe: bool, cts: i32, nalu_data: &[u8]) -> Bytes {
    let frame_type = if keyframe { 0x17 } else { 0x27 };
    let mut tag = vec![frame_type, 0x01, (cts >> 16) as u8, (cts >> 8) as u8, cts as u8];
    tag.extend_from_slice(nalu_data);
    Bytes::from(tag)
}

// =====================================================================
// HTTP/1.1 `Connection: close` GET client, with header capture.
// =====================================================================

use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};

async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    http_get_with_range(addr, path, None).await
}

async fn http_get_with_range(addr: SocketAddr, path: &str, range: Option<&str>) -> HttpResponse {
    let mut opts = HttpGetOptions {
        timeout: TIMEOUT,
        ..HttpGetOptions::default()
    };
    opts.range = range;
    http_get_with(addr, path, opts).await
}

// =====================================================================
// RTMP client helpers (mirror rtmp_archive_e2e.rs).
// =====================================================================

async fn rtmp_client_handshake(stream: &mut TcpStream) -> Vec<u8> {
    let mut handshake = Handshake::new(PeerType::Client);
    let p0_and_p1 = handshake.generate_outbound_p0_and_p1().unwrap();
    stream.write_all(&p0_and_p1).await.unwrap();
    let mut buf = vec![0u8; 8192];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "server closed during handshake");
        match handshake.process_bytes(&buf[..n]).unwrap() {
            HandshakeProcessResult::InProgress { response_bytes } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
            }
            HandshakeProcessResult::Completed {
                response_bytes,
                remaining_bytes,
            } => {
                if !response_bytes.is_empty() {
                    stream.write_all(&response_bytes).await.unwrap();
                }
                return remaining_bytes;
            }
        }
    }
}

async fn send_results(stream: &mut TcpStream, results: &[ClientSessionResult]) {
    for result in results {
        if let ClientSessionResult::OutboundResponse(packet) = result {
            stream.write_all(&packet.bytes).await.unwrap();
        }
    }
}

async fn send_result(stream: &mut TcpStream, result: &ClientSessionResult) {
    if let ClientSessionResult::OutboundResponse(packet) = result {
        stream.write_all(&packet.bytes).await.unwrap();
    }
}

async fn read_until<F>(stream: &mut TcpStream, session: &mut ClientSession, predicate: F)
where
    F: Fn(&ClientSessionEvent) -> bool,
{
    let mut buf = vec![0u8; 65536];
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - Instant::now();
        let n = match tokio::time::timeout(remaining, stream.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => n,
            Ok(Ok(_)) => panic!("server closed connection unexpectedly"),
            Ok(Err(e)) => panic!("read error: {e}"),
            Err(_) => panic!("timed out waiting for expected RTMP event"),
        };
        let results = session.handle_input(&buf[..n]).unwrap();
        for result in results {
            match result {
                ClientSessionResult::OutboundResponse(packet) => {
                    stream.write_all(&packet.bytes).await.unwrap();
                }
                ClientSessionResult::RaisedEvent(ref event) => {
                    if predicate(event) {
                        return;
                    }
                }
                _ => {}
            }
        }
    }
}

async fn connect_and_publish(addr: SocketAddr, app: &str, stream_key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial_results) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial_results).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect_result = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish_result = session
        .request_publishing(stream_key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish_result).await;
    read_until(&mut stream, &mut session, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

/// Publish N keyframes with RTMP timestamps in milliseconds
/// taken from `timestamps_ms`. The CMAF coalescer closes a
/// segment when a new keyframe arrives, so N keyframes yield
/// N-1 closed segments (the last one finalizes on broadcast
/// disconnect or broadcast-end drain termination).
async fn publish_keyframe_train(
    addr: SocketAddr,
    app: &str,
    key: &str,
    timestamps_ms: &[u32],
) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    for &ts in timestamps_ms {
        let kf = flv_video_nalu(true, 0, &nalu);
        let r = session.publish_video_data(kf, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut rtmp_stream, &r).await;
    }

    (rtmp_stream, session)
}

// =====================================================================
// Test 1: multi-keyframe scrub window arithmetic.
// =====================================================================

/// Publish five keyframes at RTMP timestamps 0, 2000, 4000,
/// 6000, 8000 ms. The coalescer closes a segment on each new
/// keyframe, so four closed segments land in the index before
/// the RTMP session is dropped. Splitting the full window at a
/// midpoint selects a subset of those segments; the find_range
/// contract documents "segments whose [start_dts, end_dts)
/// overlaps [query_start, query_end) are returned", so the
/// assertions verify every half-window row obeys the per-row
/// overlap property and the union of the two halves is a
/// superset (or equal, when the midpoint lands between two
/// segments) of the full-window response.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_scrub_window_arithmetic() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(TestServerConfig::default().with_archive_dir(&archive_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, rtmp_session) =
        publish_keyframe_train(rtmp_addr, "live", "dvr", &[0, 2000, 4000, 6000, 8000]).await;

    // Wait for the bridge to drain + redb to commit every closed
    // segment. 500 ms matches the rtmp_archive_e2e pattern.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Full-window scan establishes the ground-truth row set.
    let resp = http_get(admin_addr, "/playback/live/dvr").await;
    assert_eq!(resp.status, 200, "full-window GET status");
    let full_rows: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("full-window body is JSON array");
    assert!(
        full_rows.len() >= 3,
        "expected >= 3 closed segments from 5-keyframe train, got {} rows: {full_rows:?}",
        full_rows.len()
    );

    // Rows must be ordered by start_dts ascending.
    for window in full_rows.windows(2) {
        let a = window[0]["start_dts"].as_u64().unwrap();
        let b = window[1]["start_dts"].as_u64().unwrap();
        assert!(a <= b, "full-window rows unsorted by start_dts: {a} then {b}");
    }

    // Pick a midpoint halfway between the first and last rows'
    // start_dts. Deliberately numeric-derived so the test does
    // not depend on the RTMP -> CMAF timescale factor being
    // exactly 90 (the bridge is allowed to evolve; the contract
    // under test is the redb/HTTP scrub arithmetic, not the
    // ingest-side timestamp math).
    let min_start = full_rows.first().unwrap()["start_dts"].as_u64().unwrap();
    let max_start = full_rows.last().unwrap()["start_dts"].as_u64().unwrap();
    assert!(
        max_start > min_start,
        "need > 1 distinct start_dts to scrub; got min={min_start} max={max_start}",
    );
    let midpoint = min_start + (max_start - min_start) / 2;

    // First half: [0, midpoint). Every returned row must OVERLAP
    // the window, i.e. its start_dts < midpoint (otherwise it
    // would be excluded per "segments that start at or after
    // query_end are excluded").
    let resp = http_get(admin_addr, &format!("/playback/live/dvr?to={midpoint}")).await;
    assert_eq!(resp.status, 200, "first-half GET status");
    let first_half: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("first-half body is JSON array");
    assert!(
        !first_half.is_empty(),
        "first half [0, {midpoint}) must contain the first segment (start_dts={min_start})",
    );
    for row in &first_half {
        let start = row["start_dts"].as_u64().unwrap();
        assert!(
            start < midpoint,
            "first-half row has start_dts={start} >= midpoint={midpoint}, violates find_range semantics",
        );
    }

    // Second half: [midpoint, u64::MAX). Every returned row
    // must OVERLAP the window, i.e. its end_dts > midpoint.
    let resp = http_get(admin_addr, &format!("/playback/live/dvr?from={midpoint}")).await;
    assert_eq!(resp.status, 200, "second-half GET status");
    let second_half: Vec<serde_json::Value> =
        serde_json::from_slice(&resp.body).expect("second-half body is JSON array");
    assert!(
        !second_half.is_empty(),
        "second half [{midpoint}, inf) must contain the last segment (start_dts={max_start})",
    );
    for row in &second_half {
        let end = row["end_dts"].as_u64().unwrap();
        assert!(
            end > midpoint,
            "second-half row has end_dts={end} <= midpoint={midpoint}, violates find_range semantics",
        );
    }

    // Union invariant: every full-window segment_seq appears in
    // at least one half-window. A segment that straddles the
    // midpoint appears in BOTH halves, so the union may contain
    // duplicates; the set-union equality is the load-bearing
    // property.
    use std::collections::HashSet;
    let full_seqs: HashSet<u64> = full_rows.iter().map(|r| r["segment_seq"].as_u64().unwrap()).collect();
    let first_seqs: HashSet<u64> = first_half.iter().map(|r| r["segment_seq"].as_u64().unwrap()).collect();
    let second_seqs: HashSet<u64> = second_half.iter().map(|r| r["segment_seq"].as_u64().unwrap()).collect();
    let union_seqs: HashSet<u64> = first_seqs.union(&second_seqs).copied().collect();
    assert_eq!(
        union_seqs, full_seqs,
        "half-window union must cover the full-window segment set exactly",
    );

    // /playback/latest agrees with the last row of the
    // full-window scan.
    let resp = http_get(admin_addr, "/playback/latest/live/dvr").await;
    assert_eq!(resp.status, 200, "latest GET status");
    let latest: serde_json::Value = serde_json::from_slice(&resp.body).expect("latest body is JSON");
    let last_row = full_rows.last().unwrap();
    assert_eq!(latest["start_dts"], last_row["start_dts"]);
    assert_eq!(latest["segment_seq"], last_row["segment_seq"]);

    drop(rtmp_stream);
    drop(rtmp_session);
    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Test 2: live-DVR scrub while the publisher is still active.
// =====================================================================

/// Hold the RTMP publisher open across multiple `/playback/*`
/// scans. The read handlers run the sync redb scan on
/// `spawn_blocking`; this test proves the scan does not
/// deadlock against the writer's ongoing exclusive-lock use.
///
/// Also asserts that a `/playback/latest/*` fetched late in
/// the publish sees a newer `start_dts` than one fetched
/// early, proving the index updates visibly during an active
/// broadcast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_dvr_scrub_while_publisher_is_active() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(TestServerConfig::default().with_archive_dir(&archive_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    // Publish the first half of the keyframe train, hold the
    // RTMP session open, scrub, publish more, scrub again.
    let (mut rtmp_stream, mut session) = connect_and_publish(rtmp_addr, "live", "dvr").await;
    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    // First batch: keyframes at 0 and 2000 ms (one closed seg).
    for ts in [0u32, 2000] {
        let kf = flv_video_nalu(true, 0, &nalu);
        let r = session.publish_video_data(kf, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut rtmp_stream, &r).await;
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Scan #1: publisher still holds the RTMP session. The
    // admin handler must complete the redb scan without
    // hanging on the writer's file lock.
    let t0 = Instant::now();
    let resp = http_get(admin_addr, "/playback/live/dvr").await;
    let scan_one_elapsed = t0.elapsed();
    assert_eq!(resp.status, 200, "live scrub #1 status");
    let early: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("live scrub #1 body is JSON");
    assert!(
        !early.is_empty(),
        "live scrub #1 returned 0 rows; writer may have raced the reader",
    );
    let early_max_start = early.iter().map(|r| r["start_dts"].as_u64().unwrap()).max().unwrap();
    // Five seconds is a generous upper bound; on loopback the
    // scan completes in <50 ms. A violation here means the
    // handler blocked on the writer's lock instead of letting
    // `spawn_blocking` release the runtime.
    assert!(
        scan_one_elapsed < Duration::from_secs(5),
        "live scrub #1 took {scan_one_elapsed:?}; expected sub-second completion",
    );

    // Second batch: keyframes at 4000 and 6000 ms (two more
    // closed segments).
    for ts in [4000u32, 6000] {
        let kf = flv_video_nalu(true, 0, &nalu);
        let r = session.publish_video_data(kf, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut rtmp_stream, &r).await;
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Scan #2: still-live, but the index has grown. `latest`
    // must advance past the scan-#1 maximum.
    let resp = http_get(admin_addr, "/playback/latest/live/dvr").await;
    assert_eq!(resp.status, 200, "live scrub #2 latest status");
    let latest: serde_json::Value = serde_json::from_slice(&resp.body).expect("latest body is JSON");
    let latest_start = latest["start_dts"].as_u64().unwrap();
    assert!(
        latest_start > early_max_start,
        "latest start_dts must advance during live publish: early={early_max_start} latest={latest_start}",
    );

    // File-route fetch from still-live archive: pick the first
    // segment from the early scan + confirm its bytes are
    // readable while the writer is still active.
    let first = &early[0];
    let seq_num = first["segment_seq"].as_u64().unwrap();
    let file_path = format!("/playback/file/live/dvr/0.mp4/{seq_num:08}.m4s");
    let resp = http_get(admin_addr, &file_path).await;
    assert_eq!(resp.status, 200, "live file GET status for {file_path}");
    assert!(resp.body.len() >= 8, "file body too short during live read");
    assert_eq!(
        &resp.body[4..8],
        b"moof",
        "live file did not start with a `moof` box: {file_path}",
    );

    drop(rtmp_stream);
    drop(session);
    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Test 3: Content-Type headers.
// =====================================================================

/// `/playback/*` handlers hard-code `application/json` or
/// `application/octet-stream` but the write-side test never
/// reads headers off the wire. This test's extended HTTP client
/// captures every header; if a future refactor drops the
/// explicit `Content-Type` or changes it to `text/plain` the
/// assertions here catch the regression at PR time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_routes_emit_expected_content_types() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(TestServerConfig::default().with_archive_dir(&archive_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, session) = publish_keyframe_train(rtmp_addr, "live", "dvr", &[0, 2000, 4000]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Range route: JSON.
    let resp = http_get(admin_addr, "/playback/live/dvr").await;
    assert_eq!(resp.status, 200);
    let ct = resp.header("content-type").expect("range route missing Content-Type");
    assert!(
        ct.contains("application/json"),
        "range route Content-Type must carry application/json, got {ct:?}",
    );

    // Latest route: JSON.
    let resp = http_get(admin_addr, "/playback/latest/live/dvr").await;
    assert_eq!(resp.status, 200);
    let ct = resp.header("content-type").expect("latest route missing Content-Type");
    assert!(
        ct.contains("application/json"),
        "latest route Content-Type must carry application/json, got {ct:?}",
    );

    // File route: octet-stream.
    let range_rows: Vec<serde_json::Value> = {
        let r = http_get(admin_addr, "/playback/live/dvr").await;
        serde_json::from_slice(&r.body).expect("range body is JSON")
    };
    let first_seq = range_rows[0]["segment_seq"].as_u64().unwrap();
    let file_path = format!("/playback/file/live/dvr/0.mp4/{first_seq:08}.m4s");
    let resp = http_get(admin_addr, &file_path).await;
    assert_eq!(resp.status, 200, "file GET status");
    let ct = resp.header("content-type").expect("file route missing Content-Type");
    assert!(
        ct.contains("application/octet-stream"),
        "file route Content-Type must carry application/octet-stream, got {ct:?}",
    );

    // Content-Length should equal the body length on the file
    // route (the handler sets it explicitly).
    let cl = resp
        .header("content-length")
        .expect("file route missing Content-Length")
        .parse::<usize>()
        .expect("Content-Length is numeric");
    assert_eq!(
        cl,
        resp.body.len(),
        "file Content-Length {cl} must match actual body length {}",
        resp.body.len(),
    );

    drop(rtmp_stream);
    drop(session);
    server.shutdown().await.expect("shutdown");
}

// =====================================================================
// Test 4: HTTP Range: bytes= on /playback/file/*.
// =====================================================================

/// `/playback/file/*` honors RFC 7233 byte-range requests.
/// HTML5 `<video>` tags issue `Range: bytes=0-` on first fetch
/// and subsequently request `Range: bytes=<seek>-` as the viewer
/// scrubs. Without range support every seek would re-download
/// the full segment from byte zero, which made `<video>`-driven
/// DVR scrubbing impractical on segments larger than a handful
/// of kilobytes.
///
/// Covers four flavors:
/// * `bytes=A-B` -- explicit closed range, 206 Partial Content
/// * `bytes=A-` -- open-ended tail, 206 Partial Content
/// * `bytes=-N` -- last N bytes (suffix), 206 Partial Content
/// * invalid range (`bytes=99999-`) -- 416 Range Not Satisfiable
///
/// Also asserts the response carries `Accept-Ranges: bytes` on
/// the non-ranged path so clients probing for range support
/// see a positive signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_file_supports_byte_range_requests() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(TestServerConfig::default().with_archive_dir(&archive_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();

    let (rtmp_stream, session) = publish_keyframe_train(rtmp_addr, "live", "dvr", &[0, 2000, 4000]).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Pick the first archived segment + establish the ground
    // truth by fetching the full body first.
    let range_rows: Vec<serde_json::Value> = {
        let r = http_get(admin_addr, "/playback/live/dvr").await;
        serde_json::from_slice(&r.body).expect("range body is JSON")
    };
    let first_seq = range_rows[0]["segment_seq"].as_u64().unwrap();
    let file_path = format!("/playback/file/live/dvr/0.mp4/{first_seq:08}.m4s");

    let full = http_get(admin_addr, &file_path).await;
    assert_eq!(full.status, 200, "baseline full GET status");
    assert_eq!(
        full.header("accept-ranges"),
        Some("bytes"),
        "non-ranged file response must advertise Accept-Ranges",
    );
    let total = full.body.len();
    assert!(
        total >= 32,
        "need at least 32 bytes of segment to exercise range arithmetic, got {total}",
    );

    // Closed range: `bytes=0-15`. Expect 16 bytes (inclusive).
    let resp = http_get_with_range(admin_addr, &file_path, Some("bytes=0-15")).await;
    assert_eq!(resp.status, 206, "closed-range status");
    assert_eq!(resp.body.len(), 16, "closed-range body length");
    assert_eq!(
        resp.body,
        full.body[0..16],
        "closed-range bytes must match full body prefix"
    );
    assert_eq!(
        resp.header("content-range"),
        Some(format!("bytes 0-15/{total}").as_str()),
        "closed-range Content-Range",
    );
    assert_eq!(resp.header("content-length"), Some("16"), "closed-range Content-Length");

    // Open-tail range: `bytes=<total/2>-`. Expect the back half.
    let mid = total / 2;
    let resp = http_get_with_range(admin_addr, &file_path, Some(&format!("bytes={mid}-"))).await;
    assert_eq!(resp.status, 206, "open-tail status");
    assert_eq!(
        resp.body.len(),
        total - mid,
        "open-tail body length: total={total} mid={mid}",
    );
    assert_eq!(resp.body, full.body[mid..], "open-tail bytes must match full body tail");
    assert_eq!(
        resp.header("content-range"),
        Some(format!("bytes {mid}-{}/{total}", total - 1).as_str()),
        "open-tail Content-Range",
    );

    // Suffix range: `bytes=-8`. Expect the last 8 bytes.
    let resp = http_get_with_range(admin_addr, &file_path, Some("bytes=-8")).await;
    assert_eq!(resp.status, 206, "suffix-range status");
    assert_eq!(resp.body.len(), 8, "suffix-range body length");
    assert_eq!(
        resp.body,
        full.body[total - 8..],
        "suffix-range bytes must match full body last 8",
    );
    assert_eq!(
        resp.header("content-range"),
        Some(format!("bytes {}-{}/{total}", total - 8, total - 1).as_str()),
        "suffix-range Content-Range",
    );

    // Unsatisfiable range: start beyond end-of-file. 416 per
    // RFC 7233 with `Content-Range: bytes */<total>`.
    let beyond = total + 1000;
    let resp = http_get_with_range(admin_addr, &file_path, Some(&format!("bytes={beyond}-"))).await;
    assert_eq!(resp.status, 416, "unsatisfiable status");
    assert_eq!(
        resp.header("content-range"),
        Some(format!("bytes */{total}").as_str()),
        "unsatisfiable Content-Range",
    );

    // Multi-range falls through to a normal 200 (we do not emit
    // multipart/byteranges).
    let resp = http_get_with_range(admin_addr, &file_path, Some("bytes=0-10,20-30")).await;
    assert_eq!(resp.status, 200, "multi-range should fall through to 200");
    assert_eq!(resp.body.len(), total, "multi-range full body returned");

    drop(rtmp_stream);
    drop(session);
    server.shutdown().await.expect("shutdown");
}
