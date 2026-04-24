//! RTMP ingest -> transcode ladder -> LL-HLS master-playlist
//! end-to-end integration test.
//!
//! Gated on the `transcode` + `rtmp` Cargo features. When GStreamer's
//! required plugin set is not installed on the host, the test prints a
//! soft-skip message and returns early so CI hosts without the heavy
//! dep closure stay green.
//!
//! # Shape
//!
//! * A real `rml_rtmp` client publishes video + audio FLV tags to the
//!   `lvqr_cli::start`-driven server. The synthetic NAL payloads are
//!   the same ones `rtmp_hls_e2e.rs` uses; they flow through the real
//!   RTMP bridge + real FragmentBroadcasterRegistry + real
//!   TranscodeRunner just like a production publish.
//! * The HLS master playlist at `/hls/live/demo/master.m3u8` MUST
//!   advertise four variants (the source + the 720p / 480p / 240p
//!   rendition siblings) with the ladder's `BANDWIDTH` /
//!   `RESOLUTION` / URI attributes.
//! * The `TranscodeRunner` handle MUST report non-zero fragment
//!   counts for each rendition's software video factory AND audio
//!   passthrough factory so the ladder genuinely observed the
//!   source.
//! * The `AudioPassthroughTranscoderFactory` MUST have republished
//!   source audio fragments onto `<source>/<rendition>/1.mp4`
//!   broadcasters that the LL-HLS bridge drains into per-rendition
//!   `audio.m3u8` playlists. Verify by fetching each rendition's
//!   audio playlist off the HLS surface.
//!
//! The real-decode path (GStreamer producing output fragments from a
//! real CMAF fixture) is already verified in
//! `crates/lvqr-transcode/tests/software_ladder.rs`; the 106 C
//! contract here is the composition root + master-playlist
//! composition + per-rendition broadcaster plumbing.

#![cfg(all(feature = "transcode", feature = "rtmp"))]

use lvqr_test_utils::flv::{
    flv_audio_aac_lc_seq_header_44k_stereo, flv_audio_raw, flv_video_nalu, flv_video_seq_header,
};
use lvqr_test_utils::http::{HttpGetOptions, HttpResponse, http_get_with};
use lvqr_test_utils::rtmp::{read_until, rtmp_client_handshake, send_result, send_results};
use lvqr_test_utils::{TestServer, TestServerConfig};
use lvqr_transcode::{RenditionSpec, SoftwareTranscoderFactory};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(10);
/// How long we're willing to wait for the ladder to register its
/// sibling broadcasts on the HLS server after RTMP publish starts.
const DRAIN_DEADLINE: Duration = Duration::from_secs(15);

/// Probe the GStreamer plugin set. Returns a skip reason when the
/// required elements are absent.
fn skip_reason() -> Option<String> {
    let probe = SoftwareTranscoderFactory::new(
        RenditionSpec::preset_720p(),
        lvqr_fragment::FragmentBroadcasterRegistry::new(),
    );
    if !probe.is_available() {
        return Some(format!(
            "skipping transcode_ladder_e2e: required GStreamer elements missing {:?}",
            probe.missing_elements()
        ));
    }
    None
}

// =====================================================================
// RTMP client helpers (same shape as rtmp_hls_e2e.rs). FLV tag
// builders + http_get now live in `lvqr_test_utils::{flv, http}`.
// =====================================================================

async fn connect_and_publish(addr: SocketAddr, app: &str, key: &str) -> (TcpStream, ClientSession) {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .unwrap()
        .unwrap();
    stream.set_nodelay(true).unwrap();
    let remaining = rtmp_client_handshake(&mut stream).await;

    let config = ClientSessionConfig::new();
    let (mut session, initial) = ClientSession::new(config).unwrap();
    send_results(&mut stream, &initial).await;
    if !remaining.is_empty() {
        let results = session.handle_input(&remaining).unwrap();
        send_results(&mut stream, &results).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let connect = session.request_connection(app.to_string()).unwrap();
    send_result(&mut stream, &connect).await;
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::ConnectionRequestAccepted)
    })
    .await;

    let publish = session
        .request_publishing(key.to_string(), PublishRequestType::Live)
        .unwrap();
    send_result(&mut stream, &publish).await;
    read_until(&mut stream, &mut session, TIMEOUT, |e| {
        matches!(e, ClientSessionEvent::PublishRequestAccepted)
    })
    .await;

    (stream, session)
}

