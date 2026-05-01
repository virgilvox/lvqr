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

    // Codec gate. WHIP draft §3.1 says the server SHOULD reject
    // offers it cannot serve with an HTTP error rather than
    // accepting an offer + answering with a=inactive. The str0m
    // ingest bridge consumes only H.264 + HEVC video; without
    // this check, an offer that carries only VP8 / VP9 / AV1
    // (Chrome's default ordering before `setCodecPreferences`
    // pins H264) negotiates a video media line that never
    // forwards a sample, and the publisher sees ICE connect with
    // no obvious failure. Detect the case + return 415 so the
    // operator gets a clear error.
    if let Err(reason) = check_video_codec_supported(&body) {
        tracing::warn!(broadcast = %broadcast, %reason, "WHIP offer rejected: no supported video codec");
        metrics::counter!(
            "lvqr_whip_unsupported_codec_total",
            "broadcast" => broadcast.clone(),
        )
        .increment(1);
        return Err(WhipError::UnsupportedCodec(reason));
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

/// Validate that the SDP offer carries at least one supported
/// video codec inside any `m=video` section. Returns `Ok(())` when
/// the offer either has no `m=video` (audio-only publisher; the
/// existing audio path handles that) or at least one video media
/// line whose payload types map to H264 or H265 via `a=rtpmap`.
/// Returns `Err(reason)` when every `m=video` section advertises
/// codecs we cannot serve (VP8 / VP9 / AV1 / etc).
///
/// Heuristic but spec-grounded. The function intentionally does
/// NOT reuse a full SDP parser because the validation is cheap +
/// the only failure mode is a permissive false-positive (we accept
/// an offer that should have been rejected, then fall back to the
/// existing silent-drop), not a false-negative (we never reject a
/// valid offer). False-negatives would break legitimate H264
/// publishers; the current implementation is line-oriented so
/// case-insensitive `H264` / `H265` substring matches are stable
/// against the SDP variants ffmpeg, OBS, and Chrome emit.
fn check_video_codec_supported(offer: &[u8]) -> Result<(), String> {
    let Ok(text) = std::str::from_utf8(offer) else {
        // Not UTF-8: defer the rejection to the answerer (which
        // emits MalformedOffer for unparseable SDP). The codec
        // gate is the operator-friendly path; surfacing a non-UTF-8
        // body as 415 would mis-classify the failure shape.
        return Ok(());
    };

    // Walk the offer line-by-line. Track which `m=` section we are
    // inside so a payload type advertised under `m=audio` cannot
    // satisfy the video codec check (e.g. an offer with `H264` in
    // the m=audio rtpmap -- non-conformant but defensively
    // rejected).
    let mut in_video_section = false;
    let mut saw_video = false;
    let mut saw_supported = false;
    let mut offered_codecs: Vec<String> = Vec::new();
    for line in text.split(['\n', '\r']) {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("m=") {
            // New media section. The first token is the media kind.
            let kind = rest.split(|c: char| c.is_whitespace()).next().unwrap_or("");
            in_video_section = kind.eq_ignore_ascii_case("video");
            if in_video_section {
                saw_video = true;
            }
            continue;
        }
        if !in_video_section {
            continue;
        }
        // a=rtpmap:<pt> <encoding-name>/<clock-rate>[/<channels>]
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            // Skip past the payload-type number to the encoding name.
            let after_pt = rest.split_once(' ').map(|(_, name)| name).unwrap_or(rest);
            let encoding = after_pt.split('/').next().unwrap_or("").trim();
            if !encoding.is_empty() {
                offered_codecs.push(encoding.to_string());
            }
            if encoding.eq_ignore_ascii_case("H264") || encoding.eq_ignore_ascii_case("H265") {
                saw_supported = true;
            }
        }
    }

    if !saw_video {
        // Audio-only offer. Not the C-6 case; let the existing
        // (silent-drop) audio path handle it.
        return Ok(());
    }
    if saw_supported {
        return Ok(());
    }
    let offered = if offered_codecs.is_empty() {
        "(no a=rtpmap lines under m=video)".to_string()
    } else {
        offered_codecs.join(", ")
    };
    Err(format!(
        "offer carries m=video but no H264/H265 rtpmap; offered codecs: [{offered}]"
    ))
}

