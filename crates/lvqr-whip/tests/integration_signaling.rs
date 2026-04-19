//! Integration tests for the WHIP signaling router.
//!
//! Slot 3 of the 5-artifact contract. Drives a real `axum::Router`
//! via `tower::ServiceExt::oneshot` with a stub [`SdpAnswerer`]
//! that returns a canned answer and a stub [`SessionHandle`] that
//! records trickle calls. The e2e loopback test in
//! `tests/e2e_str0m_loopback.rs` exercises the real str0m
//! answerer; this file is deliberately lightweight and focuses on
//! the HTTP contract.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use lvqr_whip::{SdpAnswerer, SessionHandle, WhipError, WhipServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

// =====================================================================
// Stubs
// =====================================================================

struct StubAnswerer {
    trickle_count: Arc<AtomicUsize>,
}

impl StubAnswerer {
    fn new() -> (Self, Arc<AtomicUsize>) {
        let t = Arc::new(AtomicUsize::new(0));
        (
            Self {
                trickle_count: t.clone(),
            },
            t,
        )
    }
}

impl SdpAnswerer for StubAnswerer {
    fn create_session(&self, _broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhipError> {
        if offer.is_empty() {
            return Err(WhipError::MalformedOffer("empty".into()));
        }
        Ok((
            Box::new(CountingHandle {
                trickle_count: self.trickle_count.clone(),
            }),
            Bytes::from_static(b"v=0\r\nstub-answer\r\n"),
        ))
    }
}

struct CountingHandle {
    trickle_count: Arc<AtomicUsize>,
}

impl SessionHandle for CountingHandle {
    fn add_trickle(&self, _sdp_fragment: &[u8]) -> Result<(), WhipError> {
        self.trickle_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

// =====================================================================
// Helpers
// =====================================================================

async fn body_bytes(body: Body) -> Bytes {
    to_bytes(body, 16 * 1024).await.expect("collect body")
}

fn sdp_offer(broadcast: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/whip/{broadcast}"))
        .header(header::CONTENT_TYPE, "application/sdp")
        .body(Body::from("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"))
        .expect("build post")
}

// =====================================================================
// POST /whip/{broadcast}
// =====================================================================

#[tokio::test]
async fn post_offer_returns_created_with_location_and_answer() {
    let (answerer, _trickle) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server.clone());

    let response = router.oneshot(sdp_offer("live/test")).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);

    let location = response
        .headers()
        .get(header::LOCATION)
        .expect("Location header present")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with("/whip/live/test/"),
        "unexpected Location: {location}"
    );
    assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(), "application/sdp");

    let body = body_bytes(response.into_body()).await;
    assert!(body.as_ref().starts_with(b"v=0"), "response body is not an SDP answer");
    assert!(
        std::str::from_utf8(&body).unwrap().contains("stub-answer"),
        "answer body missing stub marker"
    );

    assert_eq!(server.session_count(), 1);
}

#[tokio::test]
async fn post_offer_without_content_type_returns_415() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .body(Body::from("v=0\r\n"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(server.session_count(), 0);
}

#[tokio::test]
async fn post_offer_with_wrong_content_type_returns_415() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server);

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn post_offer_accepts_content_type_with_parameters() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server);

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .header(header::CONTENT_TYPE, "application/sdp; charset=utf-8")
        .body(Body::from("v=0\r\n"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn post_offer_with_empty_body_returns_400() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .header(header::CONTENT_TYPE, "application/sdp")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.session_count(), 0);
}

// =====================================================================
// DELETE /whip/{broadcast}/{session_id}
// =====================================================================

#[tokio::test]
async fn delete_unknown_session_returns_404() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server);

    let request = Request::builder()
        .method("DELETE")
        .uri("/whip/live/test/bogus")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_lifecycle_post_then_delete() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));

    let post_resp = lvqr_whip::router_for(server.clone())
        .oneshot(sdp_offer("live/test"))
        .await
        .unwrap();
    assert_eq!(post_resp.status(), StatusCode::CREATED);
    let location = post_resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(server.session_count(), 1);

    let del_resp = lvqr_whip::router_for(server.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(location.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del_resp.status(), StatusCode::OK);
    assert_eq!(server.session_count(), 0);

    let second_del = lvqr_whip::router_for(server.clone())
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(location)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second_del.status(), StatusCode::NOT_FOUND);
}

