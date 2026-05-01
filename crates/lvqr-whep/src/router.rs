//! axum router for the WHEP signaling surface.
//!
//! WHEP (draft-ietf-wish-whep) signaling is three HTTP operations:
//!
//! * `POST /whep/{broadcast}` with an SDP offer body returns
//!   `201 Created`, a `Location` header naming the new session
//!   resource, and the SDP answer body.
//! * `PATCH /whep/{broadcast}/{session_id}` accepts trickle ICE
//!   candidates as an SDP fragment. `204 No Content` on success.
//! * `DELETE /whep/{broadcast}/{session_id}` tears down the
//!   session. `200 OK` on success.
//!
//! The handlers here validate request shape (content type on
//! writes, session lookup on patch / delete) and delegate every
//! WebRTC-specific decision to the [`crate::SdpAnswerer`] and
//! [`crate::SessionHandle`] traits stored behind the
//! [`crate::WhepServer`] state. Dropping in a real `str0m`-backed
//! answerer is a single type swap at construction time; the
//! router code is unchanged.

use crate::server::{SessionEntry, SessionId, WhepError, WhepServer};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Router, body::Bytes};
use lvqr_auth::{AuthDecision, extract};

/// Build the axum router wired to the given [`WhepServer`]. The
/// router is ready to mount under `lvqr-cli`'s axum binding (or
/// a dedicated `--whep-addr` socket) once the signaling layer is
/// composed.
///
/// Routing note: broadcast names in LVQR follow the RTMP
/// `{app}/{stream_key}` convention and therefore contain a `/`
/// (e.g. `live/test`). axum path parameters only match a single
/// URL segment, so the router uses a `/whep/{*path}` catch-all
/// and splits the tail off manually inside each handler — the
/// same pattern `lvqr-hls::MultiHlsServer::router` uses for the
/// LL-HLS surface.
pub fn router(server: WhepServer) -> Router {
    Router::new()
        .route(
            "/whep/{*path}",
            post(handle_offer).patch(handle_trickle).delete(handle_terminate),
        )
        .with_state(server)
}

/// Require `Content-Type: application/sdp`. Everything else is a
/// client bug and maps to 415. `Content-Type` is compared
/// case-insensitively against the exact media type; parameters
/// (e.g. `application/sdp; charset=utf-8`) are accepted.
fn require_sdp_content_type(headers: &HeaderMap) -> Result<(), WhepError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(WhepError::UnsupportedContentType);
    };
    let Ok(text) = value.to_str() else {
        return Err(WhepError::UnsupportedContentType);
    };
    // Accept `application/sdp` with or without parameters. Split
    // on `;` so `application/sdp; charset=utf-8` parses cleanly.
    let media_type = text.split(';').next().unwrap_or("").trim();
    if media_type.eq_ignore_ascii_case("application/sdp") {
        Ok(())
    } else {
        Err(WhepError::UnsupportedContentType)
    }
}

/// Split a catch-all path into `(broadcast, session_id)`. The
/// broadcast is everything before the last `/`, the session id is
/// the tail segment. Returns `None` if the path does not contain a
/// slash (i.e. the client addressed `/whep/{broadcast}` without a
/// session id on PATCH or DELETE).
fn split_session_path(path: &str) -> Option<(&str, &str)> {
    let (broadcast, session_id) = path.rsplit_once('/')?;
    if broadcast.is_empty() || session_id.is_empty() {
        return None;
    }
    Some((broadcast, session_id))
}