#[cfg(test)]
mod codec_gate_tests {
    use super::check_video_codec_supported;

    const HEADER: &str = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n";

    #[test]
    fn h264_only_offer_is_accepted() {
        let sdp = format!(
            "{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 \
             packetization-mode=1\r\n"
        );
        check_video_codec_supported(sdp.as_bytes()).expect("H264 video should pass");
    }

    #[test]
    fn h265_only_offer_is_accepted() {
        let sdp = format!("{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 97\r\na=rtpmap:97 H265/90000\r\n");
        check_video_codec_supported(sdp.as_bytes()).expect("H265 video should pass");
    }

    #[test]
    fn vp8_only_offer_is_rejected_with_offered_list() {
        // The original VP8 silent-drop bug: Chrome offers VP8 first
        // without a `setCodecPreferences` pin. Pre-C-6, str0m
        // accepted the offer, replied a=inactive on video, and the
        // publisher saw ICE connect with no media. Now: 415 with
        // the rejected codec named in the response body.
        let sdp = format!("{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 VP8/90000\r\n");
        let err = check_video_codec_supported(sdp.as_bytes()).expect_err("VP8 should reject");
        assert!(err.contains("VP8"), "reason must name the rejected codec; got {err}");
    }

    #[test]
    fn vp9_av1_only_offer_is_rejected() {
        let sdp = format!(
            "{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 96 97\r\na=rtpmap:96 VP9/90000\r\na=rtpmap:97 \
             AV1/90000\r\n"
        );
        let err = check_video_codec_supported(sdp.as_bytes()).expect_err("VP9+AV1 should reject");
        assert!(err.contains("VP9") && err.contains("AV1"));
    }

    #[test]
    fn mixed_codec_offer_with_h264_among_alternatives_is_accepted() {
        // Realistic Chrome offer: VP8 first, then H264, then VP9.
        // After our `setCodecPreferences` browser-side fix Chrome
        // pins H264 only, but a non-LVQR-aware client still sends
        // the multi-codec list -- the gate must accept as long as
        // ANY codec we serve is in the offer.
        let sdp = format!(
            "{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 96 98 100\r\na=rtpmap:96 VP8/90000\r\na=rtpmap:98 \
             H264/90000\r\na=rtpmap:100 VP9/90000\r\n"
        );
        check_video_codec_supported(sdp.as_bytes()).expect("multi-codec with H264 should pass");
    }

    #[test]
    fn audio_only_offer_passes_through() {
        // No m=video at all -- the C-6 gate is video-only by scope.
        let sdp = format!("{HEADER}m=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n");
        check_video_codec_supported(sdp.as_bytes()).expect("audio-only offer must pass");
    }

    #[test]
    fn h264_in_audio_section_does_not_satisfy_video_gate() {
        // Defensive: a malformed offer that lists H264 under an
        // m=audio section must not trick the gate. The video
        // section here advertises only VP8.
        let sdp = format!(
            "{HEADER}m=audio 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\nm=video 9 \
             UDP/TLS/RTP/SAVPF 97\r\na=rtpmap:97 VP8/90000\r\n"
        );
        let err = check_video_codec_supported(sdp.as_bytes()).expect_err("must reject");
        assert!(err.contains("VP8"));
    }

    #[test]
    fn case_insensitive_codec_match() {
        // SDPs are technically case-insensitive on encoding names;
        // some publishers emit lowercase "h264".
        let sdp = format!("{HEADER}m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 h264/90000\r\n");
        check_video_codec_supported(sdp.as_bytes()).expect("lowercase h264 should pass");
    }

    #[test]
    fn empty_offer_passes_through() {
        // The body-empty check above this gate already returns 400.
        // The gate itself must not panic on an empty body so the
        // caller's existing error path stays in charge.
        check_video_codec_supported(b"").expect("empty body should pass through to other validators");
    }
}
