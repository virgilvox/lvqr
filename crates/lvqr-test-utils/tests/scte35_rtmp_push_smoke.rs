//! End-to-end smoke for the `scte35-rtmp-push` test bin (session 155).
//!
//! Spawns the bin against a `TestServer`'s ephemeral RTMP listener,
//! polls the relay's HLS variant playlist for a `#EXT-X-DATERANGE`
//! line, and asserts the daterange ID matches the bin's emitted
//! `event_id`. Real network end-to-end (no mocks) per CLAUDE.md.
//!
//! Establishes the load-bearing default-gate coverage for the bin
//! independent of the Playwright e2e (which is gated on
//! `LVQR_LIVE_RTMP_TESTS=1` and only runs in CI). Captures the full
//! wire path:
//!
//! ```text
//! scte35-rtmp-push
//!     -> TCP / RTMP handshake
//!     -> ClientSession.request_connection / request_publishing
//!     -> publish_video_data (AVC sequence header + IDR + P-slices)
//!     -> publish_amf0_data (onCuePoint scte35-bin64)
//! TestServer (RTMP listener)
//!     -> ServerSessionEvent::Amf0DataReceived (session 152 patch)
//!     -> parse_oncuepoint_scte35 -> base64-decode -> bridge
//!     -> publish_scte35 onto FragmentBroadcasterRegistry
//! HLS bridge
//!     -> manifest renderer (#EXT-X-DATERANGE per HLS 4.4.5.1)
//! ```

use std::process::Stdio;
use std::time::{Duration, Instant};

use lvqr_test_utils::http::{HttpGetOptions, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};

const BIN_PATH: &str = env!("CARGO_BIN_EXE_scte35-rtmp-push");

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scte35_rtmp_push_renders_daterange_in_variant_playlist() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug,scte35_rtmp_push=info")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    let stream_key = "rtmp-push-smoke";
    let rtmp_url = format!("rtmp://{}/live/{stream_key}", rtmp_addr);

    // Spawn the bin: 6-second publish, single onCuePoint at offset
    // 2.0 s (early enough that ffprobe-style segment finalize closes
    // a segment containing the cue before the bin exits).
    let mut child = tokio::process::Command::new(BIN_PATH)
        .arg("--rtmp-url")
        .arg(&rtmp_url)
        .arg("--duration-secs")
        .arg("6.0")
        .arg("--inject-at-secs")
        .arg("2.0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn scte35-rtmp-push");

    // Poll the variant playlist for the DATERANGE line. The bin
    // takes ~6 s to run end-to-end; the relay's HLS bridge needs at
    // least one IDR-bounded segment to render a non-empty playlist
    // body. 30-second budget gives ~5x headroom on a loaded macos
    // CI runner.
    let path = format!("/hls/live/{stream_key}/playlist.m3u8");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last_body = String::new();
    let mut found = false;
    while Instant::now() < deadline {
        let resp = http_get_with(
            hls_addr,
            &path,
            HttpGetOptions {
                timeout: Duration::from_secs(2),
                ..Default::default()
            },
        )
        .await;
        if resp.status == 200 {
            last_body = String::from_utf8_lossy(&resp.body).to_string();
            if last_body.contains("#EXT-X-DATERANGE") {
                found = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    // Wait up to 12 s for the bin to finish naturally (its own
    // --duration-secs is 6); SIGKILL only as a last resort so the
    // bin's stdout JSON exit line gets flushed cleanly.
    let exit_deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < exit_deadline {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(_) => break,
        }
    }
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
    }
    let output = child.wait_with_output().await.expect("collect bin output");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        found,
        "variant playlist never carried #EXT-X-DATERANGE within 30 s.\n\
         last body:\n{last_body}\n\nbin stdout:\n{stdout}\n\nbin stderr (tail):\n{}\n",
        stderr
            .lines()
            .rev()
            .take(40)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n"),
    );

    // The default --scte35-hex fixture uses event_id 0xCAFEBABE so
    // the relay-rendered ID must contain the decimal form.
    assert!(
        last_body.contains("ID=\"splice-3405691582\""),
        "DATERANGE ID must be derived from event_id 0xCAFEBABE = 3405691582; body:\n{last_body}",
    );
    assert!(
        last_body.contains("CLASS=\"urn:scte:scte35:2014:bin\""),
        "DATERANGE must carry the SCTE-35 CLASS attribute; body:\n{last_body}",
    );

    // The bin's stdout JSON line must report at least one event +
    // some frames -- a smoke-test parity guard so a future bin
    // regression that silently sends nothing surfaces here.
    assert!(
        stdout.contains("\"events_sent\":1"),
        "bin must report events_sent=1 on stdout; stdout was:\n{stdout}",
    );

    server.shutdown().await.expect("shutdown TestServer");
}