// =====================================================================
// Thin HTTP GET wrapper. The original returned `Result<HttpResponse,
// String>` so callers could surface the path in error messages; the
// shared helper panics with generic context on connect/read failure.
// We preserve the Result-returning signature so the call sites'
// existing `?`-propagation style stays unchanged; the shared helper
// is fail-fast in practice so the Err arm is unreachable today.
// =====================================================================

#[allow(clippy::unnecessary_wraps)]
async fn http_get(addr: SocketAddr, path: &str) -> Result<HttpResponse, String> {
    let resp = http_get_with(
        addr,
        path,
        HttpGetOptions {
            timeout: TIMEOUT,
            ..Default::default()
        },
    )
    .await;
    Ok(resp)
}

fn extract_attr(line: &str, key: &str) -> Option<String> {
    let tag = format!("{key}=");
    let start = line.find(&tag)? + tag.len();
    let rest = &line[start..];
    if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        Some(stripped[..end].to_string())
    } else {
        let end = rest.find(',').unwrap_or(rest.len());
        Some(rest[..end].to_string())
    }
}

struct ParsedVariant {
    bandwidth_bps: u64,
    resolution: Option<String>,
    uri: String,
}

fn parse_variants(body: &str) -> Vec<ParsedVariant> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.starts_with("#EXT-X-STREAM-INF") {
            continue;
        }
        let bandwidth = extract_attr(line, "BANDWIDTH")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let resolution = extract_attr(line, "RESOLUTION");
        let uri = lines.next().unwrap_or("").trim().to_string();
        out.push(ParsedVariant {
            bandwidth_bps: bandwidth,
            resolution,
            uri,
        });
    }
    out
}

// =====================================================================
// Publish helper: video seq + audio seq + two keyframes spaced past
// the 2 s segment boundary plus a handful of audio frames so the
// audio-passthrough transcoder has something to forward.
// =====================================================================