// =====================================================================
// PATCH /whip/{broadcast}/{session_id}
// =====================================================================

#[tokio::test]
async fn patch_unknown_session_returns_404() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server);

    let request = Request::builder()
        .method("PATCH")
        .uri("/whip/live/test/bogus")
        .header(header::CONTENT_TYPE, "application/trickle-ice-sdpfrag")
        .body(Body::from("a=candidate:..."))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_existing_session_forwards_to_handle() {
    let (answerer, trickle) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));

    let post_resp = lvqr_whip::router_for(server.clone())
        .oneshot(sdp_offer("live/test"))
        .await
        .unwrap();
    let location = post_resp
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let patch_resp = lvqr_whip::router_for(server.clone())
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(location)
                .header(header::CONTENT_TYPE, "application/trickle-ice-sdpfrag")
                .body(Body::from("a=candidate:1 1 UDP 2113929471 192.0.2.1 56789 typ host"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(trickle.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unknown_method_returns_405() {
    let (answerer, _) = StubAnswerer::new();
    let server = WhipServer::new(Arc::new(answerer));
    let router = lvqr_whip::router_for(server);

    let request = Request::builder()
        .method("GET")
        .uri("/whip/live/test")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}

// =====================================================================
// Auth gate (Tier 4 item 4.8 session A)
// =====================================================================

/// Static provider that rejects unless the bearer token matches `want`.
/// Exercises the `extract_whip` -> `auth.check` -> 401 path without
/// pulling in a JWT provider's full claim surface.
struct GateAuth {
    want: &'static str,
}

impl lvqr_auth::AuthProvider for GateAuth {
    fn check(&self, ctx: &lvqr_auth::AuthContext) -> lvqr_auth::AuthDecision {
        let lvqr_auth::AuthContext::Publish { key, .. } = ctx else {
            return lvqr_auth::AuthDecision::deny("non-publish on WHIP");
        };
        if key == self.want {
            lvqr_auth::AuthDecision::Allow
        } else {
            lvqr_auth::AuthDecision::deny("wrong token")
        }
    }
}

#[tokio::test]
async fn post_offer_with_valid_bearer_returns_201() {
    let (answerer, _) = StubAnswerer::new();
    let auth: lvqr_auth::SharedAuth = Arc::new(GateAuth { want: "good" });
    let server = WhipServer::with_auth_provider(Arc::new(answerer), auth);

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::AUTHORIZATION, "Bearer good")
        .body(Body::from("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"))
        .unwrap();

    let response = lvqr_whip::router_for(server.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(server.session_count(), 1);
}

#[tokio::test]
async fn post_offer_missing_bearer_returns_401() {
    let (answerer, _) = StubAnswerer::new();
    let auth: lvqr_auth::SharedAuth = Arc::new(GateAuth { want: "good" });
    let server = WhipServer::with_auth_provider(Arc::new(answerer), auth);

    let response = lvqr_whip::router_for(server.clone())
        .oneshot(sdp_offer("live/test"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(server.session_count(), 0, "denied offer must not create a session");
}

#[tokio::test]
async fn post_offer_with_wrong_bearer_returns_401() {
    let (answerer, _) = StubAnswerer::new();
    let auth: lvqr_auth::SharedAuth = Arc::new(GateAuth { want: "good" });
    let server = WhipServer::with_auth_provider(Arc::new(answerer), auth);

    let request = Request::builder()
        .method("POST")
        .uri("/whip/live/test")
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::AUTHORIZATION, "Bearer bad")
        .body(Body::from("v=0\r\n"))
        .unwrap();

    let response = lvqr_whip::router_for(server.clone()).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(server.session_count(), 0);
}
