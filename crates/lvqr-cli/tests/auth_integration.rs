//! Admin HTTP integration tests for the auth pipeline.
//!
//! Closes the audit finding from `tracking/AUDIT-READINESS-2026-04-13.md`
//! that called out: "JWT provider is wired into the CLI but has no
//! integration test. The unit tests in `lvqr-auth` exercise the provider
//! in isolation. No test verifies that `lvqr-cli serve --jwt-secret foo`
//! actually validates a real JWT end-to-end."
//!
//! Every test here uses [`lvqr_test_utils::TestServer`], which is the
//! real production `lvqr_cli::start` path, just with ephemeral loopback
//! ports. The assertions go over real TCP sockets against real axum
//! routes; no `tower::ServiceExt::oneshot` shortcuts, no mocks. If the
//! admin auth middleware regresses in any way, these tests go red.
//!
//! The test binary also gives the admin HTTP layer its first real
//! integration coverage: prior to this session, admin routes were
//! tested only at the unit level via `axum::Router::oneshot` inside
//! `lvqr-admin/src/routes.rs`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{EncodingKey, Header, encode};
use lvqr_auth::{
    AuthScope, JwtAuthConfig, JwtAuthProvider, JwtClaims, SharedAuth, StaticAuthConfig, StaticAuthProvider,
};
use lvqr_test_utils::{TestServer, TestServerConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(5);

// =====================================================================
// Minimal HTTP/1.1 client: just enough to drive GET requests and parse
// a status line plus body off the admin listener. Hand-rolled to avoid
// pulling reqwest into lvqr-cli's dev-deps; the admin API surface is
// small and the parser only needs to handle single-response conns.
// =====================================================================

struct HttpResponse {
    status: u16,
    body: String,
}

async fn http_get(addr: std::net::SocketAddr, path: &str, bearer: Option<&str>) -> HttpResponse {
    let mut stream = tokio::time::timeout(TIMEOUT, TcpStream::connect(addr))
        .await
        .expect("admin connect timed out")
        .expect("admin connect failed");
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n",
        path = path,
        host = addr,
    );
    if let Some(token) = bearer {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await.expect("http write");

    let mut raw = Vec::with_capacity(4096);
    let _ = tokio::time::timeout(TIMEOUT, stream.read_to_end(&mut raw))
        .await
        .expect("http read timed out")
        .expect("http read");

    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let first_line = head.lines().next().unwrap_or_default();
    let status: u16 = first_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("could not parse status line: {first_line:?}"));

    // The admin router serves responses with `Content-Length`, so the
    // body after the first blank line is the full payload. For
    // chunked responses (which admin does not emit) we would need a
    // parser; asserting Content-Length presence here would be noise.
    HttpResponse {
        status,
        body: body.to_string(),
    }
}

// =====================================================================
// JWT minting helper. Mirrors the production `JwtClaims` shape so the
// server's decoder accepts the token. Using the same type guarantees
// the test cannot drift from the production claim shape silently.
// =====================================================================

fn mint_jwt(secret: &str, scope: AuthScope, expires_in_secs: i64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let exp = (now + expires_in_secs).max(0) as usize;
    let claims = JwtClaims {
        sub: "integration-test".into(),
        exp,
        scope,
        iss: None,
        aud: None,
        broadcast: None,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("failed to encode test JWT")
}

// =====================================================================
// Test cases
// =====================================================================

/// Sanity: open-access TestServer serves /healthz and every admin
/// endpoint without a token, and the JSON body decodes to the
/// documented stats shape.
#[tokio::test]
async fn open_access_serves_admin_routes_without_auth() {
    let server = TestServer::start(TestServerConfig::new())
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    let healthz = http_get(addr, "/healthz", None).await;
    assert_eq!(healthz.status, 200);
    assert!(healthz.body.contains("ok"), "healthz body: {:?}", healthz.body);

    let stats = http_get(addr, "/api/v1/stats", None).await;
    assert_eq!(stats.status, 200, "stats body: {:?}", stats.body);
    let parsed: serde_json::Value = serde_json::from_str(&stats.body).expect("stats body is not JSON");
    // Every RelayStats field must be present as a number.
    for field in [
        "publishers",
        "tracks",
        "subscribers",
        "bytes_received",
        "bytes_sent",
        "uptime_secs",
    ] {
        assert!(
            parsed[field].is_number(),
            "expected stats.{field} to be a number, got {:?}",
            parsed[field]
        );
    }

    server.shutdown().await.expect("shutdown");
}

/// Static-token provider: missing bearer is 401, wrong bearer is 401,
/// correct bearer is 200. Healthz is always open regardless of auth
/// because Prometheus/k8s probes must reach it unauthenticated.
#[tokio::test]
async fn static_admin_token_gates_api_but_not_healthz() {
    let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
        admin_token: Some("secret-xyz".into()),
        publish_key: None,
        subscribe_token: None,
    }));
    let server = TestServer::start(TestServerConfig::new().with_auth(auth))
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    assert_eq!(http_get(addr, "/healthz", None).await.status, 200);

    let missing = http_get(addr, "/api/v1/stats", None).await;
    assert_eq!(missing.status, 401, "missing bearer should be 401");

    let wrong = http_get(addr, "/api/v1/stats", Some("not-the-token")).await;
    assert_eq!(wrong.status, 401, "wrong bearer should be 401");

    let ok = http_get(addr, "/api/v1/stats", Some("secret-xyz")).await;
    assert_eq!(ok.status, 200, "correct bearer should be 200: {:?}", ok.body);

    server.shutdown().await.expect("shutdown");
}

