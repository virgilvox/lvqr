//! End-to-end exercise of the `lvqr-hls` axum router.
//!
//! Drives real HTTP requests through [`HlsServer::router`] via
//! `tower::ServiceExt::oneshot`, without binding a TCP listener.
//! This is the "integration" slot of the 5-artifact contract for
//! the `server` module and the "e2e" slot of the crate-level
//! contract (the whole router surface is exercised end-to-end, just
//! over the axum service trait rather than a loopback socket).
//!
//! A loopback-TCP version of this test lands when `lvqr-cli`
//! composes HLS into its serve path; at that point
//! `lvqr-test-utils::TestServer` grows an HLS axum address and the
//! real HTTP handshake can be exercised through `TestServer::http_base()`.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use http_body_util::BodyExt;
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{HlsServer, PlaylistBuilderConfig};
use std::time::Duration;
use tower::ServiceExt;

fn chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
    CmafChunk {
        track_id: "0.mp4".into(),
        payload: Bytes::from_static(b""),
        dts,
        duration,
        kind,
    }
}

async fn get_body(server: &HlsServer, path: &str) -> (StatusCode, String, Vec<u8>) {
    let router = server.router();
    let req = Request::builder().uri(path).body(Body::empty()).unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, content_type, bytes)
}

#[tokio::test]
async fn playlist_init_and_segment_round_trip() {
    let server = HlsServer::new(PlaylistBuilderConfig {
        timescale: 90_000,
        starting_sequence: 0,
        map_uri: "init.mp4".into(),
        uri_prefix: String::new(),
        target_duration_secs: 2,
        part_target_secs: 0.2,
    });

    // Publish an init segment and a full 200 ms-per-part 2 s
    // segment: 10 parts, the first is a keyframe.
    server.push_init(Bytes::from_static(b"\x00init_bytes")).await;
    let part_dur = 18_000u64;
    let mut dts = 0u64;
    for i in 0..10 {
        let kind = if i == 0 {
            CmafChunkKind::Segment
        } else {
            CmafChunkKind::Partial
        };
        let body = Bytes::from(format!("part-{i}-body").into_bytes());
        server
            .push_chunk_bytes(&chunk(dts, part_dur, kind), body)
            .await
            .unwrap();
        dts += part_dur;
    }
    // Fire a second Segment-kind chunk to close the first segment
    // into the manifest.
    server
        .push_chunk_bytes(
            &chunk(dts, part_dur, CmafChunkKind::Segment),
            Bytes::from_static(b"seg1-part0"),
        )
        .await
        .unwrap();

    // /playlist.m3u8 returns the rendered manifest.
    let (status, ct, body) = get_body(&server, "/playlist.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/vnd.apple.mpegurl");
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.starts_with("#EXTM3U"));
    assert!(text.contains("#EXT-X-VERSION:9"));
    assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4\""));
    assert!(text.contains("seg-0.m4s"));

    // /init.mp4 returns the published init bytes.
    let (status, ct, body) = get_body(&server, "/init.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "video/mp4");
    assert_eq!(body, b"\x00init_bytes");

    // The part URIs from the playlist resolve back to their bytes.
    // Harvest the URIs directly from the manifest view.
    let uris: Vec<String> = {
        let r = server.router();
        let _ = r; // we only use `server` below
        let builder_view = server.clone();
        // Re-render so we have a fresh borrow of the manifest.
        let (_, _, body) = get_body(&builder_view, "/playlist.m3u8").await;
        std::str::from_utf8(&body)
            .unwrap()
            .lines()
            .filter_map(|line| {
                line.strip_prefix("#EXT-X-PART:")
                    .and_then(|rest| rest.find("URI=\"").map(|i| &rest[i + 5..]))
                    .and_then(|s| s.find('"').map(|end| s[..end].to_string()))
            })
            .collect()
    };
    assert!(!uris.is_empty(), "expected at least one part URI");
    for uri in &uris {
        let (status, ct, body) = get_body(&server, &format!("/{uri}")).await;
        assert_eq!(status, StatusCode::OK, "fetching {uri}");
        assert_eq!(ct, "video/iso.segment");
        assert!(body.starts_with(b"part-") || body == b"seg1-part0");
    }
}

