//! `/api/v1/streamkeys/*` admin routes.
//!
//! Exposes runtime CRUD over the [`lvqr_auth::StreamKeyStore`] wired
//! into [`crate::AdminState`] by `lvqr-cli`'s composition root. The
//! handlers go through the same admin-auth middleware as every other
//! `/api/v1/*` route, so a configured `--admin-token` (or JWT
//! provider) is required before mint / revoke / rotate.
//!
//! Wire shape mirrors the brief locked in `tracking/SESSION_146_BRIEFING.md`:
//!
//! ```text
//! GET    /api/v1/streamkeys                -> 200 { "keys": [StreamKey] }
//! POST   /api/v1/streamkeys                -> 201 StreamKey      (body: StreamKeySpec)
//! DELETE /api/v1/streamkeys/:id            -> 204 | 404
//! POST   /api/v1/streamkeys/:id/rotate     -> 200 StreamKey | 404 (body: StreamKeySpec or empty)
//! ```
//!
//! Every mutating route increments
//! `lvqr_streamkeys_changed_total{op="mint"|"revoke"|"rotate"}`
//! exactly once per successful API call, so dashboards can watch
//! the keyset velocity without scraping the full list.

use crate::routes::{AdminError, AdminState};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use lvqr_auth::{StreamKey, StreamKeySpec};
use serde::{Deserialize, Serialize};

/// Body for `GET /api/v1/streamkeys`. The wrapper exists so the
/// response can grow sibling fields (counts, pagination cursors)
/// without a breaking schema change. `#[serde(default)]` on
/// `keys` so a pre-146 client deserialising a future-server body
/// that omits the field gets `vec![]` rather than a parse error.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamKeyList {
    #[serde(default)]
    pub keys: Vec<StreamKey>,
}

/// `GET /api/v1/streamkeys`. Returns every entry currently in the
/// store, INCLUDING expired ones so operators can revoke stale
/// keys. With no store wired (operator passed `--no-streamkeys`)
/// returns `{"keys": []}` so polling tooling can run unconditionally.
pub async fn list_streamkeys(State(state): State<AdminState>) -> Result<Json<StreamKeyList>, AdminError> {
    let keys = match state.streamkey_store() {
        Some(store) => store.list(),
        None => Vec::new(),
    };
    Ok(Json(StreamKeyList { keys }))
}

/// `POST /api/v1/streamkeys`. Body is a [`StreamKeySpec`]; server
/// fills in `id`, `token`, `created_at`, and `expires_at` from
/// `ttl_seconds`. Returns `201 Created` with the full `StreamKey`
/// (including the literal token, per the operator-facing API
/// model -- see brief decision 5).
pub async fn mint_streamkey(
    State(state): State<AdminState>,
    Json(spec): Json<StreamKeySpec>,
) -> Result<Response, AdminError> {
    let Some(store) = state.streamkey_store() else {
        return Err(AdminError::Internal(
            "stream-key store is not configured (server booted with --no-streamkeys)".into(),
        ));
    };
    let key = store.mint(spec);
    metrics::counter!("lvqr_streamkeys_changed_total", "op" => "mint").increment(1);
    Ok((StatusCode::CREATED, Json(key)).into_response())
}

/// `DELETE /api/v1/streamkeys/:id`. Returns `204 No Content` on
/// success and `404 Not Found` for an unknown id (idempotent
/// callers can rely on 204 to mean "this id is no longer in the
/// store").
pub async fn revoke_streamkey(State(state): State<AdminState>, Path(id): Path<String>) -> Result<Response, AdminError> {
    let Some(store) = state.streamkey_store() else {
        return Err(AdminError::Internal(
            "stream-key store is not configured (server booted with --no-streamkeys)".into(),
        ));
    };
    if store.revoke(&id) {
        metrics::counter!("lvqr_streamkeys_changed_total", "op" => "revoke").increment(1);
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(AdminError::NotFound(format!("stream-key id {id:?} not found")))
    }
}

