//! End-to-end integration tests for the Tier 4 item 4.7 latency SLO
//! tracker + `/api/v1/slo` admin route.
//!
//! Boots a full `TestServer`, publishes synthetic fragments directly
//! onto the shared `FragmentBroadcasterRegistry` (stamping each with
//! a wall-clock ingest time via `Fragment::with_ingest_time_ms`), and
//! waits for the egress drain loops (HLS + DASH + WS) to record
//! samples on the tracker. Then fetches `/api/v1/slo` over HTTP and
//! asserts the JSON body surfaces the per-`(broadcast, transport)`
//! p50 / p95 / p99 shape.
//!
//! Real egress drain path (no mocks), real axum HTTP round-trip,
//! real bytes on the wire. The synthetic fragments bypass RTMP to
//! keep the tests hermetic + fast; the broadcaster wiring is
//! identical to what every ingest protocol drives.
//!
//! * `slo_route_reports_hls_latency_samples_after_publish` -- 107 A
//!   original, LL-HLS drain side.
//! * `slo_route_reports_dash_latency_samples_after_publish` -- 109 A
//!   addition, MPEG-DASH drain side.
//! * `slo_route_reports_ws_latency_samples_after_publish` -- 110 A
//!   addition, WS relay session aux drain side.

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentBroadcasterRegistry, FragmentFlags, FragmentMeta};
use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

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

