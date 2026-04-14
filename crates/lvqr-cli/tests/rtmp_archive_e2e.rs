//! RTMP ingest -> `lvqr-archive` DVR index end-to-end integration test.
//!
//! Sister test to `rtmp_hls_e2e.rs`. This one exercises the Tier 2.4
//! archive path: a real `rml_rtmp` client publishes two keyframes
//! into a full `lvqr_cli::start`-driven server whose `archive_dir`
//! is set to a temp directory. After the publish lands, the test
//! opens the redb segment index at `<archive_dir>/archive.redb`
//! directly, asserts `find_range` returns a non-empty, sorted list
//! of `SegmentRef` rows for the video track, and asserts every
//! listed `path` points at a real file on disk whose length matches
//! the recorded `length`.
//!
//! No mocks: real RTMP handshake + session, real bridge observer,
//! real on-disk writes, real redb queries.

use lvqr_archive::{RedbSegmentIndex, SegmentIndex};
use lvqr_test_utils::{TestServer, TestServerConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use bytes::Bytes;

const TIMEOUT: Duration = Duration::from_secs(10);

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

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

/// Minimal raw-TCP HTTP/1.1 `Connection: close` GET client. Avoids
/// pulling `reqwest` / `hyper-util` in as a dev-dep for a handful
/// of test reads. Mirrors the helper in `rtmp_hls_e2e.rs`.
async fn http_get(addr: SocketAddr, path: &str) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("http GET connect timed out")
        .expect("http GET connect failed");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("http GET read timed out")
        .expect("http GET read failed");
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http response missing header terminator");
    let header_text = std::str::from_utf8(&buf[..split]).expect("http headers are not utf-8");
    let status_line = header_text.lines().next().expect("http response missing status line");
    let mut parts = status_line.splitn(3, ' ');
    let _http_version = parts.next();
    let status: u16 = parts
        .next()
        .expect("status line missing code")
        .parse()
        .expect("status code is not numeric");
    HttpResponse {
        status,
        body: buf[split + 4..].to_vec(),
    }
}

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
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let remaining = deadline - tokio::time::Instant::now();
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

async fn publish_two_keyframes(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let (mut rtmp_stream, mut session) = connect_and_publish(addr, app, key).await;

    let seq = flv_video_seq_header();
    let r = session.publish_video_data(seq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut rtmp_stream, &r).await;

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut rtmp_stream, &r).await;

    (rtmp_stream, session)
}

