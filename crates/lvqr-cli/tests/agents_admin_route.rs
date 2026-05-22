//! End-to-end test for `GET /api/v1/agents` (read-only agent introspection).
//!
//! The introspection surface reflects the operator-configured agent set
//! (`config.whisper_model`), which is independent of whether the whisper.cpp
//! model file actually loads -- the model is loaded lazily when an agent
//! attaches to a live audio track, not at server start. So this test
//! configures a placeholder model path, starts the server with no publisher,
//! and asserts the configured agent surfaces with `enabled: true` and an
//! empty `active` list. No real model file or GStreamer is required, so it
//! needs no env-gated skip. Gated on the `whisper` feature so it builds in
//! the same CI lane as `whisper_cli_e2e.rs`.

#![cfg(feature = "whisper")]

use lvqr_test_utils::http::{HttpGetOptions, http_delete, http_get_with, http_post_json};
use lvqr_test_utils::{TestServer, TestServerConfig};

/// Helper: the set of agent names currently running per the admin route.
async fn agent_names(admin_addr: std::net::SocketAddr) -> Vec<String> {
    let resp = http_get_with(admin_addr, "/api/v1/agents", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    body["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .map(|a| a["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_route_reports_configured_whisper_agent() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // A placeholder path: the model is only loaded when an agent attaches to
    // a live audio track, which never happens here (no publisher).
    let model_path = "/tmp/lvqr-test-nonexistent-ggml.bin";
    let server = TestServer::start(TestServerConfig::default().with_whisper_model(model_path))
        .await
        .expect("start TestServer with --whisper-model");
    let admin_addr = server.admin_addr();

    let resp = http_get_with(admin_addr, "/api/v1/agents", HttpGetOptions::default()).await;
    assert_eq!(resp.status, 200, "status={} body={}", resp.status, resp.body_text());

    let body: serde_json::Value = serde_json::from_slice(&resp.body).expect("valid JSON");
    assert_eq!(body["enabled"], true, "a configured whisper model must report enabled");

    let agents = body["agents"].as_array().expect("agents is array");
    assert_eq!(
        agents.len(),
        1,
        "exactly the whisper captions agent should be configured"
    );
    assert_eq!(agents[0]["name"], "captions");
    assert_eq!(agents[0]["kind"], "captions");
    assert_eq!(
        agents[0]["model"], model_path,
        "agent must surface its configured model path"
    );
    assert_eq!(agents[0]["window_ms"], 5000, "default whisper window is 5s");

    // No publisher -> no attachment.
    assert!(
        body["active"].as_array().expect("active is array").is_empty(),
        "no agent should be attached without a live broadcast; got {body}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_route_start_and_stop_at_runtime() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("lvqr=debug")
        .with_test_writer()
        .try_init();

    // No startup model: the runner still installs under the whisper feature,
    // so an agent can be started from zero. The model path is only loaded when
    // the agent attaches to a live audio track, which never happens here.
    let server = TestServer::start(TestServerConfig::default())
        .await
        .expect("start TestServer");
    let admin_addr = server.admin_addr();

    assert!(agent_names(admin_addr).await.is_empty(), "no agents at start");

    // Start the captions agent.
    let body = serde_json::to_vec(&serde_json::json!({
        "model": "/tmp/lvqr-test-nonexistent-ggml.bin", "window_ms": 3000
    }))
    .unwrap();
    let resp = http_post_json(admin_addr, "/api/v1/agents", None, &body).await;
    assert_eq!(
        resp.status,
        201,
        "start status={} body={}",
        resp.status,
        resp.body_text()
    );
    assert!(
        agent_names(admin_addr).await.contains(&"captions".to_string()),
        "captions running after start"
    );

    // Starting again conflicts.
    let dup = http_post_json(admin_addr, "/api/v1/agents", None, &body).await;
    assert_eq!(dup.status, 409, "duplicate start must 409; got {}", dup.status);

    // Stop it.
    let del = http_delete(admin_addr, "/api/v1/agents/captions", None).await;
    assert_eq!(del.status, 200, "stop status={} body={}", del.status, del.body_text());
    assert!(
        !agent_names(admin_addr).await.contains(&"captions".to_string()),
        "captions gone after stop"
    );

    // Stopping an absent agent 404s.
    let missing = http_delete(admin_addr, "/api/v1/agents/ghost", None).await;
    assert_eq!(
        missing.status, 404,
        "stopping absent agent must 404; got {}",
        missing.status
    );

    server.shutdown().await.expect("shutdown");
}
