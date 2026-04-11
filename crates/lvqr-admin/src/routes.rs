use axum::{Json, Router, routing::get};
use lvqr_core::RelayStats;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Stream info returned by the API.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    pub name: String,
    pub subscribers: usize,
}

/// Shared state for the admin API.
///
/// Uses callbacks so the admin crate doesn't depend on relay or ingest types.
/// The CLI wires these to real relay metrics and bridge state.
#[derive(Clone)]
pub struct AdminState {
    get_stats: Arc<dyn Fn() -> RelayStats + Send + Sync>,
    get_streams: Arc<dyn Fn() -> Vec<StreamInfo> + Send + Sync>,
}

impl AdminState {
    pub fn new(
        get_stats: impl Fn() -> RelayStats + Send + Sync + 'static,
        get_streams: impl Fn() -> Vec<StreamInfo> + Send + Sync + 'static,
    ) -> Self {
        Self {
            get_stats: Arc::new(get_stats),
            get_streams: Arc::new(get_streams),
        }
    }
}

/// Build the admin API router.
pub fn build_router(state: AdminState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/streams", get(list_streams))
        .with_state(state)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn get_stats(axum::extract::State(state): axum::extract::State<AdminState>) -> Json<RelayStats> {
    Json((state.get_stats)())
}

async fn list_streams(axum::extract::State(state): axum::extract::State<AdminState>) -> Json<Vec<StreamInfo>> {
    Json((state.get_streams)())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state(streams: Vec<(&'static str, usize)>) -> AdminState {
        let streams_for_stats = streams.clone();
        AdminState::new(
            move || {
                let total_subs: u64 = streams_for_stats.iter().map(|(_, s)| *s as u64).sum();
                RelayStats {
                    tracks: streams_for_stats.len() as u64 * 2, // video + audio per stream
                    subscribers: total_subs,
                    publishers: streams_for_stats.len() as u64,
                    ..Default::default()
                }
            },
            move || {
                streams
                    .iter()
                    .map(|(name, subs)| StreamInfo {
                        name: name.to_string(),
                        subscribers: *subs,
                    })
                    .collect()
            },
        )
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stats_empty() {
        let state = test_state(vec![]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.tracks, 0);
        assert_eq!(stats.subscribers, 0);
        assert_eq!(stats.publishers, 0);
    }

    #[tokio::test]
    async fn stats_with_active_streams() {
        let state = test_state(vec![("live/test", 5)]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.publishers, 1);
        assert_eq!(stats.subscribers, 5);
        assert_eq!(stats.tracks, 2); // video + audio
    }

    #[tokio::test]
    async fn list_streams_returns_active() {
        let state = test_state(vec![("live/stream1", 2), ("live/stream2", 3)]);
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/streams").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let streams: Vec<StreamInfo> = serde_json::from_slice(&body).unwrap();
        assert_eq!(streams.len(), 2);

        let mut names: Vec<&str> = streams.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["live/stream1", "live/stream2"]);
    }
}
