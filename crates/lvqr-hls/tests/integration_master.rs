//! End-to-end exercise of the [`MultiHlsServer`] master / audio
//! rendition routing landed in session 13.
//!
//! Drives real HTTP requests through [`MultiHlsServer::router`] via
//! `tower::ServiceExt::oneshot`. No TCP socket; the existing
//! single-rendition integration test in `integration_server.rs`
//! covers the per-`HlsServer` path, and the `lvqr-cli` workspace
//! crate exercises the multi-broadcast router end-to-end over a
//! loopback socket in `crates/lvqr-cli/tests/rtmp_hls_e2e.rs`. This
//! test is the "no producer" slice of the master playlist surface:
//! we drive `ensure_video` / `ensure_audio` directly with synthetic
//! chunks so we can assert the master-playlist generation,
//! per-rendition routing, and the prefixed cache lookup all work
//! independently of any RTMP plumbing.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use http_body_util::BodyExt;
use lvqr_cmaf::{CmafChunk, CmafChunkKind};
use lvqr_hls::{MultiHlsServer, PlaylistBuilderConfig};
use tower::ServiceExt;

fn video_chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
    CmafChunk {
        track_id: "0.mp4".into(),
        payload: Bytes::from_static(b"video-bytes"),
        dts,
        duration,
        kind,
    }
}

fn audio_chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
    CmafChunk {
        track_id: "1.mp4".into(),
        payload: Bytes::from_static(b"audio-bytes"),
        dts,
        duration,
        kind,
    }
}

async fn get(multi: &MultiHlsServer, path: &str) -> (StatusCode, String, Vec<u8>) {
    let router = multi.router();
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
async fn master_playlist_includes_audio_rendition_when_both_tracks_present() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());

    // --- Push a video init + one segment chunk into the broadcast. ---
    let video = multi.ensure_video("live/test");
    video.push_init(Bytes::from_static(b"video-init")).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .expect("push video chunk");

    // --- Push an audio init + one segment chunk into the same broadcast. ---
    let audio = multi.ensure_audio("live/test");
    audio.push_init(Bytes::from_static(b"audio-init")).await;
    audio
        .push_chunk_bytes(
            &audio_chunk(0, 96_000, CmafChunkKind::Segment),
            Bytes::from_static(b"audio-chunk-0"),
        )
        .await
        .expect("push audio chunk");

    // --- Master playlist: must declare both renditions. ---
    let (status, ct, body) = get(&multi, "/hls/live/test/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/vnd.apple.mpegurl");
    let body = String::from_utf8(body).expect("master body utf-8");
    assert!(body.starts_with("#EXTM3U\n"), "body: {body}");
    assert!(body.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
    assert!(
        body.contains("#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\""),
        "master missing audio rendition: {body}"
    );
    assert!(body.contains("URI=\"audio.m3u8\""), "master missing audio URI: {body}");
    assert!(
        body.contains("#EXT-X-STREAM-INF"),
        "master missing variant stream: {body}"
    );
    assert!(
        body.contains("AUDIO=\"audio\""),
        "variant should reference audio group: {body}"
    );
    assert!(
        body.contains("\nplaylist.m3u8\n") || body.ends_with("\nplaylist.m3u8\n"),
        "variant URI line missing or wrong: {body}"
    );

    // --- Video media playlist: served by ensure_video's HlsServer. ---
    let (status, _, body) = get(&multi, "/hls/live/test/playlist.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(body.starts_with("#EXTM3U"));
    assert!(body.contains("#EXT-X-MAP:URI=\"init.mp4\""));

    // --- Audio media playlist: served by ensure_audio's HlsServer. ---
    let (status, _, body) = get(&multi, "/hls/live/test/audio.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(body.starts_with("#EXTM3U"), "audio playlist body: {body}");
    assert!(
        body.contains("#EXT-X-MAP:URI=\"audio-init.mp4\""),
        "audio playlist should reference audio-init.mp4: {body}"
    );
    // The audio playlist must reference its own prefixed chunks, not
    // the video chunks.
    assert!(
        body.contains("audio-part-") || body.contains("audio-seg-"),
        "audio playlist body should reference audio-prefixed chunks: {body}"
    );

    // --- Init segments served per rendition. ---
    let (status, ct, body) = get(&multi, "/hls/live/test/init.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "video/mp4");
    assert_eq!(body, b"video-init");

    let (status, ct, body) = get(&multi, "/hls/live/test/audio-init.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "video/mp4");
    assert_eq!(body, b"audio-init");
}

#[tokio::test]
async fn master_playlist_omits_audio_when_only_video_has_published() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());

    let video = multi.ensure_video("live/video-only");
    video.push_init(Bytes::from_static(b"video-init")).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/video-only/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        !body.contains("#EXT-X-MEDIA:"),
        "master should not declare audio rendition when audio is absent: {body}"
    );
    assert!(
        !body.contains("AUDIO=\""),
        "variant should not reference an audio group when audio is absent: {body}"
    );
    assert!(body.contains("#EXT-X-STREAM-INF"));

    // Audio routes 404 when audio rendition does not exist.
    let (status, _, _) = get(&multi, "/hls/live/video-only/audio.m3u8").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(&multi, "/hls/live/video-only/audio-init.mp4").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn master_playlist_returns_404_for_unknown_broadcast() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    let (status, _, _) = get(&multi, "/hls/live/ghost/master.m3u8").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