/// `POST /api/v1/streamkeys/:id/rotate`. Empty body preserves
/// label / broadcast / expires_at and only swaps the token; a
/// non-empty body is parsed as a [`StreamKeySpec`] override and
/// re-scopes the key while rotating (a `null` field on the
/// override CLEARS the existing field -- see the trait docs).
/// Returns `200 OK` with the new `StreamKey` body (including the
/// new token literal) on success, `404 Not Found` for an unknown id.
///
/// Body is taken as raw [`Bytes`] rather than `Json<StreamKeySpec>`
/// so a truly empty request (Content-Length: 0, no Content-Type)
/// rotates while preserving scope. axum's `Json` extractor 400s on
/// an empty body even when wrapped in `Option`, so SDKs that
/// idiomatically send no body for "no override" need this raw
/// parse to round-trip cleanly.
pub async fn rotate_streamkey(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Response, AdminError> {
    let Some(store) = state.streamkey_store() else {
        return Err(AdminError::Internal(
            "stream-key store is not configured (server booted with --no-streamkeys)".into(),
        ));
    };
    let override_spec: Option<StreamKeySpec> = if body.iter().all(u8::is_ascii_whitespace) {
        None
    } else {
        match serde_json::from_slice::<StreamKeySpec>(&body) {
            Ok(spec) => Some(spec),
            Err(e) => return Err(AdminError::Internal(format!("rotate body parse failed: {e}"))),
        }
    };
    match store.rotate(&id, override_spec) {
        Some(key) => {
            metrics::counter!("lvqr_streamkeys_changed_total", "op" => "rotate").increment(1);
            Ok((StatusCode::OK, Json(key)).into_response())
        }
        None => Err(AdminError::NotFound(format!("stream-key id {id:?} not found"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::build_router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use lvqr_auth::{InMemoryStreamKeyStore, SharedAuth, SharedStreamKeyStore, StaticAuthConfig, StaticAuthProvider};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn state_with_store() -> (AdminState, SharedStreamKeyStore) {
        let store: SharedStreamKeyStore = Arc::new(InMemoryStreamKeyStore::new());
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new)
            .with_streamkey_store(store.clone());
        (state, store)
    }

    async fn read_body(resp: axum::response::Response) -> Vec<u8> {
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn list_returns_empty_when_store_not_configured() {
        let state = AdminState::new(lvqr_core::RelayStats::default, Vec::<crate::StreamInfo>::new);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streamkeys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let parsed: StreamKeyList = serde_json::from_slice(&body).unwrap();
        assert!(parsed.keys.is_empty(), "no-store deployments must serve an empty list");
    }

    #[tokio::test]
    async fn mint_creates_a_key_and_lists_surface_it() {
        let (state, store) = state_with_store();
        let app = build_router(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/streamkeys")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"label":"camera-a","broadcast":"live/cam-a"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED, "mint must return 201");
        let body = read_body(resp).await;
        let key: StreamKey = serde_json::from_slice(&body).unwrap();
        assert!(key.token.starts_with("lvqr_sk_"));
        assert_eq!(key.label.as_deref(), Some("camera-a"));
        assert_eq!(key.broadcast.as_deref(), Some("live/cam-a"));

        // list must surface the just-minted entry.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streamkeys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let listed: StreamKeyList = serde_json::from_slice(&body).unwrap();
        assert_eq!(listed.keys.len(), 1);
        assert_eq!(listed.keys[0].id, key.id);

        // Store-side verification: the reverse index resolves the token.
        assert_eq!(store.get_by_token(&key.token).map(|k| k.id), Some(key.id));
    }

    #[tokio::test]
    async fn mint_accepts_empty_body_as_a_default_spec() {
        let (state, _store) = state_with_store();
        let app = build_router(state);
        // axum's Json extractor requires content-type + a body. An empty `{}`
        // is the conventional "no scope" mint.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/streamkeys")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = read_body(resp).await;
        let key: StreamKey = serde_json::from_slice(&body).unwrap();
        assert!(key.label.is_none() && key.broadcast.is_none() && key.expires_at.is_none());
    }

    #[tokio::test]
    async fn revoke_returns_204_then_404_on_repeat() {
        let (state, store) = state_with_store();
        let key = store.mint(StreamKeySpec::default());
        let app = build_router(state);

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/streamkeys/{}", key.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/v1/streamkeys/{}", key.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "second revoke is 404");
    }

    #[tokio::test]
    async fn rotate_empty_body_preserves_scope_and_swaps_token() {
        let (state, store) = state_with_store();
        let original = store.mint(StreamKeySpec {
            label: Some("preserve".into()),
            broadcast: Some("live/keep".into()),
            ttl_seconds: None,
        });
        let app = build_router(state);

        // Empty body -- preserve scope.
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/streamkeys/{}/rotate", original.id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let rotated: StreamKey = serde_json::from_slice(&body).unwrap();
        assert_eq!(rotated.id, original.id);
        assert_ne!(rotated.token, original.token);
        assert_eq!(rotated.label.as_deref(), Some("preserve"));
        assert_eq!(rotated.broadcast.as_deref(), Some("live/keep"));
    }

    #[tokio::test]
    async fn rotate_with_override_re_scopes() {
        let (state, store) = state_with_store();
        let original = store.mint(StreamKeySpec {
            label: Some("v1".into()),
            broadcast: Some("live/v1".into()),
            ttl_seconds: None,
        });
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/streamkeys/{}/rotate", original.id))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"label":"v2","broadcast":"live/v2","ttl_seconds":300}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        let rotated: StreamKey = serde_json::from_slice(&body).unwrap();
        assert_eq!(rotated.label.as_deref(), Some("v2"));
        assert_eq!(rotated.broadcast.as_deref(), Some("live/v2"));
        assert!(rotated.expires_at.is_some());
    }

    #[tokio::test]
    async fn rotate_unknown_id_is_404() {
        let (state, _store) = state_with_store();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/streamkeys/doesnotexist/rotate")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn streamkey_routes_respect_admin_auth() {
        let (mut state, _store) = state_with_store();
        let auth: SharedAuth = Arc::new(StaticAuthProvider::new(StaticAuthConfig {
            admin_token: Some("secret".into()),
            ..Default::default()
        }));
        state = state.with_auth(auth);
        let app = build_router(state);

        // Missing bearer -- 401.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streamkeys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Correct bearer -- 200.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/streamkeys")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
