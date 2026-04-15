//! Integration tests for the WHEP signaling router.
//!
//! This is the integration slot of the 5-artifact contract for
//! `lvqr-whep`. It drives a real `axum::Router` via
//! `tower::ServiceExt::oneshot` with a stub [`SdpAnswerer`] that
//! returns a canned answer body and a stub [`SessionHandle`] that
//! records every `add_trickle` / `on_raw_sample` call for later
//! assertion. The tests cover:
//!
//! * Content-Type validation on POST and PATCH.
//! * Session lifecycle: a successful POST creates a new session,
//!   returns `201 Created` with a `Location` header, and
//!   increments the server's session count. A subsequent DELETE
//!   removes it.
//! * Unknown session handling on PATCH and DELETE returns 404.
//! * Trickle ICE body is forwarded to the session handle.
//! * [`RawSampleObserver`] fanout only reaches sessions whose
//!   broadcast matches.
//! * Empty offer body returns 400.
//!
//! The fuzz, e2e, and conformance slots land with the `str0m`
//! wiring in a later session.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use bytes::Bytes;
use lvqr_cmaf::RawSample;
use lvqr_ingest::{RawSampleObserver, VideoCodec};
use lvqr_whep::{SdpAnswerer, SessionHandle, WhepError, WhepServer};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

// =====================================================================
// Stubs
// =====================================================================

/// Stub answerer. Rejects empty offers with `MalformedOffer` and
/// otherwise returns a canned SDP answer. Every session it creates
/// shares the same [`CountingHandle`] counters via an `Arc` so
/// tests can assert raw-sample and trickle call counts.
struct StubAnswerer {
    trickle_count: Arc<AtomicUsize>,
    sample_count: Arc<AtomicUsize>,
}

impl StubAnswerer {
    fn new() -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let t = Arc::new(AtomicUsize::new(0));
        let s = Arc::new(AtomicUsize::new(0));
        (
            Self {
                trickle_count: t.clone(),
                sample_count: s.clone(),
            },
            t,
            s,
        )
    }
}

impl SdpAnswerer for StubAnswerer {
    fn create_session(&self, _broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhepError> {
        if offer.is_empty() {
            return Err(WhepError::MalformedOffer("empty".into()));
        }
        Ok((
            Box::new(CountingHandle {
                trickle_count: self.trickle_count.clone(),
                sample_count: self.sample_count.clone(),
            }),
            Bytes::from_static(b"v=0\r\nstub-answer\r\n"),
        ))
    }
}

struct CountingHandle {
    trickle_count: Arc<AtomicUsize>,
    sample_count: Arc<AtomicUsize>,
}

impl SessionHandle for CountingHandle {
    fn add_trickle(&self, _sdp_fragment: &[u8]) -> Result<(), WhepError> {
        self.trickle_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn on_raw_sample(&self, _track: &str, _codec: VideoCodec, _sample: &RawSample) {
        self.sample_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Stub answerer that hands out handles tagged with their broadcast
/// name so the fanout test can assert that `on_raw_sample` only
/// reached the subscribed sessions.
struct TaggingAnswerer {
    hits_one: Arc<AtomicUsize>,
    hits_two: Arc<AtomicUsize>,
}

impl SdpAnswerer for TaggingAnswerer {
    fn create_session(&self, broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhepError> {
        if offer.is_empty() {
            return Err(WhepError::MalformedOffer("empty".into()));
        }
        let counter = match broadcast {
            "live/one" => self.hits_one.clone(),
            "live/two" => self.hits_two.clone(),
            _ => Arc::new(AtomicUsize::new(0)),
        };
        Ok((
            Box::new(CountingHandle {
                trickle_count: Arc::new(AtomicUsize::new(0)),
                sample_count: counter,
            }),
            Bytes::from_static(b"v=0\r\nstub\r\n"),
        ))
    }
}

// =====================================================================
// Helpers
// =====================================================================

async fn body_bytes(body: Body) -> Bytes {
    // 16 KiB is generous for our test bodies. Production code should
    // use a real limit.
    to_bytes(body, 16 * 1024).await.expect("collect body")
}

fn sdp_offer(broadcast: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/whep/{broadcast}"))
        .header(header::CONTENT_TYPE, "application/sdp")
        .body(Body::from("v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n"))
        .expect("build post")
}

// =====================================================================
// POST /whep/{broadcast}
// =====================================================================

#[tokio::test]
async fn post_offer_returns_created_with_location_and_answer() {
    let (answerer, _trickle, _samples) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server.clone());

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
        location.starts_with("/whep/live/test/"),
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
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/whep/live/test")
        .body(Body::from("v=0\r\n"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(server.session_count(), 0, "failed POST must not register a session");
}

#[tokio::test]
async fn post_offer_with_wrong_content_type_returns_415() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server);

    let request = Request::builder()
        .method("POST")
        .uri("/whep/live/test")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn post_offer_accepts_content_type_with_parameters() {
    // `application/sdp; charset=utf-8` is a compliant content-type
    // for an SDP body. The server must accept it.
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server);

    let request = Request::builder()
        .method("POST")
        .uri("/whep/live/test")
        .header(header::CONTENT_TYPE, "application/sdp; charset=utf-8")
        .body(Body::from("v=0\r\n"))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn post_offer_with_empty_body_returns_400() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/whep/live/test")
        .header(header::CONTENT_TYPE, "application/sdp")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(server.session_count(), 0);
}

// =====================================================================
// DELETE /whep/{broadcast}/{session_id}
// =====================================================================

#[tokio::test]
async fn delete_unknown_session_returns_404() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server);

    let request = Request::builder()
        .method("DELETE")
        .uri("/whep/live/test/bogus")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn session_lifecycle_post_then_delete() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));

