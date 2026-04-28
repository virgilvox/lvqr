//! Pure-MoQ glass-to-glass SLO end-to-end (session 159 PATH-X).
//!
//! Closes Phase A v1.1 #5 (the last open roadmap row): a real RTMP
//! publisher (the existing `scte35-rtmp-push` bin doubles as the
//! synthetic-H.264 driver) feeds a `TestServer`, the bin
//! `lvqr-moq-sample-pusher` subscribes to `<broadcast>/0.mp4` +
//! `<broadcast>/0.timing` over the relay's MoQ surface, joins
//! frames against anchors by `group_id`, and POSTs samples to
//! `POST /api/v1/slo/client-sample`. The test asserts a non-empty
//! entry appears under `transport="moq"` on `GET /api/v1/slo`.
//!
//! Wire path:
//!
//! ```text
//! scte35-rtmp-push
//!     -> ffmpeg-shaped synthetic AVC publish over RTMP
//! TestServer (RTMP listener)
//!     -> RtmpMoqBridge (session 159 timing-track wiring)
//!     -> MoqTrackSink (0.mp4 frames + group sequences)
//!     -> MoqTimingTrackSink (0.timing 16-byte LE anchors per keyframe)
//! lvqr-moq-sample-pusher
//!     -> moq-native client connect
//!     -> subscribe(<broadcast>/0.mp4)
//!     -> subscribe(<broadcast>/0.timing)
//!     -> TimingAnchorJoin lookup per video frame
//!     -> POST /api/v1/slo/client-sample
//! TestServer (admin route, dual-auth)
//!     -> LatencyTracker.record("moq", latency_ms)
//!     -> GET /api/v1/slo serializes the entry on the wire
//! ```
//!
//! Default-feature gate (no `#[ignore]`, no env-var requirement) so
//! ubuntu-latest CI exercises this on every push, mirroring the
//! `scte35_rtmp_push_smoke.rs` shape.

use std::process::Stdio;
use std::time::{Duration, Instant};

use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};

