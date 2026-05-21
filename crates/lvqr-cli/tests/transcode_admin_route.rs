//! End-to-end test for `GET /api/v1/transcode/ladders` (read-only ladder
//! introspection).
//!
//! Unlike `transcode_ladder_e2e.rs`, this test asserts only the
//! introspection surface, which reflects the operator-configured ladder
//! (`config.transcode_renditions`) regardless of whether GStreamer can
//! actually encode. The factories opt out when their elements are missing,
//! but the configured ladder and `enabled` flag are deterministic, so no
//! GStreamer-element probe / skip guard is needed here. Gated on the same
//! `transcode` + `rtmp` features as its sibling so CI's Feature-transcode
//! lane runs it.

#![cfg(all(feature = "transcode", feature = "rtmp"))]

use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with, http_post_json};
use lvqr_test_utils::{TestServer, TestServerConfig};
use lvqr_transcode::RenditionSpec;

/// Helper: the set of rendition names currently in the ladder per the admin
/// introspection route.
async fn ladder_names(admin_addr: std::net::SocketAddr) -> Vec<String> {
    let resp = http_get_with(admin_addr, "/api/v1/transcode/ladders", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    body["renditions"]
        .as_array()
        .expect("renditions array")
        .iter()
        .map(|r| r["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcode_route_reports_configured_ladder() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    let ladder = RenditionSpec::default_ladder();
    let server = TestServer::start(TestServerConfig::default().with_transcode_ladder(ladder.clone()))
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();

    let resp = http_get_with(admin_addr, "/api/v1/transcode/ladders", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "status={} body={}", resp.status, resp.body_text());

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["enabled"], true, "a configured ladder must report enabled");
    assert_eq!(body["encoder"], "software", "default encoder backend is software");

    let renditions = body["renditions"].as_array().expect("renditions is array");
    assert_eq!(
        renditions.len(),
        ladder.len(),
        "every configured rendition must surface; got {renditions:?}"
    );
    // The configured ladder names must all be present and carry resolution.
    for spec in &ladder {
        let found = renditions
            .iter()
            .find(|r| r["name"] == spec.name)
            .unwrap_or_else(|| panic!("rendition {} missing from {renditions:?}", spec.name));
        assert_eq!(found["width"], spec.width);
        assert_eq!(found["height"], spec.height);
        assert_eq!(found["video_bitrate_kbps"], spec.video_bitrate_kbps);
    }

    // `active` is present and is an array (empty until source fragments are
    // transcoded; that real-encode path is covered by transcode_ladder_e2e).
    assert!(body["active"].is_array(), "active must be an array; got {body}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transcode_route_add_and_remove_rendition_at_runtime() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // A startup ladder installs the runner, which is what enables runtime
    // mutation. No GStreamer elements are needed: the mutation is a
    // config-level ladder edit reflected by the introspection route.
    let server = TestServer::start(TestServerConfig::default().with_transcode_ladder(RenditionSpec::default_ladder()))
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();

    let before = ladder_names(admin_addr).await;
    assert!(
        !before.contains(&"360p".to_string()),
        "360p not present at start: {before:?}"
    );

    // Add a new rendition.
    let body = serde_json::to_vec(&serde_json::json!({
        "name": "360p", "width": 640, "height": 360, "video_bitrate_kbps": 800, "audio_bitrate_kbps": 96
    }))
    .unwrap();
    let resp = http_post_json(admin_addr, "/api/v1/transcode/ladders", None, &body).await;
    assert_eq!(resp.status, 201, "add status={} body={}", resp.status, resp.body_text());
    assert!(
        ladder_names(admin_addr).await.contains(&"360p".to_string()),
        "360p present after add"
    );

    // Adding it again conflicts.
    let dup = http_post_json(admin_addr, "/api/v1/transcode/ladders", None, &body).await;
    assert_eq!(dup.status, 409, "duplicate add must 409; got {}", dup.status);

    // Remove it.
    let del = http_delete(admin_addr, "/api/v1/transcode/ladders/360p", None).await;
    assert_eq!(del.status, 200, "remove status={} body={}", del.status, del.body_text());
    assert!(
        !ladder_names(admin_addr).await.contains(&"360p".to_string()),
        "360p gone after remove"
    );

    // Removing a nonexistent rendition 404s.
    let missing = http_delete(admin_addr, "/api/v1/transcode/ladders/999p", None).await;
    assert_eq!(
        missing.status, 404,
        "removing absent rendition must 404; got {}",
        missing.status
    );

    server.shutdown().await.expect("shutdown");
}