async fn handle_offer(
    State(server): State<WhepServer>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, WhepError> {
    require_sdp_content_type(&headers)?;
    if body.is_empty() {
        return Err(WhepError::MalformedOffer("empty offer body".into()));
    }

    // On a POST, the captured `path` is the broadcast name
    // verbatim (e.g. `live/test`). The server mints a fresh
    // session id and appends it to the path when building the
    // `Location` header.
    let broadcast = path;

    // Subscribe-side auth gate. WHEP is the egress (subscriber)
    // counterpart to WHIP and consults the operator's
    // `SubscribeAuth` bucket. Pre-fix the WHEP router did not
    // import lvqr-auth at all, so a deployment with
    // `--subscribe-token` configured silently exposed every
    // broadcast over WHEP. The check matches the pattern WHIP's
    // handle_offer + the live HLS / DASH `live_playback_auth_middleware`
    // already use: build the per-protocol AuthContext, ask the
    // shared provider, return 401 + the provider's reason on deny.
    let auth_header = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok());
    let ctx = extract::extract_whep(&broadcast, auth_header);
    if let AuthDecision::Deny { reason } = server.auth().check(&ctx) {
        tracing::warn!(broadcast = %broadcast, reason = %reason, "WHEP offer denied");
        return Err(WhepError::Unauthorized(reason));
    }

    // Audit C-9: if the upstream publisher has already sent an
    // audio codec config and the answerer cannot serve it (today:
    // an AAC publisher reaching a non-transcode-feature build),
    // return 422 upfront instead of accepting the session +
    // silently dropping every audio sample. Best-effort: when no
    // audio config has arrived yet (publisher not yet started, or
    // video-only broadcast), the cache is empty and the gate
    // falls through. The session-runtime per-sample
    // `lvqr_whep_codec_mismatch_drops_total` counter (audit I-7)
    // remains the canonical source of truth for drops the
    // up-front gate could not catch.
    if let Some(snapshot) = server.cached_audio_config(&broadcast)
        && !server.state.answerer.supports_audio_codec(snapshot.codec)
    {
        let codec_label = match snapshot.codec {
            lvqr_ingest::MediaCodec::Aac => "aac",
            lvqr_ingest::MediaCodec::Opus => "opus",
            lvqr_ingest::MediaCodec::H264 => "h264",
            lvqr_ingest::MediaCodec::H265 => "h265",
        };
        tracing::warn!(
            %broadcast,
            codec = codec_label,
            "WHEP offer rejected: publisher audio codec not serveable on this surface"
        );
        metrics::counter!(
            "lvqr_whep_audio_codec_unavailable_total",
            "broadcast" => broadcast.clone(),
            "codec" => codec_label,
        )
        .increment(1);
        return Err(WhepError::AudioCodecUnavailable(format!(
            "publisher audio codec is {codec_label}; this WHEP server cannot serve it (no transcoder wired)"
        )));
    }

    let (handle, answer) = server.state.answerer.create_session(&broadcast, &body)?;

    // Session 113: if the upstream publisher has already broadcast
    // the AAC sequence header, replay it to the new session
    // handle so a subscriber that joined after publish-start still
    // gets the AudioSpecificConfig the AAC-to-Opus transcoder needs.
    if let Some(cfg) = server.cached_audio_config(&broadcast) {
        handle.on_audio_config(&cfg.track, cfg.codec, &cfg.config_bytes);
    }

    let session_id = SessionId::new_random();
    server.state.sessions.insert(
        session_id.clone(),
        SessionEntry {
            broadcast: broadcast.clone(),
            handle,
        },
    );

    let location = format!("/whep/{}/{}", broadcast, session_id.as_str());
    let location_value = HeaderValue::from_str(&location)
        .map_err(|e| WhepError::AnswererFailed(format!("location header build failed: {e}")))?;

    let mut response = (
        StatusCode::CREATED,
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/sdp"))],
        answer,
    )
        .into_response();
    response.headers_mut().insert(header::LOCATION, location_value);
    tracing::debug!(broadcast = %broadcast, session = %session_id.as_str(), "whep session created");
    Ok(response)
}

async fn handle_trickle(
    State(server): State<WhepServer>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, WhepError> {
    // WHEP §4.2 specifies `application/trickle-ice-sdpfrag` for
    // trickle ICE bodies, but the IETF draft also permits
    // `application/sdp` in older implementations. Accept either
    // rather than reject a compliant client.
    require_trickle_content_type(&headers)?;

    // PATCH requires `/whep/{broadcast}/{session_id}`; reject the
    // shape without a session-id tail as `SessionNotFound`.
    let Some((_broadcast, session_id)) = split_session_path(&path) else {
        return Err(WhepError::SessionNotFound);
    };
    let session_id = SessionId::from_string(session_id.to_string());
    let entry = server
        .state
        .sessions
        .get(&session_id)
        .ok_or(WhepError::SessionNotFound)?;
    entry.value().handle.add_trickle(&body)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Trickle ICE bodies accept either `application/trickle-ice-sdpfrag`
/// or `application/sdp`. Empty content-type is still 415.
fn require_trickle_content_type(headers: &HeaderMap) -> Result<(), WhepError> {
    let Some(value) = headers.get(header::CONTENT_TYPE) else {
        return Err(WhepError::UnsupportedContentType);
    };
    let Ok(text) = value.to_str() else {
        return Err(WhepError::UnsupportedContentType);
    };
    let media_type = text.split(';').next().unwrap_or("").trim();
    if media_type.eq_ignore_ascii_case("application/trickle-ice-sdpfrag")
        || media_type.eq_ignore_ascii_case("application/sdp")
    {
        Ok(())
    } else {
        Err(WhepError::UnsupportedContentType)
    }
}

async fn handle_terminate(State(server): State<WhepServer>, Path(path): Path<String>) -> Result<Response, WhepError> {
    let Some((_broadcast, session_id)) = split_session_path(&path) else {
        return Err(WhepError::SessionNotFound);
    };
    let session_id = SessionId::from_string(session_id.to_string());
    let removed = server.state.sessions.remove(&session_id);
    if removed.is_none() {
        return Err(WhepError::SessionNotFound);
    }
    tracing::debug!(session = %session_id.as_str(), "whep session terminated");
    Ok(StatusCode::OK.into_response())
}