/// JWT provider happy path: a freshly minted HS256 token with
/// `scope=Admin` and a future `exp` is accepted by the admin
/// middleware, and the /api/v1/stats endpoint returns a decodable
/// JSON body. This is the specific assertion the audit called out.
#[tokio::test]
async fn jwt_provider_accepts_admin_scoped_token() {
    let secret = "hs256-test-secret-please-change";
    let provider = JwtAuthProvider::new(JwtAuthConfig {
        secret: secret.to_string(),
        issuer: None,
        audience: None,
    })
    .expect("JwtAuthProvider::new");
    let server = TestServer::start(TestServerConfig::new().with_auth(Arc::new(provider)))
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    let token = mint_jwt(secret, AuthScope::Admin, 3600);
    let resp = http_get(addr, "/api/v1/stats", Some(&token)).await;
    assert_eq!(resp.status, 200, "body: {:?}", resp.body);

    // Body must still parse as the documented stats shape even under
    // JWT gating; the middleware returns early on deny but defers to
    // the handler on allow.
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).expect("stats body is JSON");
    assert!(parsed["publishers"].is_number());

    server.shutdown().await.expect("shutdown");
}

/// JWT provider negative path: a token signed with a different secret
/// decodes to a decoder error and the middleware returns 401.
#[tokio::test]
async fn jwt_provider_rejects_wrong_secret() {
    let server_secret = "correct-secret";
    let attacker_secret = "attacker-secret";
    let provider = JwtAuthProvider::new(JwtAuthConfig {
        secret: server_secret.to_string(),
        issuer: None,
        audience: None,
    })
    .expect("JwtAuthProvider::new");
    let server = TestServer::start(TestServerConfig::new().with_auth(Arc::new(provider)))
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    let bad = mint_jwt(attacker_secret, AuthScope::Admin, 3600);
    let resp = http_get(addr, "/api/v1/stats", Some(&bad)).await;
    assert_eq!(resp.status, 401, "wrong-secret JWT must be rejected");

    server.shutdown().await.expect("shutdown");
}

/// JWT provider scope check: a token with `scope=Subscribe` must not
/// be accepted for an Admin context. This is a regression guard
/// against anyone accidentally making `AuthScope::includes` too
/// permissive in `crates/lvqr-auth/src/provider.rs`.
#[tokio::test]
async fn jwt_provider_rejects_insufficient_scope() {
    let secret = "scope-test-secret";
    let provider = JwtAuthProvider::new(JwtAuthConfig {
        secret: secret.to_string(),
        issuer: None,
        audience: None,
    })
    .expect("JwtAuthProvider::new");
    let server = TestServer::start(TestServerConfig::new().with_auth(Arc::new(provider)))
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    let subscribe_token = mint_jwt(secret, AuthScope::Subscribe, 3600);
    let resp = http_get(addr, "/api/v1/stats", Some(&subscribe_token)).await;
    assert_eq!(
        resp.status, 401,
        "Subscribe-scoped JWT must not satisfy an Admin context"
    );

    server.shutdown().await.expect("shutdown");
}

/// JWT provider expiration check: a token with `exp` in the past is
/// rejected by `jsonwebtoken`'s validation and surfaced as 401 by the
/// middleware.
#[tokio::test]
async fn jwt_provider_rejects_expired_token() {
    let secret = "expiration-test-secret";
    let provider = JwtAuthProvider::new(JwtAuthConfig {
        secret: secret.to_string(),
        issuer: None,
        audience: None,
    })
    .expect("JwtAuthProvider::new");
    let server = TestServer::start(TestServerConfig::new().with_auth(Arc::new(provider)))
        .await
        .expect("TestServer::start");
    let addr = server.admin_addr();

    // exp = now - 3600 lands ~1h in the past. jsonwebtoken's default
    // validation has a 60s leeway which this clears by a wide margin.
    let expired = mint_jwt(secret, AuthScope::Admin, -3600);
    let resp = http_get(addr, "/api/v1/stats", Some(&expired)).await;
    assert_eq!(resp.status, 401, "expired JWT must be rejected");

    server.shutdown().await.expect("shutdown");
}
