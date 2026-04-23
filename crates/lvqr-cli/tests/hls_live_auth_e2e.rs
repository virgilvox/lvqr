//! Live HLS + DASH subscribe-auth gate tests (session 112).
//!
//! Closes the audit finding from 2026-04-21 that
//! `crates/lvqr-hls/src/server.rs:7-9` defers auth to the CLI
//! composition root but the composition root never applied it.
//! Every request to `/hls/{broadcast}/...` and
//! `/dash/{broadcast}/...` on a TestServer configured with a
//! static subscribe token was world-readable before session 112;
//! these tests lock that behavior into the happy path.
//!
//! Coverage:
//!
//! 1. `authed_hls_rejects_missing_token` -- subscribe-auth
//!    deployment, GET `/hls/live/demo/playlist.m3u8` with no
//!    bearer -> 401.
//! 2. `authed_hls_accepts_bearer_header` -- same deployment with
//!    `Authorization: Bearer <token>` -> not 401 (200 or 404
//!    depending on whether the broadcast exists; the gate is
//!    what is under test, not the HLS handler's own logic).
//! 3. `authed_hls_accepts_query_token` -- `?token=<token>` falls
//!    back when the header is absent, matching the existing
//!    `/playback/*` pattern.
//! 4. `authed_hls_rejects_wrong_token` -- wrong bearer is 401.
//! 5. `authed_dash_rejects_missing_token` -- the same gate applies
//!    to DASH manifest + segment routes.
//! 6. `escape_hatch_disables_live_auth` -- TestServer configured
//!    with subscribe-auth AND `without_live_playback_auth()` lets
//!    the unauthed request through; the escape hatch is the
//!    contract for deployments that want open live playback with
//!    auth scoped to ingest + admin + DVR only.
//! 7. `noop_provider_never_gates` -- the default (no auth
//!    configured) TestServer serves live HLS without a bearer, so
//!    unauthenticated deployments see no behavior change from
//!    session 112.

use std::net::SocketAddr;
use std::sync::Arc;

use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_test_utils::http::{HttpGetOptions, http_get_status, http_get_with};
use lvqr_test_utils::{TestServer, TestServerConfig};

async fn status_with_bearer(addr: SocketAddr, path: &str, bearer: Option<&str>) -> u16 {
    match bearer {
        Some(token) => {
            http_get_with(addr, path, HttpGetOptions::with_bearer(token))
                .await
                .status
        }
        None => http_get_status(addr, path).await,
    }
}

fn static_subscribe_auth(token: &str) -> SharedAuth {
    Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: None,
        publish_key: None,
        subscribe_token: Some(token.to_string()),
    }))
}

#[tokio::test]
async fn authed_hls_rejects_missing_token() {
    let server = TestServer::start(TestServerConfig::new().with_auth(static_subscribe_auth("viewer-secret")))
        .await
        .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = status_with_bearer(addr, "/hls/live/demo/playlist.m3u8", None).await;
    assert_eq!(status, 401, "expected 401 without bearer; got {status}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn authed_hls_accepts_bearer_header() {
    let server = TestServer::start(TestServerConfig::new().with_auth(static_subscribe_auth("viewer-secret")))
        .await
        .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = status_with_bearer(addr, "/hls/live/demo/playlist.m3u8", Some("viewer-secret")).await;
    assert_ne!(status, 401, "auth gate should have allowed the bearer through");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn authed_hls_accepts_query_token() {
    let server = TestServer::start(TestServerConfig::new().with_auth(static_subscribe_auth("viewer-secret")))
        .await
        .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = http_get_status(addr, "/hls/live/demo/playlist.m3u8?token=viewer-secret").await;
    assert_ne!(status, 401, "query token should have allowed the request through");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn authed_hls_rejects_wrong_token() {
    let server = TestServer::start(TestServerConfig::new().with_auth(static_subscribe_auth("viewer-secret")))
        .await
        .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = status_with_bearer(addr, "/hls/live/demo/playlist.m3u8", Some("wrong-token")).await;
    assert_eq!(status, 401, "wrong bearer should be rejected; got {status}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn authed_dash_rejects_missing_token() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_dash()
            .with_auth(static_subscribe_auth("viewer-secret")),
    )
    .await
    .expect("TestServer::start");
    let addr = server.dash_addr();

    let status_no_bearer = status_with_bearer(addr, "/dash/live/demo/manifest.mpd", None).await;
    assert_eq!(
        status_no_bearer, 401,
        "expected 401 without bearer on DASH; got {status_no_bearer}"
    );

    let status_ok = status_with_bearer(addr, "/dash/live/demo/manifest.mpd", Some("viewer-secret")).await;
    assert_ne!(status_ok, 401, "DASH auth gate should have allowed the bearer through");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn escape_hatch_disables_live_auth() {
    let server = TestServer::start(
        TestServerConfig::new()
            .with_auth(static_subscribe_auth("viewer-secret"))
            .without_live_playback_auth(),
    )
    .await
    .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = status_with_bearer(addr, "/hls/live/demo/playlist.m3u8", None).await;
    assert_ne!(
        status, 401,
        "escape hatch should have disabled the live-playback auth gate; got {status}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn noop_provider_never_gates() {
    // Default TestServerConfig installs no auth provider, so the
    // server boots with NoopAuthProvider. The middleware still
    // runs but the provider always allows; unauthed deployments
    // see no behavior change from session 112.
    let server = TestServer::start(TestServerConfig::new())
        .await
        .expect("TestServer::start");
    let addr = server.hls_addr();

    let status = status_with_bearer(addr, "/hls/live/demo/playlist.m3u8", None).await;
    assert_ne!(status, 401, "noop provider must allow unauthed live HLS; got {status}");

    server.shutdown().await.expect("shutdown");
}