#[tokio::test]
async fn closed_segment_uri_serves_coalesced_bytes() {
    // Regression test for the LL-HLS closed-segment-bytes cache
    // bug surfaced by the first `hls-conformance.yml` CI run: the
    // audio sub-playlist listed `audio-seg-0.m4s` under an
    // `#EXTINF` line but `HlsServer::push_chunk_bytes` only cached
    // partial URIs, so a plain HLS client (ffmpeg, Safari fallback)
    // that followed the `#EXTINF` link got a 404. Session 33 fixes
    // this by coalescing constituent part bytes into a single blob
    // on every segment close. This test exercises the fix by
    // pushing 10 partials plus one more Segment-kind chunk to
    // close segment 0, then GET-ing `/seg-0.m4s` and asserting
    // the body is the concatenation of the 10 pushed part bodies.
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    server.push_init(Bytes::from_static(b"\x00init")).await;

    let part_dur = 18_000u64;
    let mut dts = 0u64;
    let mut expected_seg0: Vec<u8> = Vec::new();
    for i in 0..10 {
        let kind = if i == 0 {
            CmafChunkKind::Segment
        } else {
            CmafChunkKind::Partial
        };
        let body_bytes = format!("part-{i}-body").into_bytes();
        expected_seg0.extend_from_slice(&body_bytes);
        server
            .push_chunk_bytes(&chunk(dts, part_dur, kind), Bytes::from(body_bytes))
            .await
            .unwrap();
        dts += part_dur;
    }
    // One more Segment-kind push closes segment 0 into the
    // rendered playlist; its bytes are the concat of the prior
    // 10 part bodies we captured above.
    server
        .push_chunk_bytes(
            &chunk(dts, part_dur, CmafChunkKind::Segment),
            Bytes::from_static(b"seg1-part0"),
        )
        .await
        .unwrap();

    // The playlist now lists seg-0.m4s under an #EXTINF line; the
    // cache must return the coalesced bytes for it.
    let (status, ct, body) = get_body(&server, "/seg-0.m4s").await;
    assert_eq!(status, StatusCode::OK, "GET /seg-0.m4s must succeed after close");
    assert_eq!(ct, "video/iso.segment");
    assert_eq!(body, expected_seg0, "coalesced bytes must equal concat of pushed parts");
    assert!(!body.is_empty(), "coalesced segment body must be non-empty");
}

