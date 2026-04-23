//! Live HLS + DASH signed-URL tests (session 128).
//!
//! Session 124 added HMAC-signed URL support on the `/playback/*`
//! route tree. Session 128 extends the primitive to live HLS + DASH
//! via a broadcast-scoped signature whose input is
//! `"<scheme>:<broadcast>?exp=<exp>"`. A single signed URL grants
//! access to every URL under that broadcast's live tree (master
//! playlist, media playlist, init segments, numbered / partial
//! media segments) because LL-HLS playlists reference segment URIs
//! that roll over every 200 ms; a path-bound signature would be
//! impractical.
//!
//! This test file drives the full stack: boots a TestServer with a
//! subscribe token (no bearer on the test client) + an HMAC secret,
//! mints signed URLs via `lvqr_cli::sign_live_url`, and asserts:
//!
//! * A valid signed URL on HLS returns not-401 without a bearer.
//! * A valid signed URL on DASH returns not-401 without a bearer.
//! * A tampered sig returns 403.
//! * An expired URL returns 403.
//! * An HLS-minted sig on a DASH URL returns 403 (scheme binding).
//! * A sig for broadcast A presented on broadcast B returns 403.
//! * No sig + no bearer returns 401 (fall-through to the normal
//!   subscribe-token gate; the short-circuit is additive, not
//!   replacing).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use lvqr_auth::{SharedAuth, StaticAuthConfig, StaticAuthProvider};
use lvqr_cli::{LiveScheme, sign_live_url};
use lvqr_test_utils::http::http_get_status;
use lvqr_test_utils::{TestServer, TestServerConfig};

const SECRET: &[u8] = b"live-signed-url-test-secret";

fn subscribe_auth(token: &str) -> SharedAuth {
    Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: None,
        publish_key: None,
        subscribe_token: Some(token.to_string()),
    }))
}

fn unix_exp_in(seconds: u64) -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + seconds
}

async fn boot_server() -> TestServer {
    TestServer::start(
        TestServerConfig::new()
            .with_dash()
            .with_auth(subscribe_auth("real-subscriber-token"))
            .with_hmac_playback_secret(String::from_utf8_lossy(SECRET).into_owned()),
    )
    .await
    .expect("TestServer::start")
}

#[tokio::test]
async fn hls_signed_url_grants_access_without_bearer() {
    let server = boot_server().await;
    let addr = server.hls_addr();

    let exp = unix_exp_in(600);
    let suffix = sign_live_url(SECRET, LiveScheme::Hls, "live/demo", exp);
    let path = format!("/hls/live/demo/playlist.m3u8?{suffix}");

    let status = http_get_status(addr, &path).await;
    assert_ne!(
        status, 401,
        "valid HLS signed URL should short-circuit the subscribe gate; got {status}"
    );
    assert_ne!(
        status, 403,
        "valid HLS signed URL should not be forbidden; got {status}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn dash_signed_url_grants_access_without_bearer() {
    let server = boot_server().await;
    let addr = server.dash_addr();

    let exp = unix_exp_in(600);
    let suffix = sign_live_url(SECRET, LiveScheme::Dash, "live/demo", exp);
    let path = format!("/dash/live/demo/manifest.mpd?{suffix}");

    let status = http_get_status(addr, &path).await;
    assert_ne!(
        status, 401,
        "valid DASH signed URL should short-circuit the subscribe gate; got {status}"
    );
    assert_ne!(
        status, 403,
        "valid DASH signed URL should not be forbidden; got {status}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn tampered_hls_sig_returns_403() {
    let server = boot_server().await;
    let addr = server.hls_addr();

    let exp = unix_exp_in(600);
    let mut suffix = sign_live_url(SECRET, LiveScheme::Hls, "live/demo", exp);
    // Flip the last byte of the sig.
    let last = suffix.pop().expect("non-empty suffix");
    let flipped = if last == 'A' { 'B' } else { 'A' };
    suffix.push(flipped);
    let path = format!("/hls/live/demo/playlist.m3u8?{suffix}");

    let status = http_get_status(addr, &path).await;
    assert_eq!(status, 403, "tampered HLS sig must return 403; got {status}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn expired_hls_url_returns_403() {
    let server = boot_server().await;
    let addr = server.hls_addr();

    // Exp in the past. sign_live_url does not enforce a floor on
    // the expiry so an operator minting an already-expired URL
    // produces a verifiable signature with a stale exp, and the
    // verifier returns 403 "signed URL expired".
    let exp = 1_000_000;
    let suffix = sign_live_url(SECRET, LiveScheme::Hls, "live/demo", exp);
    let path = format!("/hls/live/demo/playlist.m3u8?{suffix}");

    let status = http_get_status(addr, &path).await;
    assert_eq!(status, 403, "expired HLS URL must return 403; got {status}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn hls_sig_rejected_on_dash_route() {
    let server = boot_server().await;
    let dash_addr = server.dash_addr();

    let exp = unix_exp_in(600);
    // Mint for HLS scheme, then present the sig on the DASH route.
    // Each scheme's signed input bakes in the scheme tag, so the
    // HMAC produced for "hls:live/demo?exp=..." will not verify
    // when the DASH middleware reconstructs "dash:live/demo?exp=...".
    let suffix = sign_live_url(SECRET, LiveScheme::Hls, "live/demo", exp);
    let path = format!("/dash/live/demo/manifest.mpd?{suffix}");

    let status = http_get_status(dash_addr, &path).await;
    assert_eq!(status, 403, "HLS-minted sig on DASH must return 403; got {status}");

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn hls_sig_rejected_on_wrong_broadcast() {
    let server = boot_server().await;
    let addr = server.hls_addr();

    let exp = unix_exp_in(600);
    // Sign for broadcast A, present on broadcast B.
    let suffix = sign_live_url(SECRET, LiveScheme::Hls, "live/broadcast-a", exp);
    let path = format!("/hls/live/broadcast-b/playlist.m3u8?{suffix}");

    let status = http_get_status(addr, &path).await;
    assert_eq!(
        status, 403,
        "broadcast-A sig on broadcast-B must return 403; got {status}"
    );

    server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn missing_sig_falls_through_to_subscribe_gate() {
    let server = boot_server().await;
    let addr = server.hls_addr();

    // No sig + no bearer -> 401 from the subscribe-token gate.
    let status = http_get_status(addr, "/hls/live/demo/playlist.m3u8").await;
    assert_eq!(
        status, 401,
        "no sig + no bearer should fall through to the subscribe-token gate and 401; got {status}"
    );

    server.shutdown().await.expect("shutdown");
}
