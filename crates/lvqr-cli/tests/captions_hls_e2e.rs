//! End-to-end test for the HLS captions surface (Tier 4 item
//! 4.5 session C).
//!
//! Spins up a `TestServer` with default config (HLS on),
//! publishes a synthetic video fragment + a synthetic caption
//! Fragment directly onto the shared
//! `FragmentBroadcasterRegistry` (skipping any real ingest
//! protocol + the whisper inference path), then drives raw-TCP
//! HTTP/1.1 GETs against the LL-HLS surface to assert the
//! master playlist declares the SUBTITLES rendition, the
//! captions playlist references a `.vtt` segment, and the
//! `.vtt` body carries the synthetic cue text.
//!
//! This test deliberately does NOT use the `whisper` Cargo
//! feature: the wire shape between the agent and the HLS
//! bridge is `(broadcast, "captions")` with `Fragment.payload`
//! = UTF-8 cue text + `dts/duration` in wall-clock UNIX ms,
//! and that contract is testable without whisper.cpp.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use lvqr_fragment::{Fragment, FragmentFlags, FragmentMeta};
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

/// Synthetic video fragment. The HLS bridge tries to push
/// this through its CMAF policy; the policy may reject the
/// fake bytes but the bridge first calls
/// `MultiHlsServer::ensure_video`, which creates the video
/// rendition state we need for the master playlist to
/// return 200.
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

fn synthetic_caption_fragment(text: &str, start_ms: u64, duration_ms: u64) -> Fragment {
    Fragment::new(
        "captions",
        0,
        0,
        0,
        start_ms,
        start_ms,
        duration_ms,
        FragmentFlags::KEYFRAME,
        Bytes::copy_from_slice(text.as_bytes()),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caption_fragment_flows_through_to_hls_subtitle_rendition() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let hls_addr = server.hls_addr();
    let registry = server.fragment_registry().clone();

    // Step 1: publish a synthetic video fragment so the HLS
    // bridge creates the per-broadcast video rendition.
    // master.m3u8 returns 404 until the video rendition
    // exists, regardless of what subtitles renditions report.
    let video_meta = FragmentMeta::new("avc1.640028", 90_000);
    let video_bc = registry.get_or_create("live/cam1", "0.mp4", video_meta);
    video_bc.emit(synthetic_video_fragment());

    // Step 2: publish a synthetic caption fragment onto the
    // captions track. dts + duration in wall-clock UNIX ms
    // (the producer convention from
    // lvqr_agent_whisper::worker::run_inference).
    let captions_meta = FragmentMeta::new("wvtt", 1000);
    let captions_bc = registry.get_or_create("live/cam1", "captions", captions_meta);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    captions_bc.emit(synthetic_caption_fragment("hello captions", now_ms, 4_500));

    // Hold the producer-side clones alive long enough for
    // the registry's on_entry_created callback to fire +
    // spawn the drain tasks + drain at least one fragment
    // each. The bridges spawn synchronously; the drains run
    // on the test's tokio runtime, so a single yield window
    // is sufficient.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Step 3: master playlist must declare the SUBTITLES
    // rendition and the variant must reference the subs
    // group. The video rendition is also present (HLS
    // bridge created it from the video fragment above).
    let resp = http_get(hls_addr, "/hls/live/cam1/master.m3u8").await;
    assert_eq!(resp.status, 200, "master.m3u8 status");
    let body = std::str::from_utf8(&resp.body).expect("master.m3u8 body utf-8");
    eprintln!("--- master.m3u8 ---\n{body}\n--- end ---");
    assert!(
        body.contains("#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\""),
        "master must declare SUBTITLES rendition: {body}"
    );
    assert!(
        body.contains("URI=\"captions/playlist.m3u8\""),
        "master must reference captions playlist URI: {body}"
    );
    assert!(
        body.contains("LANGUAGE=\"en\""),
        "captions rendition must declare English: {body}"
    );
    assert!(
        body.contains("SUBTITLES=\"subs\""),
        "video variant must reference subs group: {body}"
    );

    // Step 4: captions playlist must reference seg-0.vtt
    // with an EXTINF entry.
    let resp = http_get(hls_addr, "/hls/live/cam1/captions/playlist.m3u8").await;
    assert_eq!(resp.status, 200, "captions playlist status");
    let body = std::str::from_utf8(&resp.body).expect("captions playlist body utf-8");
    eprintln!("--- captions/playlist.m3u8 ---\n{body}\n--- end ---");
    assert!(body.starts_with("#EXTM3U"), "playlist must start with #EXTM3U");
    assert!(
        body.contains("#EXTINF:5.000,") || body.contains("#EXTINF:6.000,"),
        "playlist must contain an EXTINF entry: {body}"
    );
    assert!(body.contains("seg-0.vtt"), "playlist must reference seg-0.vtt: {body}");

    // Step 5: the .vtt segment body must carry the cue text.
    let resp = http_get(hls_addr, "/hls/live/cam1/captions/seg-0.vtt").await;
    assert_eq!(resp.status, 200, ".vtt segment status");
    let body = std::str::from_utf8(&resp.body).expect("vtt body utf-8");
    eprintln!("--- captions/seg-0.vtt ---\n{body}\n--- end ---");
    assert!(
        body.starts_with("WEBVTT\n\n"),
        "vtt body must start with WEBVTT header: {body}"
    );
    assert!(
        body.contains("hello captions"),
        "vtt body must contain the cue text: {body}"
    );
    assert!(
        body.contains(" --> "),
        "vtt body must contain a cue timestamp arrow: {body}"
    );

    // Step 6: a missing .vtt segment must 404, not 500.
    let resp = http_get(hls_addr, "/hls/live/cam1/captions/seg-99.vtt").await;
    assert_eq!(resp.status, 404, "unknown .vtt segment must 404");

    drop(video_bc);
    drop(captions_bc);
    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn master_playlist_omits_subtitles_when_no_captions_track() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let hls_addr = server.hls_addr();
    let registry = server.fragment_registry().clone();

    // Publish only video (no captions track at all).
    let video_meta = FragmentMeta::new("avc1.640028", 90_000);
    let video_bc = registry.get_or_create("live/cam1", "0.mp4", video_meta);
    video_bc.emit(synthetic_video_fragment());
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = http_get(hls_addr, "/hls/live/cam1/master.m3u8").await;
    assert_eq!(resp.status, 200);
    let body = std::str::from_utf8(&resp.body).expect("master.m3u8 utf-8");
    assert!(
        !body.contains("TYPE=SUBTITLES"),
        "no captions producer means no SUBTITLES rendition: {body}"
    );
    assert!(
        !body.contains("SUBTITLES=\""),
        "video variant must not reference subs group when none exists: {body}"
    );

    drop(video_bc);
    server.shutdown().await.expect("shutdown");
}
