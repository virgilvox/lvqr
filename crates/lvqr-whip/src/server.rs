//! WHIP server state, session registry, and the trait boundary that
//! decouples HTTP signaling from the actual WebRTC state machine.
//!
//! Sibling of `lvqr_whep::server`. The two crates intentionally do
//! not share a common core: the WHEP side packetizes outbound H.264
//! onto SRTP, the WHIP side depacketizes inbound SRTP back into
//! AVCC `RawSample` values, and the overlap is limited to the HTTP
//! surface. Keeping them in parallel modules is simpler than
//! extracting a generic WebRTC signaling crate and paying for the
//! extra indirection at every call site.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use dashmap::DashMap;
use lvqr_auth::{NoopAuthProvider, SharedAuth};
use rand::RngCore;
use std::sync::Arc;

/// Unique identifier for an active WHIP publisher session.
///
/// Encoded as 16 random bytes rendered as 32 lowercase hex
/// characters. The ID appears in URLs the client uses for trickle
/// ICE and session termination, so unpredictability is a defense-
/// in-depth property rather than a security boundary. WHIP does
/// not standardize the session identifier format; any URL-safe
/// token the server generates is acceptable.
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

    /// Wrap an existing string as a session identifier.
    pub fn from_string(s: String) -> Self {
        Self(s)
    }

    /// Borrow the underlying string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Error type surfaced to the axum router. Every HTTP error the
/// WHIP handlers can emit lands in one of these variants, and the
/// [`IntoResponse`] impl maps each variant onto an HTTP status code.
#[derive(Debug, thiserror::Error)]
pub enum WhipError {
    /// The request did not carry `Content-Type: application/sdp`.
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

    /// The configured [`AuthProvider`] denied the publisher. Maps to
    /// 401. Carries the provider's reason string so operators
    /// running `RUST_LOG=debug` can see why without exposing token
    /// details in the response body.
    ///
    /// [`AuthProvider`]: lvqr_auth::AuthProvider
    #[error("unauthorized: {0}")]
    Unauthorized(String),

    /// The offer carried at least one `m=video` section but none of
    /// them advertised a codec the str0m bridge can consume (today:
    /// H264 + HEVC). Without this gate, str0m happily accepts the
    /// offer + answers with `a=inactive` on the video media section,
    /// the publisher sees ICE connect, and no `MediaData` ever reaches
    /// the bridge -- the silent-drop class of bug the in-browser
    /// VP8 case famously hit. Per WHIP draft §3.1 + RFC 9359
    /// 415 Unsupported Media Type is the right shape for "the
    /// session cannot be negotiated as offered".
    #[error("unsupported video codec: {0}")]
    UnsupportedCodec(String),
}

impl IntoResponse for WhipError {
    fn into_response(self) -> Response {
        let status = match self {
            WhipError::UnsupportedContentType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            WhipError::MalformedOffer(_) => StatusCode::BAD_REQUEST,
            WhipError::SessionNotFound => StatusCode::NOT_FOUND,
            WhipError::AnswererFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
            WhipError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            WhipError::UnsupportedCodec(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        };
        let body = self.to_string();
        (status, body).into_response()
    }
}

/// Per-session handle for the WebRTC side of a WHIP publisher.
///
/// The router stores `Box<dyn SessionHandle>` in its registry. A
/// real implementation (backed by `str0m::Rtc`) uses the handle
/// methods to feed trickle ICE candidates into the ICE agent and
/// to tear the session down on DELETE. Unlike the WHEP handle, the
/// WHIP handle does not have a sample-push entry point: samples
/// flow the other way (inbound), pumped from inside the poll task
/// through an [`crate::IngestSampleSink`] handed to the answerer
/// at construction time.
pub trait SessionHandle: Send + Sync + 'static {
    /// Accept a trickle ICE candidate carried in an SDP fragment
    /// body. A `PATCH` handler calls this on receipt of a
    /// well-formed body. Errors bubble back up as 400.
    fn add_trickle(&self, sdp_fragment: &[u8]) -> Result<(), WhipError>;
}

