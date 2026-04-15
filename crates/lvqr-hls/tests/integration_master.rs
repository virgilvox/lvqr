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
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use lvqr_cmaf::{
    CmafChunk, CmafChunkKind, HevcInitParams, OpusInitParams, VideoInitParams, write_avc_init_segment,
    write_hevc_init_segment, write_opus_init_segment,
};
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
    let audio = multi.ensure_audio("live/test", 48_000);
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
async fn media_playlists_carry_sibling_rendition_reports() {
    // Video + audio both publishing. Each media playlist must
    // declare an `#EXT-X-RENDITION-REPORT` pointing at the sibling
    // with that sibling's current (LAST-MSN, LAST-PART). Session
    // 31 addition on top of the session-13 multi-rendition router.
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());

    // Video: one segment chunk at sequence 0.
    let video = multi.ensure_video("live/rr");
    video.push_init(Bytes::from_static(b"video-init")).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .unwrap();

    // Audio: two partial chunks in the open segment so LAST-PART
    // resolves to a distinct non-zero index that cannot be faked.
    let audio = multi.ensure_audio("live/rr", 48_000);
    audio.push_init(Bytes::from_static(b"audio-init")).await;
    audio
        .push_chunk_bytes(
            &audio_chunk(0, 96_000, CmafChunkKind::Segment),
            Bytes::from_static(b"audio-chunk-0"),
        )
        .await
        .unwrap();
    audio
        .push_chunk_bytes(
            &audio_chunk(96_000, 96_000, CmafChunkKind::Partial),
            Bytes::from_static(b"audio-chunk-1"),
        )
        .await
        .unwrap();

    // Video playlist: report points at audio.m3u8 at the audio
    // open segment (msn 0) with LAST-PART=1 (two partials, indices
    // 0 and 1 in the open segment).
    let (status, _, body) = get(&multi, "/hls/live/rr/playlist.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("#EXT-X-RENDITION-REPORT:URI=\"audio.m3u8\",LAST-MSN=0,LAST-PART=1"),
        "video playlist should report audio sibling at (0, 1); got:\n{body}"
    );

    // Audio playlist: report points at playlist.m3u8 at the video
    // open segment (next msn is 1 because the one segment-kind
    // push put a pending part in the open segment, so LAST-MSN
    // still reads 1, LAST-PART=0).
    let (status, _, body) = get(&multi, "/hls/live/rr/audio.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("#EXT-X-RENDITION-REPORT:URI=\"playlist.m3u8\""),
        "audio playlist should report video sibling; got:\n{body}"
    );
}

#[tokio::test]
async fn media_playlist_skips_rendition_report_when_sibling_absent() {
    // Video-only broadcast: the video media playlist must NOT
    // emit a rendition-report line because there is no sibling to
    // report. Guards against regressing to the video-only path
    // where `build_sibling_reports` would otherwise emit a dangling
    // report for an audio.m3u8 route that does not resolve.
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    let video = multi.ensure_video("live/video-rr-only");
    video.push_init(Bytes::from_static(b"video-init")).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/video-rr-only/playlist.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        !body.contains("#EXT-X-RENDITION-REPORT"),
        "video-only playlist should not emit a rendition report: {body}"
    );
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

// --- Session 27: codec string parsed from the pushed init segment
// flows into the master playlist ---

/// H.264 Baseline @L3.1 SPS + PPS pinned against the same fixture
/// used by `lvqr-cmaf::init::tests`. Pushing a real AVC init
/// segment through `HlsServer::push_init` exercises the
/// `detect_video_codec_string` call site from end to end.
const AVC_SPS: &[u8] = &[
    0x67, 0x42, 0x00, 0x1F, 0xD9, 0x40, 0x50, 0x04, 0xFB, 0x01, 0x10, 0x00, 0x00, 0x03, 0x00, 0x10, 0x00, 0x00, 0x03,
    0x03, 0xC0, 0xF1, 0x83, 0x2A,
];
const AVC_PPS: &[u8] = &[0x68, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];

/// Real x265 HEVC Main 3.0 VPS + SPS + PPS, same corpus pin as
/// `lvqr-cmaf::init::tests` and `lvqr-whip::bridge::tests`.
const HEVC_VPS: &[u8] = &[
    0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03,
    0x00, 0x3c, 0x95, 0x94, 0x09,
];
const HEVC_SPS: &[u8] = &[
    0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0,
    0x0a, 0x08, 0x0f, 0x16, 0x59, 0x59, 0x52, 0x93, 0x0b, 0xc0, 0x5a, 0x02, 0x00, 0x00, 0x03, 0x00, 0x02, 0x00, 0x00,
    0x03, 0x00, 0x3c, 0x10,
];
const HEVC_PPS: &[u8] = &[0x44, 0x01, 0xc0, 0x73, 0xc1, 0x89];

