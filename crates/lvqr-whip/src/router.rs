//! axum router for the WHIP signaling surface.
//!
//! WHIP (draft-ietf-wish-whip) signaling is three HTTP operations:
//!
//! * `POST /whip/{broadcast}` with an SDP offer body returns
//!   `201 Created`, a `Location` header naming the new session
//!   resource, and the SDP answer body.
//! * `PATCH /whip/{broadcast}/{session_id}` accepts trickle ICE
//!   candidates as an SDP fragment. `204 No Content` on success.
//! * `DELETE /whip/{broadcast}/{session_id}` tears down the
//!   session. `200 OK` on success.
//!
//! The handlers here validate request shape (content type on
//! writes, session lookup on patch / delete) and delegate every
//! WebRTC-specific decision to the [`crate::SdpAnswerer`] and
//! [`crate::SessionHandle`] traits stored behind the
//! [`crate::WhipServer`] state. The router code is a near-mirror
//! of `lvqr_whep::router`; the differences are (a) the path
//! prefix and (b) that WHIP does not need a raw-sample-fanout
//! side channel because samples flow inbound through the
//! answerer, not outbound through the router.

use crate::server::{SessionEntry, SessionId, WhipError, WhipServer};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Router, body::Bytes};
use lvqr_auth::{AuthDecision, extract};

/// Build the axum router wired to the given [`WhipServer`]. Mount
/// under `lvqr-cli`'s dedicated `--whip-port` axum binding or
/// nest into another router.
///
/// Routing note: broadcast names follow the RTMP `{app}/{key}`
/// convention and therefore contain a `/`. axum path parameters
/// only match a single URL segment, so the router uses a
/// `/whip/{*path}` catch-all and splits the tail off manually
/// inside each handler — the same pattern `lvqr_whep::router`
/// uses for the WHEP surface.
pub fn router(server: WhipServer) -> Router {
    Router::new()
        .route(
            "/whip/{*path}",
            post(handle_offer).patch(handle_trickle).delete(handle_terminate),
        )
        .with_state(server)
}

fn require_sdp_content_type(headers: &HeaderMap) -> Result<(), WhipError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(WhipError::UnsupportedContentType);
    };
    let Ok(text) = value.to_str() else {
        return Err(WhipError::UnsupportedContentType);
    };
    let media_type = text.split(';').next().unwrap_or("").trim();
    if media_type.eq_ignore_ascii_case("application/sdp") {
        Ok(())
    } else {
        Err(WhipError::UnsupportedContentType)
    }
}

fn require_trickle_content_type(headers: &HeaderMap) -> Result<(), WhipError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(WhipError::UnsupportedContentType);
    };
    let Ok(text) = value.to_str() else {
        return Err(WhipError::UnsupportedContentType);
    };
    let media_type = text.split(';').next().unwrap_or("").trim();
    if media_type.eq_ignore_ascii_case("application/trickle-ice-sdpfrag")
        || media_type.eq_ignore_ascii_case("application/sdp")
    {
        Ok(())
    } else {
        Err(WhipError::UnsupportedContentType)
    }
}

/// Split a catch-all path into `(broadcast, session_id)`. The
/// broadcast is everything before the last `/`, the session id is
/// the tail segment. Returns `None` if the path does not contain
/// at least one slash (i.e. the client addressed `/whip/{broadcast}`
/// without a session id on PATCH or DELETE).
fn split_session_path(path: &str) -> Option<(&str, &str)> {
    let (broadcast, session_id) = path.rsplit_once('/')?;
    if broadcast.is_empty() || session_id.is_empty() {
        return None;
    }
    Some((broadcast, session_id))
}

async fn handle_offer(
    State(server): State<WhipServer>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, WhipError> {
    require_sdp_content_type(&headers)?;
    if body.is_empty() {
        return Err(WhipError::MalformedOffer("empty offer body".into()));
    }

    // On a POST, the captured `path` is the broadcast name
    // verbatim (e.g. `live/test`). The server mints a fresh
    // session id and appends it to the path when building the
    // `Location` header.
    let broadcast = path;

    let auth_header = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let ctx = extract::extract_whip(&broadcast, auth_header);
    if let AuthDecision::Deny { reason } = server.auth().check(&ctx) {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WHIP offer denied");
        return Err(WhipError::Unauthorized(reason));
    }

    let (handle, answer) = server.state.answerer.create_session(&broadcast, &body)?;

    let session_id = SessionId::new_random();
    server.state.sessions.insert(
        session_id.clone(),
        SessionEntry {
            broadcast: broadcast.clone(),
            handle,
        },
    );

    let location = format!("/whip/{}/{}", broadcast, session_id.as_str());
    let location_value = HeaderValue::from_str(&location)
        .map_err(|e| WhipError::AnswererFailed(format!("location header build failed: {e}")))?;

    let mut response = (
        StatusCode::CREATED,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/sdp"))],
        answer,
    )
        .into_response();
    response.headers_mut().insert(header::LOCATION, location_value);
    tracing::debug!(broadcast = %broadcast, session = %session_id.as_str(), "whip session created");
    Ok(response)
}

async fn handle_trickle(
    State(server): State<WhipServer>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, WhipError> {
    require_trickle_content_type(&headers)?;

    let Some((_broadcast, session_id)) = split_session_path(&path) else {
        return Err(WhipError::SessionNotFound);
    };
    let session_id = SessionId::from_string(session_id.to_string());
    let entry = server
        .state
        .sessions
        .get(&session_id)
        .ok_or(WhipError::SessionNotFound)?;
    entry.value().handle.add_trickle(&body)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn handle_terminate(State(server): State<WhipServer>, Path(path): Path<String>) -> Result<Response, WhipError> {
    let Some((_broadcast, session_id)) = split_session_path(&path) else {
        return Err(WhipError::SessionNotFound);
    };
    let session_id = SessionId::from_string(session_id.to_string());
    let removed = server.state.sessions.remove(&session_id);
    if removed.is_none() {
        return Err(WhipError::SessionNotFound);
    }
    tracing::debug!(session = %session_id.as_str(), "whip session terminated");
    Ok(StatusCode::OK.into_response())
}
