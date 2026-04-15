//! WHEP server state, session registry, and the trait boundary that
//! decouples HTTP signaling from the actual WebRTC state machine.
//!
//! The router module wires these types onto an axum `Router`. A
//! concrete [`SdpAnswerer`] implementation plugs in the real
//! WebRTC stack (currently planned as `str0m`); tests plug in a
//! stub that returns canned answers.
//!
//! The split exists because the HTTP contract (status codes,
//! session lifecycle, `Location` header, content-type handling) is
//! load-bearing and should ship before the WebRTC API discovery
//! work locks in a specific runtime library. Deferring the str0m
//! integration to a later session lets that session focus on one
//! thing (getting offer / answer / ICE / DTLS / SRTP to round-trip
//! with a real browser client) without mixing router-shape
//! decisions into the same commit.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use dashmap::DashMap;
use lvqr_cmaf::RawSample;
use lvqr_ingest::{RawSampleObserver, VideoCodec};
use rand::RngCore;
use std::sync::Arc;

/// Unique identifier for an active WHEP subscriber session.
///
/// Encoded as 16 random bytes rendered as 32 lowercase hex
/// characters. The ID appears in URLs the client uses for trickle
/// ICE and session termination, so unpredictability is a defense-
/// in-depth property, not a security boundary. WHEP does not
/// standardize the session identifier format; any URL-safe token
/// the server generates is acceptable.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    /// Generate a fresh, random session ID.
    pub fn new_random() -> Self {
        let mut buf = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut buf);
        let mut hex = String::with_capacity(32);
        for byte in buf {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
        }
        Self(hex)
    }

    /// Wrap an existing string as a session identifier. Used by the
    /// router to parse the `{session_id}` path parameter back into a
    /// typed key for the session registry.
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Borrow the underlying string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Error type surfaced to the axum router. Every HTTP error the
/// WHEP handlers can emit lands in one of these variants, and the
/// [`IntoResponse`] impl maps each variant onto an HTTP status code.
#[derive(Debug, thiserror::Error)]
pub enum WhepError {
    /// The request did not carry `Content-Type: application/sdp`.
    /// WHEP clients are required to advertise the offer body as
    /// SDP; anything else is a client bug and returns 415.
    #[error("Content-Type: application/sdp required")]
    UnsupportedContentType,

    /// The offer body was empty, not valid SDP, or rejected by
    /// [`SdpAnswerer::create_session`] as structurally malformed.
    #[error("malformed SDP offer: {0}")]
    MalformedOffer(String),

    /// The session referenced by `{session_id}` is not in the
    /// registry. Fires for PATCH and DELETE on an unknown or
    /// already-terminated session.
    #[error("session not found")]
    SessionNotFound,

    /// The answerer impl (usually `str0m` once wired) failed in a
    /// non-client-visible way (DTLS handshake setup, ICE agent
    /// bind, internal state error). Maps to 500.
    #[error("answerer internal error: {0}")]
    AnswererFailed(String),
}

impl IntoResponse for WhepError {
    fn into_response(self) -> Response {
        let status = match self {
            WhepError::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            WhepError::MalformedOffer(_) => StatusCode::BAD_REQUEST,
            WhepError::SessionNotFound => StatusCode::NOT_FOUND,
            WhepError::AnswererFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = self.to_string();
        (status, body).into_response()
    }
}

/// Per-session handle for the WebRTC side of a WHEP subscription.
///
/// The router stores `Box<dyn SessionHandle>` in its registry. A
/// real implementation (backed by `str0m::Rtc`) uses the handle
/// methods to feed trickle ICE candidates into the ICE agent and
/// to packetize per-NAL / per-AAC samples into RTP packets destined
/// for the subscribed client.
///
/// Implementations must be `Send + Sync + 'static` so the same
/// handle can be invoked from the HTTP handler task (for trickle
/// ICE) and from the ingest bridge's tokio task (for raw-sample
/// delivery) concurrently.
pub trait SessionHandle: Send + Sync + 'static {
    /// Accept a trickle ICE candidate carried in an SDP fragment
    /// body. A `PATCH` handler calls this on receipt of a
    /// well-formed body. Errors bubble back up as 400.
    fn add_trickle(&self, sdp_fragment: &[u8]) -> Result<(), WhepError>;