fn hevc_sps_x265_info() -> lvqr_codec::hevc::HevcSps {
    lvqr_codec::hevc::HevcSps {
        general_profile_space: 0,
        general_tier_flag: false,
        general_profile_idc: 1,
        general_profile_compatibility_flags: 0x60000000,
        general_level_idc: 60,
        chroma_format_idc: 1,
        pic_width_in_luma_samples: 320,
        pic_height_in_luma_samples: 240,
    }
}

#[tokio::test]
async fn master_playlist_reports_avc1_codec_for_avc_init() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    let video = multi.ensure_video("live/avc");
    let mut buf = BytesMut::new();
    write_avc_init_segment(
        &mut buf,
        &VideoInitParams {
            sps: AVC_SPS.to_vec(),
            pps: AVC_PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .expect("write avc init");
    video.push_init(buf.freeze()).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/avc/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("CODECS=\"avc1.42001F\""),
        "master should advertise avc1.42001F for this fixture: {body}"
    );
}

#[tokio::test]
async fn master_playlist_reports_hvc1_codec_for_hevc_init() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    let video = multi.ensure_video("live/hevc");
    let mut buf = BytesMut::new();
    write_hevc_init_segment(
        &mut buf,
        &HevcInitParams {
            vps: HEVC_VPS.to_vec(),
            sps: HEVC_SPS.to_vec(),
            pps: HEVC_PPS.to_vec(),
            sps_info: hevc_sps_x265_info(),
            timescale: 90_000,
        },
    )
    .expect("write hevc init");
    video.push_init(buf.freeze()).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"video-chunk-0"),
        )
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/hevc/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("CODECS=\"hvc1.1.6.L60.B0\""),
        "master should advertise hvc1 for this HEVC fixture: {body}"
    );
    assert!(
        !body.contains("avc1."),
        "master should not fall back to avc1 when init is HEVC: {body}"
    );
}

#[tokio::test]
async fn master_playlist_reports_opus_codec_when_audio_rendition_has_opus_init() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    // Video side: AVC, as a baseline.
    let video = multi.ensure_video("live/opus-audio");
    let mut video_buf = BytesMut::new();
    write_avc_init_segment(
        &mut video_buf,
        &VideoInitParams {
            sps: AVC_SPS.to_vec(),
            pps: AVC_PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .unwrap();
    video.push_init(video_buf.freeze()).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();

    // Audio side: push a real Opus init segment.
    let audio = multi.ensure_audio("live/opus-audio", 48_000);
    let mut audio_buf = BytesMut::new();
    write_opus_init_segment(
        &mut audio_buf,
        &OpusInitParams {
            channel_count: 2,
            pre_skip: 0,
            input_sample_rate: 48_000,
            timescale: 48_000,
        },
    )
    .unwrap();
    audio.push_init(audio_buf.freeze()).await;
    audio
        .push_chunk_bytes(&audio_chunk(0, 960, CmafChunkKind::Segment), Bytes::from_static(b"a"))
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/opus-audio/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("CODECS=\"avc1.42001F,opus\""),
        "master should advertise avc1 + opus when audio init is Opus: {body}"
    );
    assert!(
        !body.contains("mp4a.40.2"),
        "master should not fall back to mp4a when audio init is Opus: {body}"
    );
}

#[tokio::test]
async fn master_playlist_appends_aac_when_audio_rendition_exists() {
    let multi = MultiHlsServer::new(PlaylistBuilderConfig::default());
    let video = multi.ensure_video("live/avc-aac");
    let mut buf = BytesMut::new();
    write_avc_init_segment(
        &mut buf,
        &VideoInitParams {
            sps: AVC_SPS.to_vec(),
            pps: AVC_PPS.to_vec(),
            width: 1280,
            height: 720,
            timescale: 90_000,
        },
    )
    .unwrap();
    video.push_init(buf.freeze()).await;
    video
        .push_chunk_bytes(
            &video_chunk(0, 180_000, CmafChunkKind::Segment),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();
    let audio = multi.ensure_audio("live/avc-aac", 48_000);
    audio.push_init(Bytes::from_static(b"audio-init")).await;
    audio
        .push_chunk_bytes(
            &audio_chunk(0, 96_000, CmafChunkKind::Segment),
            Bytes::from_static(b"a"),
        )
        .await
        .unwrap();

    let (status, _, body) = get(&multi, "/hls/live/avc-aac/master.m3u8").await;
    assert_eq!(status, StatusCode::OK);
    let body = String::from_utf8(body).unwrap();
    assert!(
        body.contains("CODECS=\"avc1.42001F,mp4a.40.2\""),
        "master should combine detected video codec with AAC for the audio rendition: {body}"
    );
}