/// Real end-to-end: RTMP publish -> RtmpMoqBridge ->
/// `IndexingFragmentObserver` -> `<archive_dir>/<broadcast>/<track>/<seq>.m4s`
/// and `<archive_dir>/archive.redb`. Verifies the index has at
/// least one row for the video track, the rows are ordered by
/// `start_dts`, every row's `path` points at an existing file whose
/// length matches the recorded `length`, and `latest()` returns the
/// row with the greatest `start_dts`.
#[tokio::test]
async fn rtmp_publish_populates_archive_index() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let archive_tmp = TempDir::new().expect("tempdir");
    let archive_path = archive_tmp.path().to_path_buf();

    let server = TestServer::start(TestServerConfig::default().with_archive_dir(&archive_path))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();

    let admin_addr = server.admin_addr();

    let (_s, _sess) = publish_two_keyframes(rtmp_addr, "live", "dvr").await;

    // The archiving observer spawns one blocking task per fragment;
    // give them time to land on disk and into redb before we query.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // --- HTTP playback endpoint: GET /playback/{*broadcast} ---
    // Must be hit before shutdown because the server holds the
    // exclusive redb lock; afterwards the process can reopen the
    // index directly for deeper inspection.
    let resp = http_get(admin_addr, "/playback/live/dvr").await;
    assert_eq!(resp.status, 200, "GET /playback/live/dvr status");
    let body = std::str::from_utf8(&resp.body).expect("playback body utf-8");
    eprintln!("--- /playback/live/dvr ---\n{body}\n--- end ---");
    let http_rows: Vec<serde_json::Value> = serde_json::from_str(body).expect("playback body is JSON array");
    assert!(!http_rows.is_empty(), "playback endpoint returned empty JSON array");
    for row in &http_rows {
        assert_eq!(row["broadcast"], "live/dvr");
        assert_eq!(row["track"], "0.mp4");
        assert_eq!(row["timescale"], 90_000);
        assert!(row["end_dts"].as_u64().unwrap() > row["start_dts"].as_u64().unwrap());
        assert!(!row["path"].as_str().unwrap().is_empty());
    }

    // A window query strictly after the last recorded segment
    // must return an empty JSON array, not a 404 or 500. Each
    // published keyframe ends well before dts=1_000_000 in the
    // 90 kHz timescale, so this window cannot overlap anything.
    let resp = http_get(admin_addr, "/playback/live/dvr?from=1000000&to=2000000").await;
    assert_eq!(resp.status, 200, "future-window GET status");
    let empty: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("future-window body is JSON array");
    assert!(empty.is_empty(), "expected empty array for strictly-future window");

    // Unknown broadcast must also return 200 + empty array (the
    // index distinguishes "no rows" from "error").
    let resp = http_get(admin_addr, "/playback/live/ghost").await;
    assert_eq!(resp.status, 200, "unknown broadcast GET status");
    let ghost: Vec<serde_json::Value> = serde_json::from_slice(&resp.body).expect("ghost body is JSON array");
    assert!(ghost.is_empty(), "unknown broadcast should yield empty array");

    // --- GET /playback/latest/{*broadcast}: single most-recent row ---
    let resp = http_get(admin_addr, "/playback/latest/live/dvr").await;
    assert_eq!(resp.status, 200, "GET /playback/latest/live/dvr status");
    let latest: serde_json::Value = serde_json::from_slice(&resp.body).expect("latest body is JSON");
    assert_eq!(latest["broadcast"], "live/dvr");
    assert_eq!(latest["track"], "0.mp4");
    // The final row in the find_range body is the authoritative
    // comparison target: latest() returns the row with the largest
    // start_dts, which is also the last entry in a sorted scan.
    let last_row = http_rows.last().unwrap();
    assert_eq!(latest["start_dts"], last_row["start_dts"]);
    assert_eq!(latest["segment_seq"], last_row["segment_seq"]);

    // Unknown broadcast on the latest route must 404.
    let resp = http_get(admin_addr, "/playback/latest/live/ghost").await;
    assert_eq!(resp.status, 404, "latest ghost GET status");

    // redb takes an exclusive file lock, so the running server
    // holds it. Shut down cleanly and then reopen the index from
    // this test for read-only inspection of the on-disk state.
    // Drop the RTMP stream first so the bridge stops issuing
    // fragments mid-teardown.
    drop(_s);
    server.shutdown().await.expect("shutdown");

    let db_path = archive_path.join("archive.redb");
    assert!(db_path.exists(), "archive.redb should exist at {}", db_path.display());

    let index = RedbSegmentIndex::open(&db_path).expect("open archive.redb");
    let rows = index.find_range("live/dvr", "0.mp4", 0, u64::MAX).expect("find_range");

    assert!(
        !rows.is_empty(),
        "expected at least one archived video segment for live/dvr, got none"
    );

    // Rows must be sorted ascending by start_dts.
    for window in rows.windows(2) {
        assert!(
            window[0].start_dts <= window[1].start_dts,
            "archive rows not sorted by start_dts: {} then {}",
            window[0].start_dts,
            window[1].start_dts
        );
    }

    // Every listed path must point at a real file whose length
    // matches the recorded byte count. `byte_offset` is zero for
    // the single-file-per-fragment writer, so the file size should
    // equal `length`.
    for row in &rows {
        assert_eq!(row.broadcast, "live/dvr");
        assert_eq!(row.track, "0.mp4");
        assert_eq!(row.timescale, 90_000);
        assert_eq!(row.byte_offset, 0);
        assert!(row.end_dts > row.start_dts);
        let meta = std::fs::metadata(&row.path).unwrap_or_else(|e| {
            panic!("archive row path missing on disk: {} ({e})", row.path);
        });
        assert_eq!(
            meta.len(),
            row.length,
            "archive row length {} does not match file size {} at {}",
            row.length,
            meta.len(),
            row.path
        );
        // Written fragments are fMP4 `moof+mdat`; confirm the first
        // box header looks like `moof` so we know the observer saw
        // bridge-produced bytes, not some empty placeholder.
        let contents = std::fs::read(&row.path).expect("read archive file");
        assert!(contents.len() >= 8, "archive file too short: {}", row.path);
        assert_eq!(
            &contents[4..8],
            b"moof",
            "archive file did not start with a `moof` box: {}",
            row.path
        );
    }

    // `latest()` should return the row with the greatest start_dts.
    let latest = index
        .latest("live/dvr", "0.mp4")
        .expect("latest")
        .expect("latest row exists");
    let expected_last = rows.last().unwrap();
    assert_eq!(latest.start_dts, expected_last.start_dts);
    assert_eq!(latest.segment_seq, expected_last.segment_seq);
}