const RTMP_BIN: &str = env!("CARGO_BIN_EXE_scte35-rtmp-push");
const PUSHER_BIN: &str = env!("CARGO_BIN_EXE_lvqr-moq-sample-pusher");

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moq_sample_pusher_drives_transport_moq_entry_into_slo() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info,lvqr_moq_sample_pusher=debug,scte35_rtmp_push=info")
        .with_test_writer()
        .try_init();

    // TestServer with default config; NoopAuthProvider means the
    // /api/v1/slo/client-sample dual-auth path admits anonymous
    // pushes (the bin sends an empty bearer).
    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let admin_addr = server.admin_addr();
    let relay_port = server.relay_addr().port();

    let stream_key = "moq-timing-e2e";
    let broadcast = format!("live/{stream_key}");
    let rtmp_url = format!("rtmp://{}/live/{stream_key}", rtmp_addr);
    // Explicit IPv4 literal: TestServer binds the relay UDP socket on
    // `127.0.0.1`, but `localhost` resolves to `::1` on macOS so a
    // moq-native QUIC connect to `https://localhost:PORT` would race
    // against an unreachable IPv6 endpoint and time out. The
    // `federation_two_cluster.rs` test uses the same `127.0.0.1`
    // shape (see `crates/lvqr-cli/tests/federation_two_cluster.rs:53`).
    let relay_url = format!("https://127.0.0.1:{relay_port}");
    let slo_endpoint = format!("http://{admin_addr}/api/v1/slo/client-sample");

    // Spawn the RTMP publisher: 8-second publish, no SCTE-35
    // injection (we don't care about ad markers here, just keyframe
    // cadence). The bin emits an IDR every 60 frames at 30 fps =
    // every 2 s; an 8-second run sees ~4 keyframes => ~4 timing
    // anchors.
    let rtmp = tokio::process::Command::new(RTMP_BIN)
        .arg("--rtmp-url")
        .arg(&rtmp_url)
        .arg("--duration-secs")
        .arg("8.0")
        .arg("--inject-at-secs")
        .arg("99.0") // out-of-range so no SCTE-35 fires
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn scte35-rtmp-push");

    // Wait briefly for the publisher to push the AVC sequence
    // header + first IDR so the broadcast announces. 2 s is loose
    // headroom for a loaded macos runner.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Spawn the sample pusher against the same TestServer. 6-second
    // duration overlaps the remainder of the publisher's runtime.
    // --insecure because TestServer uses a self-signed cert.
    // --push-interval-secs 1 so we get a few samples in.
    let pusher = tokio::process::Command::new(PUSHER_BIN)
        .arg("--relay-url")
        .arg(&relay_url)
        .arg("--broadcast")
        .arg(&broadcast)
        .arg("--slo-endpoint")
        .arg(&slo_endpoint)
        .arg("--push-interval-secs")
        .arg("1.0")
        .arg("--duration-secs")
        .arg("6.0")
        .arg("--insecure")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn lvqr-moq-sample-pusher");

    // Poll GET /api/v1/slo until a transport=moq entry appears.
    // Budget: 15 s. The publisher runs 8 s + the pusher runs 6 s
    // overlapping; a moq entry should appear within ~3 s of the
    // pusher's first successful push.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut last_body = String::new();
    let mut found_moq_entry = false;
    while Instant::now() < deadline {
        let resp = http_get_with(
            admin_addr,
            "/api/v1/slo",
            HttpGetOptions {
                timeout: Duration::from_secs(2),
                ..Default::default()
            },
        )
        .await;
        if resp.status == 200 {
            last_body = String::from_utf8_lossy(&resp.body).to_string();
            // Cheap substring check for the transport label; no
            // need for a serde_json round trip in the polling loop.
            if last_body.contains(r#""transport":"moq""#) {
                found_moq_entry = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // Drain the pusher subprocess output regardless of pass / fail
    // so the assertion message can include diagnostics.
    let pusher_out = wait_with_kill(pusher, Duration::from_secs(10)).await;
    let pusher_stdout = String::from_utf8_lossy(&pusher_out.stdout).to_string();
    let pusher_stderr = String::from_utf8_lossy(&pusher_out.stderr).to_string();

    let rtmp_out = wait_with_kill(rtmp, Duration::from_secs(10)).await;
    let rtmp_stdout = String::from_utf8_lossy(&rtmp_out.stdout).to_string();

    assert!(
        found_moq_entry,
        "GET /api/v1/slo never returned a transport=moq entry within 15 s.\n\n\
         last body: {last_body}\n\n\
         rtmp bin stdout: {rtmp_stdout}\n\n\
         pusher stdout: {pusher_stdout}\n\n\
         pusher stderr (tail):\n{}\n",
        pusher_stderr
            .lines()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    );

    // Stronger assertion: parse the body and pin the transport +
    // a non-empty sample_count. The route shape is
    // `{"broadcasts": [SloEntry, ...]}` per
    // `crates/lvqr-admin/src/routes.rs::get_slo`.
    let parsed: serde_json::Value = serde_json::from_str(&last_body).expect("last_body is not valid JSON");
    let entries = parsed
        .get("broadcasts")
        .and_then(|v| v.as_array())
        .expect("SLO snapshot is missing the `broadcasts` array");
    let moq_entry = entries
        .iter()
        .find(|e| {
            e.get("transport").and_then(|v| v.as_str()) == Some("moq")
                && e.get("broadcast").and_then(|v| v.as_str()) == Some(broadcast.as_str())
        })
        .expect("no transport=moq entry on the matching broadcast");
    let sample_count = moq_entry
        .get("sample_count")
        .and_then(|v| v.as_u64())
        .expect("sample_count missing");
    assert!(sample_count >= 1, "transport=moq sample_count is zero: {moq_entry}");
    let p99 = moq_entry
        .get("p99_ms")
        .and_then(|v| v.as_u64())
        .expect("p99_ms missing");
    // Latency should be small (loopback) but cap at 5 s defensively
    // -- the bin and the route both reject samples above 5 min so
    // anything in between is a real measurement, not clock skew.
    assert!(p99 < 5_000, "p99 latency too high (likely clock skew): {p99}");

    server.shutdown().await.expect("shutdown TestServer");
}

async fn wait_with_kill(mut child: tokio::process::Child, budget: Duration) -> std::process::Output {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(_) => break,
        }
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
    }
    child.wait_with_output().await.unwrap_or_else(|_| std::process::Output {
        status: std::process::ExitStatus::default(),
        stdout: Vec::new(),
        stderr: Vec::new(),
    })
}