    /// Called by [`WhepServer`]'s [`RawSampleObserver`] impl once
    /// per sample that the upstream bridge produced, for every
    /// session subscribed to that broadcast. A real implementation
    /// delegates to the RTP packetizer and pushes the resulting
    /// payloads through the `str0m::Rtc` state machine.
    ///
    /// `codec` is the video codec the upstream bridge stamped on
    /// the sample; the session uses it to pick the matching
    /// `str0m::Pt` for `Writer::write`. Audio samples carry the
    /// default variant and the implementation must not branch on
    /// it for audio tracks.
    fn on_raw_sample(&self, track: &str, codec: VideoCodec, sample: &RawSample);
}

/// Concrete SDP answerer contract. Separating this from the
/// signaling layer keeps the router testable without a live
/// WebRTC stack and lets a future `lvqr-whep::rtc::Str0mAnswerer`
/// drop in behind the same trait.
pub trait SdpAnswerer: Send + Sync + 'static {
    /// Parse an SDP offer, construct whatever per-session state the
    /// WebRTC stack needs, and return a fresh [`SessionHandle`]
    /// plus the SDP answer body to send back to the client.
    ///
    /// Implementations should return [`WhepError::MalformedOffer`]
    /// when the offer itself is unparseable and
    /// [`WhepError::AnswererFailed`] for any other internal error.
    fn create_session(&self, broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhepError>;
}

/// Internal entry stored in the session registry.
pub(crate) struct SessionEntry {
    pub broadcast: String,
    pub handle: Box<dyn SessionHandle>,
}

/// Shared state underneath [`WhepServer`]. Held in an `Arc` so the
/// server can be cloned into both the axum router and the ingest
/// bridge's `RawSampleObserver` slot without duplicating the
/// session registry.
pub(crate) struct WhepState {
    pub answerer: Arc<dyn SdpAnswerer>,
    pub sessions: DashMap<SessionId, SessionEntry>,
}

/// Cheaply cloneable handle to the WHEP server.
///
/// Construct once with a real [`SdpAnswerer`] impl, clone into the
/// axum router via [`WhepServer::router`] and into the ingest
/// bridge via [`lvqr_ingest::RtmpMoqBridge::with_raw_sample_observer`].
/// Both clones share the same underlying session registry, so a
/// subscribe POST on the router surface immediately makes the new
/// session visible to the raw-sample observer path.
#[derive(Clone)]
pub struct WhepServer {
    pub(crate) state: Arc<WhepState>,
}

impl WhepServer {
    /// Build a new server backed by a concrete SDP answerer.
    pub fn new(answerer: Arc<dyn SdpAnswerer>) -> Self {
        Self {
            state: Arc::new(WhepState {
                answerer,
                sessions: DashMap::new(),
            }),
        }
    }

    /// Number of active subscriber sessions currently registered.
    /// Exposed for tests and for a future admin metrics hook.
    pub fn session_count(&self) -> usize {
        self.state.sessions.len()
    }
}

/// [`RawSampleObserver`] impl that fans each incoming sample out
/// to every session whose `broadcast` field matches.
///
/// Iterates the whole session map per sample. For v0.x scale
/// (single-digit concurrent subscribers per broadcast) this is a
/// non-issue; a follow-up session can add a secondary
/// `broadcast -> Vec<SessionId>` index if the session fanout cost
/// shows up in profiling.
impl RawSampleObserver for WhepServer {
    fn on_raw_sample(&self, broadcast: &str, track: &str, codec: VideoCodec, sample: &RawSample) {
        for entry in self.state.sessions.iter() {
            let session = entry.value();
            if session.broadcast == broadcast {
                session.handle.on_raw_sample(track, codec, sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_is_32_hex_chars() {
        let id = SessionId::new_random();
        assert_eq!(id.as_str().len(), 32);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn session_ids_are_unique() {
        let a = SessionId::new_random();
        let b = SessionId::new_random();
        assert_ne!(a, b, "two fresh random session ids must differ");
    }

    #[test]
    fn whep_error_status_mapping() {
        assert_eq!(
            WhepError::UnsupportedContentType.into_response().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            WhepError::MalformedOffer("bad".into()).into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            WhepError::SessionNotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            WhepError::AnswererFailed("boom".into()).into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
