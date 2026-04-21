//! End-to-end integration test for the Tier 4 item 4.7 latency SLO
//! tracker + `/api/v1/slo` admin route.
//!
//! Boots a full `TestServer`, publishes synthetic fragments directly
//! onto the shared `FragmentBroadcasterRegistry` (stamping each with
//! a wall-clock ingest time via `Fragment::with_ingest_time_ms`), and
//! waits for the HLS drain loop to record samples on the tracker.
//! Then fetches `/api/v1/slo` over HTTP and asserts the JSON body
//! surfaces the per-broadcast p50 / p95 / p99 shape.
//!
//! Real HLS drain path (no mocks), real axum HTTP round-trip, real
//! bytes on the wire. The synthetic fragments bypass RTMP to keep
//! the test hermetic + fast; the broadcaster wiring is identical to
//! what every ingest protocol drives.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);

fn minimal_init_segment() -> Bytes {
    // 24-byte `ftyp` + 16-byte minimal `moov` so the HLS policy has
    // something to hash on the push_init path. Consumers that need
    // a decodable init segment use the conformance fixtures; this
    // test only cares about the plumbing.
    let mut init = Vec::with_capacity(40);
    init.extend_from_slice(&24u32.to_be_bytes());
    init.extend_from_slice(b"ftyp");
    init.extend_from_slice(b"iso6");
    init.extend_from_slice(&0u32.to_be_bytes());
    init.extend_from_slice(b"iso6mp41");
    init.extend_from_slice(&16u32.to_be_bytes());
    init.extend_from_slice(b"moov");
    init.extend_from_slice(&[0u8; 8]);
    Bytes::from(init)
}

fn moof_mdat_fragment(seq: u64, ingest_time_ms: u64) -> Fragment {
    // Minimal `moof + mdat` pair so the HLS bridge's CMAF classifier
    // sees a boundary. Payload bytes are opaque to the SLO tracker.
    let mut payload = Vec::with_capacity(40);
    payload.extend_from_slice(&16u32.to_be_bytes());
    payload.extend_from_slice(b"moof");
    payload.extend_from_slice(&[0u8; 8]);
    payload.extend_from_slice(&16u32.to_be_bytes());
    payload.extend_from_slice(b"mdat");
    payload.extend_from_slice(&[0xAB; 8]);
    Fragment::new(
        "0.mp4",
        seq,
        0,
        0,
        seq * 1800,
        seq * 1800,
        1800,
        FragmentFlags::KEYFRAME,
        Bytes::from(payload),
    )
    .with_ingest_time_ms(ingest_time_ms)
}

async fn http_get(addr: SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("connect timeout")
        .expect("connect failed");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut buf))
        .await
        .expect("read timeout")
        .expect("read");
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("response terminator");
    let header = std::str::from_utf8(&buf[..split]).expect("headers utf8");
    let mut lines = header.lines();
    let status_line = lines.next().expect("status line");
    let mut parts = status_line.splitn(3, ' ');
    let _ = parts.next();
    let status: u16 = parts.next().expect("code").parse().expect("numeric status");
    (status, buf[split + 4..].to_vec())
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slo_route_reports_hls_latency_samples_after_publish() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();
    let registry: &FragmentBroadcasterRegistry = server.fragment_registry();

    // Simulate a broadcast starting: publish the init segment then
    // emit a handful of fragments. Stamp `ingest_time_ms` ~200ms
    // ago so the HLS drain records a positive, reproducible delta.
    let ingest_time = unix_now_ms().saturating_sub(200);
    let mut meta = FragmentMeta::new("avc1.640020", 90_000);
    meta.init_segment = Some(minimal_init_segment());
    let bc = registry.get_or_create("live/demo", "0.mp4", meta);

    for seq in 0..8u64 {
        bc.emit(moof_mdat_fragment(seq, ingest_time + seq * 5));
    }

    // Poll /api/v1/slo until the tracker reports a sample for
    // live/demo HLS. Drain task spawns asynchronously; the loop
    // lets CI hosts absorb a stray ~100 ms.
    let deadline = Instant::now() + Duration::from_secs(5);
    let body = loop {
        if Instant::now() > deadline {
            panic!("slo route never reported a live/demo hls sample");
        }
        let (status, bytes) = http_get(admin_addr, "/api/v1/slo").await;
        assert_eq!(status, 200, "status {status}");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let broadcasts = parsed
            .get("broadcasts")
            .and_then(|v| v.as_array())
            .expect("broadcasts array");
        let matched = broadcasts.iter().find(|b| {
            b["broadcast"] == "live/demo" && b["transport"] == "hls" && b["sample_count"].as_u64() == Some(8)
        });
        if let Some(entry) = matched {
            break entry.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // All eight fragments should have landed; percentiles should be
    // non-zero because the ingest stamp was 200ms behind wall clock.
    assert_eq!(body["sample_count"], 8);
    assert_eq!(body["total_observed"], 8);
    let p50 = body["p50_ms"].as_u64().expect("p50 u64");
    let p99 = body["p99_ms"].as_u64().expect("p99 u64");
    let max = body["max_ms"].as_u64().expect("max u64");
    assert!(p50 >= 150, "p50 should be at least 150 ms, got {p50}");
    assert!(p99 >= p50, "p99 ({p99}) >= p50 ({p50})");
    assert!(max >= p99, "max ({max}) >= p99 ({p99})");

    // ServerHandle accessor mirrors the admin route snapshot.
    let snapshot = server.slo().snapshot();
    assert!(
        snapshot
            .iter()
            .any(|e| e.broadcast == "live/demo" && e.transport == "hls")
    );

    server.shutdown().await.expect("shutdown");
}