/// Concrete SDP answerer contract. Separating this from the
/// signaling layer keeps the router testable without a live
/// WebRTC stack and lets `lvqr_whip::Str0mIngestAnswerer` drop in
/// behind the same trait.
pub trait SdpAnswerer: Send + Sync + 'static {
    /// Parse an SDP offer from a publishing client, construct
    /// whatever per-session state the WebRTC stack needs, and
    /// return a fresh [`SessionHandle`] plus the SDP answer body
    /// to send back to the client.
    ///
    /// Implementations should return [`WhipError::MalformedOffer`]
    /// when the offer itself is unparseable and
    /// [`WhipError::AnswererFailed`] for any other internal error.
    fn create_session(&self, broadcast: &str, offer: &[u8]) -> Result<(Box<dyn SessionHandle>, Bytes), WhipError>;
}

/// Internal entry stored in the session registry.
pub(crate) struct SessionEntry {
    #[allow(dead_code)]
    pub broadcast: String,
    pub handle: Box<dyn SessionHandle>,
}

/// Shared state underneath [`WhipServer`]. Held in an `Arc` so the
/// server can be cloned into the axum router without duplicating
/// the session registry.
pub(crate) struct WhipState {
    pub answerer: Arc<dyn SdpAnswerer>,
    pub sessions: DashMap<SessionId, SessionEntry>,
    /// Authentication provider consulted on the POST /whip/{broadcast}
    /// offer. `NoopAuthProvider` by default (open access), overridden
    /// via [`WhipServer::with_auth`].
    pub auth: SharedAuth,
}

/// Cheaply cloneable handle to the WHIP server.
///
/// Construct once with a real [`SdpAnswerer`] impl and clone into
/// the axum router via [`crate::router_for`]. Every clone shares
/// the same underlying session registry, so a successful POST on
/// any router clone is immediately visible to PATCH / DELETE on
/// any other.
#[derive(Clone)]
pub struct WhipServer {
    pub(crate) state: Arc<WhipState>,
}

impl WhipServer {
    /// Build a new server backed by a concrete SDP answerer.
    ///
    /// Auth defaults to open access ([`NoopAuthProvider`]); enable a
    /// specific provider by chaining [`WhipServer::with_auth`].
    pub fn new(answerer: Arc<dyn SdpAnswerer>) -> Self {
        Self {
            state: Arc::new(WhipState {
                answerer,
                sessions: DashMap::new(),
                auth: Arc::new(NoopAuthProvider),
            }),
        }
    }

    /// Build a new server with a pre-configured auth provider.
    ///
    /// Convenience constructor for callers that already hold a shared
    /// [`SharedAuth`]. Equivalent to `WhipServer::new(answerer).with_auth(auth)`
    /// but avoids the post-hoc `Arc::make_mut` dance on the internal
    /// state.
    pub fn with_auth_provider(answerer: Arc<dyn SdpAnswerer>, auth: SharedAuth) -> Self {
        Self {
            state: Arc::new(WhipState {
                answerer,
                sessions: DashMap::new(),
                auth,
            }),
        }
    }

    /// Reference to the auth provider. Exposed so the router can
    /// consult it without reaching through private state.
    pub(crate) fn auth(&self) -> &SharedAuth {
        &self.state.auth
    }

    /// Number of active publisher sessions currently registered.
    pub fn session_count(&self) -> usize {
        self.state.sessions.len()
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
    fn whip_error_status_mapping() {
        assert_eq!(
            WhipError::UnsupportedContentType.into_response().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            WhipError::MalformedOffer("bad".into()).into_response().status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            WhipError::SessionNotFound.into_response().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            WhipError::AnswererFailed("boom".into()).into_response().status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            WhipError::Unauthorized("nope".into()).into_response().status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
