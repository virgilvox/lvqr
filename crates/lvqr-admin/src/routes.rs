use axum::{Json, Router, routing::get};
use lvqr_core::{Registry, RelayStats};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Stream info returned by the API.
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamInfo {
    pub name: String,
    pub subscribers: usize,
}

/// Build the admin API router.
pub fn build_router(registry: Arc<Registry>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/v1/stats", get(get_stats))
        .route("/api/v1/streams", get(list_streams))
        .with_state(registry)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn get_stats(axum::extract::State(registry): axum::extract::State<Arc<Registry>>) -> Json<RelayStats> {
    let track_names = registry.track_names();
    let total_subscribers: u64 = track_names.iter().map(|t| registry.subscriber_count(t) as u64).sum();

    let stats = RelayStats {
        tracks: track_names.len() as u64,
        subscribers: total_subscribers,
        ..Default::default()
    };
    Json(stats)
}

async fn list_streams(axum::extract::State(registry): axum::extract::State<Arc<Registry>>) -> Json<Vec<StreamInfo>> {
    let names = registry.track_names();
    let streams: Vec<StreamInfo> = names
        .iter()
        .map(|t| StreamInfo {
            name: t.as_str().to_string(),
            subscribers: registry.subscriber_count(t),
        })
        .collect();
    Json(streams)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use lvqr_core::TrackName;
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_returns_ok() {
        let registry = Arc::new(Registry::new());
        let app = build_router(registry);

        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn stats_empty_registry() {
        let registry = Arc::new(Registry::new());
        let app = build_router(registry);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.tracks, 0);
        assert_eq!(stats.subscribers, 0);
    }

    #[tokio::test]
    async fn stats_with_subscribers() {
        let registry = Arc::new(Registry::new());
        let track = TrackName::new("live/test");
        let _sub1 = registry.subscribe(&track);
        let _sub2 = registry.subscribe(&track);

        let app = build_router(registry);

        let response = app
            .oneshot(Request::builder().uri("/api/v1/stats").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let stats: RelayStats = serde_json::from_slice(&body).unwrap();
        assert_eq!(stats.tracks, 1);
        assert_eq!(stats.subscribers, 2);
    }

    #[tokio::test]
    async fn list_streams_returns_active_tracks() {
        let registry = Arc::new(Registry::new());
        let _sub1 = registry.subscribe(&TrackName::new("live/stream1"));
        let _sub2 = registry.subscribe(&TrackName::new("live/stream2"));

        let app = build_router(registry);

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