async fn publish_fixture(rtmp_addr: SocketAddr) -> (TcpStream, ClientSession) {
    let (mut stream, mut session) = connect_and_publish(rtmp_addr, "live", "demo").await;

    let vseq = flv_video_seq_header();
    let r = session.publish_video_data(vseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    let aseq = flv_audio_aac_lc_seq_header_44k_stereo();
    let r = session.publish_audio_data(aseq, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    let nalu = vec![0x00, 0x00, 0x00, 0x04, 0x65, 0x88, 0x84, 0x00];
    let kf0 = flv_video_nalu(true, 0, &nalu);
    let r = session.publish_video_data(kf0, RtmpTimestamp::new(0), false).unwrap();
    send_result(&mut stream, &r).await;

    for ts in [0u32, 500, 1000, 1500, 2000] {
        let aac = flv_audio_raw(&[0u8; 64]);
        let r = session.publish_audio_data(aac, RtmpTimestamp::new(ts), false).unwrap();
        send_result(&mut stream, &r).await;
    }

    let kf1 = flv_video_nalu(true, 0, &nalu);
    let r = session
        .publish_video_data(kf1, RtmpTimestamp::new(2100), false)
        .unwrap();
    send_result(&mut stream, &r).await;

    (stream, session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transcode_ladder_master_playlist_advertises_every_rung() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }

    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=info,lvqr_transcode=debug")
        .with_test_writer()
        .try_init();

    // Boot the full stack with the default 720p / 480p / 240p ladder.
    let server = TestServer::start(TestServerConfig::default().with_transcode_ladder(RenditionSpec::default_ladder()))
        .await
        .expect("start TestServer");
    let rtmp_addr = server.rtmp_addr();
    let hls_addr = server.hls_addr();

    // Real RTMP publish with synthetic NALs. The GStreamer software
    // pipelines on each rendition factory receive fragments but cannot
    // decode the synthetic payloads into real pictures (that path is
    // covered by the 105 B software_ladder.rs integration test against
    // a real CMAF fixture). What the 106 C surface exercises here is
    // the composition root wiring: master playlist emits a variant per
    // rendition sibling, per-rendition audio passthrough forwards the
    // source AAC verbatim, and the TranscodeRunner handle reports
    // non-zero counters.
    let (_stream, _session) = publish_fixture(rtmp_addr).await;

    // Poll the master playlist until it advertises all four variants
    // (source + three rendition siblings). The software pipelines
    // register their output broadcasters on the HLS server via
    // `BroadcasterHlsBridge::install` the moment they publish any
    // fragment; the AudioPassthrough factory does the same for the
    // audio track, so the ladder siblings show up even when the video
    // decoder fails downstream.
    let deadline = Instant::now() + DRAIN_DEADLINE;
    let (master_body, variants) = loop {
        if Instant::now() > deadline {
            panic!("master playlist never reached 4 variants within {DRAIN_DEADLINE:?}");
        }
        let resp = match http_get(hls_addr, "/hls/live/demo/master.m3u8").await {
            Ok(r) if r.status == 200 => r,
            _ => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let body = match std::str::from_utf8(&resp.body) {
            Ok(s) => s.to_string(),
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let variants = parse_variants(&body);
        if variants.len() >= 4 {
            break (body, variants);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    eprintln!("--- master.m3u8 ---\n{master_body}\n--- end ---");

    assert_eq!(variants.len(), 4, "master playlist must advertise 4 variants");

    for (name, expected_res, expected_uri) in [
        ("720p", "1280x720", "./720p/playlist.m3u8"),
        ("480p", "854x480", "./480p/playlist.m3u8"),
        ("240p", "426x240", "./240p/playlist.m3u8"),
    ] {
        let found = variants
            .iter()
            .find(|v| v.uri == expected_uri)
            .unwrap_or_else(|| panic!("variant {name} missing URI {expected_uri}: {master_body}"));
        assert_eq!(
            found.resolution.as_deref(),
            Some(expected_res),
            "variant {name} resolution: {master_body}"
        );
        assert!(
            found.bandwidth_bps > 0,
            "variant {name} missing bandwidth: {master_body}"
        );
    }

    // Source variant carries at least the highest rung's bandwidth.
    let source = variants
        .iter()
        .find(|v| v.uri == "playlist.m3u8")
        .expect("source variant missing from master playlist");
    let top_rung = variants
        .iter()
        .filter(|v| v.uri.starts_with("./"))
        .map(|v| v.bandwidth_bps)
        .max()
        .unwrap_or(0);
    assert!(
        source.bandwidth_bps >= top_rung,
        "source variant bandwidth {} must be >= top rung {}",
        source.bandwidth_bps,
        top_rung
    );

    // Per-rendition audio playlist fetches succeed: this proves the
    // AudioPassthrough factory actually registered the rendition
    // audio broadcaster on the HLS server.
    for name in ["720p", "480p", "240p"] {
        let audio_path = format!("/hls/live/demo/{name}/audio.m3u8");
        let resp = http_get(hls_addr, &audio_path)
            .await
            .unwrap_or_else(|e| panic!("audio playlist fetch failed for {audio_path}: {e}"));
        assert_eq!(
            resp.status,
            200,
            "rendition {name} audio playlist status: body {}",
            String::from_utf8_lossy(&resp.body)
        );
        let body = std::str::from_utf8(&resp.body).unwrap_or("");
        assert!(
            body.starts_with("#EXTM3U"),
            "rendition {name} audio playlist not well-formed: {body}",
        );
    }

    // TranscodeRunner counters: each rendition's software video
    // factory AND audio-passthrough factory observed the source.
    // The drain tasks spawn inside the registry's on_entry_created
    // callback and run asynchronously; add a short retry window so
    // the counters have time to catch up with the publish before
    // we read them.
    let runner = server.transcode_runner().expect("transcode runner handle");
    let counter_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let all_seen = ["720p", "480p", "240p"].iter().all(|rendition| {
            runner.fragments_seen("software", rendition, "live/demo", "0.mp4") > 0
                && runner.fragments_seen("audio-passthrough", rendition, "live/demo", "1.mp4") > 0
        });
        if all_seen {
            break;
        }
        if Instant::now() > counter_deadline {
            panic!(
                "transcode counters never went non-zero for every rendition; tracked = {:?}",
                runner.tracked()
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    drop(_stream);
    drop(_session);
    server.shutdown().await.expect("shutdown");
}