    // POST to create a session.
    let post_resp = lvqr_whep::router_for(server.clone())
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

    // DELETE that session.
    let del_resp = lvqr_whep::router_for(server.clone())
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

    // Second DELETE on the now-gone session is 404.
    let second_del = lvqr_whep::router_for(server.clone())
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
// PATCH /whep/{broadcast}/{session_id}
// =====================================================================

#[tokio::test]
async fn patch_unknown_session_returns_404() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server);

    let request = Request::builder()
        .method("PATCH")
        .uri("/whep/live/test/bogus")
        .header(header::CONTENT_TYPE, "application/trickle-ice-sdpfrag")
        .body(Body::from("a=candidate:..."))
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn patch_existing_session_forwards_to_handle() {
    let (answerer, trickle, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));

    let post_resp = lvqr_whep::router_for(server.clone())
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

    let patch_resp = lvqr_whep::router_for(server.clone())
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
async fn patch_with_wrong_content_type_returns_415() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));

    let post_resp = lvqr_whep::router_for(server.clone())
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

    let patch_resp = lvqr_whep::router_for(server)
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(location)
                .header(header::CONTENT_TYPE, "text/plain")
                .body(Body::from("not sdp"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch_resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

// =====================================================================
// RawSampleObserver fanout
// =====================================================================

#[tokio::test]
async fn raw_sample_observer_routes_only_to_subscribed_sessions() {
    let hits_one = Arc::new(AtomicUsize::new(0));
    let hits_two = Arc::new(AtomicUsize::new(0));
    let answerer = TaggingAnswerer {
        hits_one: hits_one.clone(),
        hits_two: hits_two.clone(),
    };
    let server = WhepServer::new(Arc::new(answerer));

    // Subscribe one session to `live/one` and one to `live/two`.
    let _ = lvqr_whep::router_for(server.clone())
        .oneshot(sdp_offer("live/one"))
        .await
        .unwrap();
    let _ = lvqr_whep::router_for(server.clone())
        .oneshot(sdp_offer("live/two"))
        .await
        .unwrap();
    assert_eq!(server.session_count(), 2);

    // Push samples for each broadcast. Only the matching session
    // should see them.
    let sample = RawSample {
        track_id: 1,
        dts: 0,
        cts_offset: 0,
        duration: 3000,
        payload: Bytes::from_static(&[0x00, 0x00, 0x00, 0x02, 0x65, 0x88]),
        keyframe: true,
    };

    server.on_raw_sample("live/one", "0.mp4", VideoCodec::H264, &sample);
    server.on_raw_sample("live/one", "0.mp4", VideoCodec::H264, &sample);
    server.on_raw_sample("live/two", "0.mp4", VideoCodec::H264, &sample);
    server.on_raw_sample("live/three", "0.mp4", VideoCodec::H264, &sample); // unsubscribed

    assert_eq!(hits_one.load(Ordering::SeqCst), 2);
    assert_eq!(hits_two.load(Ordering::SeqCst), 1);
}

// =====================================================================
// Regression: routing covers the exact WHEP path shape
// =====================================================================

#[tokio::test]
async fn unknown_route_returns_404() {
    let (answerer, _, _) = StubAnswerer::new();
    let server = WhepServer::new(Arc::new(answerer));
    let router = lvqr_whep::router_for(server);

    let request = Request::builder()
        .method("GET")
        .uri("/whep/live/test")
        .body(Body::empty())
        .unwrap();

    let response = router.oneshot(request).await.unwrap();
    // axum returns 405 for wrong-method-on-known-route, 404 for
    // unknown route. The route exists but GET is not allowed, so
    // expect 405.
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