/// Thin wrapper over `lvqr_test_utils::http::http_get_with` that
/// preserves this file's `(status, body)` tuple shape. Every call
/// site destructures into a local let binding; a tuple return keeps
/// those bindings unchanged.
async fn http_get(addr: SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let resp = http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await;
    (resp.status, resp.body)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slo_route_reports_ws_latency_samples_after_publish() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    // Session 110 A: the WS relay session opens an auxiliary
    // fragment-registry subscription per (broadcast, track) purely
    // to sample `Fragment::ingest_time_ms` under
    // `transport="ws"`. The MoQ-side drain that feeds the wire is
    // unchanged; this test publishes synthetic fragments through
    // the registry (bypassing MoQ fanout) AND creates a MoQ
    // broadcast with named tracks so `consume_broadcast` +
    // `subscribe_track` on the session side succeed. No wire
    // frames flow on the MoQ side; the aux drain records the
    // samples the admin route then surfaces.
    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();
    let registry: &FragmentBroadcasterRegistry = server.fragment_registry();

    // 1. Create the MoQ broadcast + tracks so ws_relay_session's
    //    consume_broadcast + subscribe_track resolve. The track
    //    producers stay alive for the duration of the test so the
    //    consumer side does not receive `Closed` before we even
    //    subscribe.
    let origin = server.origin();
    let mut moq_broadcast = origin
        .create_broadcast("live/demo")
        .expect("create moq broadcast on origin");
    let _moq_video = moq_broadcast
        .create_track(lvqr_moq::Track::new("0.mp4"))
        .expect("create moq video track");

    // 2. Register the fragment broadcaster so the WS session's
    //    aux subscription finds an entry + subscribes before we
    //    emit any fragments (the registry broadcaster is a
    //    tokio::broadcast channel with no replay).
    let mut meta = FragmentMeta::new("avc1.640020", 90_000);
    meta.init_segment = Some(minimal_init_segment());
    let bc = registry.get_or_create("live/demo", "0.mp4", meta);

    // 3. Open the WS subscribe connection. Using raw TCP + the
    //    HTTP upgrade handshake via `tokio_tungstenite::client_async`
    //    keeps the test dep footprint aligned with the existing
    //    RTMP -> WS e2e test (same crate version, same
    //    `futures_util` sink imports). After the connect returns
    //    we give the server a moment to spawn `ws_relay_session`
    //    + acquire the aux subscription before we emit fragments.
    let ws_url = server.ws_url("live/demo");
    let (ws_stream, _resp) = tokio_tungstenite::connect_async(&ws_url).await.expect("ws connect");
    // Keep the stream alive so the session does not hang up on
    // send-side closure. We never read from the WS on the test
    // thread because no wire frames flow (the MoQ origin broadcast
    // has no keyframes), but the socket staying open is what holds
    // the session alive.
    let _ws_keepalive = ws_stream;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 4. Emit 8 backdated fragments onto the registry. The
    //    session's aux drain records one sample per fragment under
    //    `transport="ws"`.
    let ingest_time = unix_now_ms().saturating_sub(200);
    for seq in 0..8u64 {
        bc.emit(moof_mdat_fragment(seq, ingest_time + seq * 5));
    }

    // 5. Poll /api/v1/slo until the tracker reports 8 samples for
    //    `transport="ws"` under `live/demo`. 5 s budget, same as
    //    HLS + DASH tests.
    let deadline = Instant::now() + Duration::from_secs(5);
    let body = loop {
        if Instant::now() > deadline {
            let snap = server.slo().snapshot();
            panic!("slo route never reported a live/demo ws sample; snapshot: {snap:?}");
        }
        let (status, bytes) = http_get(admin_addr, "/api/v1/slo").await;
        assert_eq!(status, 200, "status {status}");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let broadcasts = parsed
            .get("broadcasts")
            .and_then(|v| v.as_array())
            .expect("broadcasts array");
        let matched = broadcasts
            .iter()
            .find(|b| b["broadcast"] == "live/demo" && b["transport"] == "ws" && b["sample_count"].as_u64() == Some(8));
        if let Some(entry) = matched {
            break entry.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(body["sample_count"], 8);
    assert_eq!(body["total_observed"], 8);
    let p50 = body["p50_ms"].as_u64().expect("p50 u64");
    let p99 = body["p99_ms"].as_u64().expect("p99 u64");
    let max = body["max_ms"].as_u64().expect("max u64");
    assert!(p50 >= 150, "p50 should be at least 150 ms, got {p50}");
    assert!(p99 >= p50, "p99 ({p99}) >= p50 ({p50})");
    assert!(max >= p99, "max ({max}) >= p99 ({p99})");

    // HLS stayed enabled on the TestServer by default, so both the
    // HLS drain and the WS aux drain fire on the same fragments.
    // Asserting they co-exist on the snapshot proves the cross-
    // transport dashboard story is live end-to-end through the WS
    // addition in 110 A.
    //
    // Poll for the HLS sibling entry rather than asserting once: the
    // HLS drain registration is independent of the WS aux drain and
    // can land microseconds-to-seconds later on a contended CI host
    // (specifically macos-latest under load). Reusing the earlier
    // 5 s pattern with a fresh deadline keeps the test deterministic
    // without flake.
    let cross_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = server.slo().snapshot();
        let has_ws = snapshot
            .iter()
            .any(|e| e.broadcast == "live/demo" && e.transport == "ws");
        let has_hls = snapshot
            .iter()
            .any(|e| e.broadcast == "live/demo" && e.transport == "hls");
        if has_ws && has_hls {
            break;
        }
        if Instant::now() > cross_deadline {
            panic!(
                "expected both live/demo ws + hls entries in snapshot within 5s; \
                 has_ws={has_ws} has_hls={has_hls}; snapshot: {snapshot:?}",
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slo_route_reports_dash_latency_samples_after_publish() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info")
        .with_test_writer()
        .try_init();

    // Turn the DASH surface on so `BroadcasterDashBridge` installs its
    // drain callback on the shared registry. Leaving HLS enabled is
    // fine: both drains see the same fragments and each records its
    // own transport label, which is exactly the cross-transport
    // dashboard story we want to assert is live end-to-end.
    let server = TestServer::start(TestServerConfig::default().with_dash())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();
    let registry: &FragmentBroadcasterRegistry = server.fragment_registry();

    // Same synthetic publish as the HLS case: init + 8 backdated
    // fragments so the DASH drain records a positive, reproducible
    // delta per `push_video_segment`.
    let ingest_time = unix_now_ms().saturating_sub(200);
    let mut meta = FragmentMeta::new("avc1.640020", 90_000);
    meta.init_segment = Some(minimal_init_segment());
    let bc = registry.get_or_create("live/demo", "0.mp4", meta);

    for seq in 0..8u64 {
        bc.emit(moof_mdat_fragment(seq, ingest_time + seq * 5));
    }

    // Poll /api/v1/slo until the DASH drain reports 8 samples for
    // live/demo. 5 s budget matches the HLS test; DASH drain is a
    // sibling tokio task spawned on the same runtime and registers
    // within a few hundred ms of the first fragment emit.
    let deadline = Instant::now() + Duration::from_secs(5);
    let body = loop {
        if Instant::now() > deadline {
            panic!("slo route never reported a live/demo dash sample");
        }
        let (status, bytes) = http_get(admin_addr, "/api/v1/slo").await;
        assert_eq!(status, 200, "status {status}");
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let broadcasts = parsed
            .get("broadcasts")
            .and_then(|v| v.as_array())
            .expect("broadcasts array");
        let matched = broadcasts.iter().find(|b| {
            b["broadcast"] == "live/demo" && b["transport"] == "dash" && b["sample_count"].as_u64() == Some(8)
        });
        if let Some(entry) = matched {
            break entry.clone();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(body["sample_count"], 8);
    assert_eq!(body["total_observed"], 8);
    let p50 = body["p50_ms"].as_u64().expect("p50 u64");
    let p99 = body["p99_ms"].as_u64().expect("p99 u64");
    let max = body["max_ms"].as_u64().expect("max u64");
    assert!(p50 >= 150, "p50 should be at least 150 ms, got {p50}");
    assert!(p99 >= p50, "p99 ({p99}) >= p50 ({p50})");
    assert!(max >= p99, "max ({max}) >= p99 ({p99})");

    // Both transports should show up since HLS stayed enabled; the
    // snapshot accessor surfaces them side by side.
    let snapshot = server.slo().snapshot();
    assert!(
        snapshot
            .iter()
            .any(|e| e.broadcast == "live/demo" && e.transport == "dash"),
        "expected live/demo dash entry in snapshot, got {snapshot:?}",
    );
    assert!(
        snapshot
            .iter()
            .any(|e| e.broadcast == "live/demo" && e.transport == "hls"),
        "expected live/demo hls entry in snapshot alongside dash, got {snapshot:?}",
    );

    server.shutdown().await.expect("shutdown");
}