#[tokio::test]
async fn init_returns_404_before_push() {
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    let (status, _ct, _body) = get_body(&server, "/init.mp4").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_uri_returns_404() {
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    let (status, _ct, _body) = get_body(&server, "/part-999-9.m4s").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blocking_reload_waits_for_target_media_sequence() {
    // The client requests the playlist with _HLS_msn=1 before
    // segment 1 has been published. The handler must park until
    // segment 1 closes.
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    server.push_init(Bytes::from_static(b"init")).await;
    // Seed segment 0 so segments is non-empty.
    server
        .push_chunk_bytes(&chunk(0, 180_000, CmafChunkKind::Segment), Bytes::from_static(b"seg0"))
        .await
        .unwrap();

    let client = server.clone();
    let pending = tokio::spawn(async move { get_body(&client, "/playlist.m3u8?_HLS_msn=1").await });

    // Give the handler a moment to park on the notify. If we
    // publish immediately the test still passes; the sleep is a
    // real-time observation that the handler actually does block.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!pending.is_finished(), "handler should be parked on _HLS_msn=1");

    // Push two more Segment-kind chunks. The second pushes
    // segment 1 onto the closed list (sequence = 1), satisfying
    // `any(|s| s.sequence >= 1)`. The first of these two chunks
    // only advances the open-segment counter; it does not yet
    // close segment 1.
    server
        .push_chunk_bytes(
            &chunk(180_000, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"seg1-part0"),
        )
        .await
        .unwrap();
    server
        .push_chunk_bytes(
            &chunk(360_000, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"seg2-part0"),
        )
        .await
        .unwrap();

    let (status, _ct, body) = tokio::time::timeout(Duration::from_secs(2), pending)
        .await
        .expect("blocking reload resolved in time")
        .expect("task joined");
    assert_eq!(status, StatusCode::OK);
    let text = std::str::from_utf8(&body).unwrap();
    // Segments 0 and 1 are now closed; the rendered manifest
    // starts its #EXT-X-MEDIA-SEQUENCE at 0 (the oldest segment
    // still retained) and contains URIs for both.
    assert!(text.contains("#EXT-X-MEDIA-SEQUENCE:0"));
    assert!(text.contains("seg-0.m4s"));
    assert!(text.contains("seg-1.m4s"));
}

#[tokio::test]
async fn delta_playlist_returns_ext_x_skip_for_long_enough_window() {
    // Drive the `_HLS_skip=YES` query through the real axum router
    // and prove the response body contains an `#EXT-X-SKIP` tag.
    // 10 segments * 2 s each = 20 s total; default
    // CAN-SKIP-UNTIL=12 s; the delta walk emits 3 skipped
    // segments and keeps 7.
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    server.push_init(Bytes::from_static(b"init")).await;
    for i in 0..10u64 {
        let dts = i * 180_000;
        server
            .push_chunk_bytes(
                &chunk(dts, 180_000, CmafChunkKind::Segment),
                Bytes::from(format!("seg-{i}-body").into_bytes()),
            )
            .await
            .unwrap();
    }
    // Force-close the last pending segment so all 10 segments
    // appear in `manifest.segments` and the delta walk can see the
    // full 20 s of closed duration. Without this, the last
    // Segment-kind chunk would sit in `preliminary_parts` and the
    // delta window would only run against 9 closed segments.
    server.close_pending_segment().await;

    let (status, ct, body) = get_body(&server, "/playlist.m3u8?_HLS_skip=YES").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/vnd.apple.mpegurl");
    let text = std::str::from_utf8(&body).unwrap();
    assert!(
        text.contains("#EXT-X-SKIP:SKIPPED-SEGMENTS=3"),
        "delta playlist missing EXT-X-SKIP:\n{text}"
    );
    assert!(text.contains("CAN-SKIP-UNTIL=12.000"), "body:\n{text}");
    // The first three segment URIs are absent; the fourth (index 3)
    // is the first kept segment.
    assert!(!text.contains("seg-0.m4s"));
    assert!(!text.contains("seg-2.m4s"));
    assert!(text.contains("seg-3.m4s"));
    assert!(text.contains("seg-9.m4s"));

    // Same playlist without the directive is the full variant.
    let (status, _, body) = get_body(&server, "/playlist.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let text = std::str::from_utf8(&body).unwrap();
    assert!(!text.contains("#EXT-X-SKIP"), "full variant must not skip:\n{text}");
    assert!(text.contains("seg-0.m4s"));
}

#[tokio::test]
async fn delta_playlist_ignores_skip_when_below_spec_floor() {
    // A short playlist (6 s total) sits below the 6 * TARGETDURATION
    // floor so the server MUST NOT emit a delta regardless of the
    // query. Assert the body is identical to the full variant.
    let server = HlsServer::new(PlaylistBuilderConfig::default());
    server.push_init(Bytes::from_static(b"init")).await;
    for i in 0..3u64 {
        let dts = i * 180_000;
        server
            .push_chunk_bytes(
                &chunk(dts, 180_000, CmafChunkKind::Segment),
                Bytes::from(format!("seg-{i}-body").into_bytes()),
            )
            .await
            .unwrap();
    }

    let (status, _, body) = get_body(&server, "/playlist.m3u8?_HLS_skip=YES").await;
    assert_eq!(status, StatusCode::OK);
    let text = std::str::from_utf8(&body).unwrap();
    assert!(
        !text.contains("#EXT-X-SKIP"),
        "short playlist must ignore _HLS_skip directive:\n{text}"
    );
    // Every segment must be present.
    assert!(text.contains("seg-0.m4s"));
    assert!(text.contains("seg-1.m4s"));
}
