//! End-to-end exercise of the `lvqr-dash` axum routers.
//!
//! Drives real HTTP requests through [`DashServer::router`] and
//! [`MultiDashServer::router`] via `tower::ServiceExt::oneshot`,
//! without binding a TCP listener. This is the "integration" slot
//! of the 5-artifact contract for the `server` module and the
//! "e2e" slot of the crate-level contract (the whole router
//! surface is exercised end-to-end, just over the axum service
//! trait rather than a loopback socket). The TCP-loopback
//! complement lives in `lvqr-cli/tests/rtmp_dash_e2e.rs`.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use http_body_util::BodyExt;
use lvqr_dash::{DashConfig, DashServer, MultiDashServer};
use tower::ServiceExt;

async fn get(router: axum::Router, path: &str) -> (StatusCode, String, Vec<u8>) {
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
async fn single_broadcast_av_round_trip_through_router() {
    let server = DashServer::new(DashConfig::default());
    server.push_video_init(Bytes::from_static(b"\x00video-init"));
    server.push_audio_init(Bytes::from_static(b"\x00audio-init"));
    server.push_video_segment(1, Bytes::from_static(b"v-seg-1"));
    server.push_video_segment(2, Bytes::from_static(b"v-seg-2"));
    server.push_audio_segment(1, Bytes::from_static(b"a-seg-1"));
    server.push_audio_segment(2, Bytes::from_static(b"a-seg-2"));

    let (status, ct, body) = get(server.router(), "/manifest.mpd").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/dash+xml");
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("<MPD"));
    assert!(text.contains("<AdaptationSet id=\"0\""));
    assert!(text.contains("<AdaptationSet id=\"1\""));
    assert!(text.contains("seg-video-$Number$.m4s"));
    assert!(text.contains("seg-audio-$Number$.m4s"));

    let (status, ct, body) = get(server.router(), "/init-video.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "video/mp4");
    assert_eq!(body, b"\x00video-init");

    let (status, ct, body) = get(server.router(), "/init-audio.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "audio/mp4");
    assert_eq!(body, b"\x00audio-init");

    let (status, ct, body) = get(server.router(), "/seg-video-1.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "video/iso.segment");
    assert_eq!(body, b"v-seg-1");

    let (status, _, body) = get(server.router(), "/seg-audio-2.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"a-seg-2");
}

#[tokio::test]
async fn manifest_404_before_any_push() {
    let server = DashServer::new(DashConfig::default());
    let (status, _, _) = get(server.router(), "/manifest.mpd").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_segment_returns_404() {
    let server = DashServer::new(DashConfig::default());
    server.push_video_init(Bytes::from_static(b"\x00init"));
    server.push_video_segment(1, Bytes::from_static(b"only-1"));
    let (status, _, _) = get(server.router(), "/seg-video-99.m4s").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(server.router(), "/garbage.m4s").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn multi_broadcast_router_dispatches_per_broadcast() {
    let multi = MultiDashServer::new(DashConfig::default());
    let a = multi.ensure("live/one");
    let b = multi.ensure("live/two");
    a.push_video_init(Bytes::from_static(b"\x00a-init"));
    b.push_video_init(Bytes::from_static(b"\x00b-init"));
    a.push_video_segment(1, Bytes::from_static(b"a-seg-1"));
    b.push_video_segment(1, Bytes::from_static(b"b-seg-1"));

    let (status, ct, body) = get(multi.router(), "/dash/live/one/manifest.mpd").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct, "application/dash+xml");
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("<MPD"));

    let (status, _, body) = get(multi.router(), "/dash/live/one/init-video.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"\x00a-init");

    let (status, _, body) = get(multi.router(), "/dash/live/two/init-video.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"\x00b-init");

    let (status, _, body) = get(multi.router(), "/dash/live/one/seg-video-1.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"a-seg-1");
    let (status, _, body) = get(multi.router(), "/dash/live/two/seg-video-1.m4s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"b-seg-1");

    // Unknown broadcast is a 404, not an empty 200.
    let (status, _, _) = get(multi.router(), "/dash/live/ghost/manifest.mpd").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
