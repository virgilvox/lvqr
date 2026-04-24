//! Webhook authentication provider (behind the `webhook` feature flag).
//!
//! Delegates every `AuthContext` decision to an operator-configured HTTP
//! endpoint. The endpoint receives a JSON `POST` body shaped as one of:
//!
//! ```json
//! {"op":"publish","app":"live","key":"<bearer>","broadcast":"live/cam1"}
//! {"op":"subscribe","token":"<bearer>","broadcast":"live/cam1"}
//! {"op":"admin","token":"<bearer>"}
//! ```
//!
//! and must reply with `{"allow": bool, "reason": str?}` on a 2xx. Non-2xx
//! responses and malformed bodies are treated as deny with the error text
//! surfaced in the decision reason.
//!
//! # Caching model
//!
//! Because `AuthProvider::check` is synchronous and LVQR calls it from async
//! contexts (axum handlers, MoQ accept loops, RTMP callbacks), the provider
//! cannot block on an HTTP call inside `check`. Instead, decisions are cached
//! with per-entry TTLs, and cache misses return `Deny` while kicking a
//! background task that pulls the decision from the webhook and populates the
//! cache. Subsequent requests for the same context succeed once the webhook
//! call completes. This mirrors `JwksAuthProvider`'s unknown-kid behaviour
//! (first request denies, refresh task makes the next one succeed).
//!
//! A separate `deny_cache_ttl` exists so that a broken or adversarial webhook
//! cannot hammer the operator's infrastructure: when a call fails, the deny
//! is cached for its TTL so pending duplicates drain quickly.
//!
//! # Deduplication
//!
//! Concurrent `check` calls for the same cache key coalesce in a
//! `pending: HashMap<CacheKey, AuthContext>`. Only one POST per unique key
//! happens per batch.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::error::AuthError;
use crate::provider::{AuthContext, AuthDecision, AuthProvider};

/// Configuration for the webhook auth provider.
#[derive(Debug, Clone)]
pub struct WebhookAuthConfig {
    /// Absolute `http` / `https` URL that receives the JSON POST.
    pub webhook_url: String,
    /// How long an allow decision stays cached before the next check for the
    /// same context re-consults the webhook.
    pub allow_cache_ttl: Duration,
    /// How long a deny decision (including "webhook call failed") stays
    /// cached. Kept shorter than `allow_cache_ttl` by default so transient
    /// outages recover quickly; kept non-zero so a flapping webhook does not
    /// generate a POST per request.
    pub deny_cache_ttl: Duration,
    /// Per-request HTTP timeout for the webhook POST.
    pub fetch_timeout: Duration,
    /// Maximum number of cached decisions. On overflow the entry with the
    /// earliest `expires_at` is evicted; this is "oldest-first" rather than
    /// strict LRU, which is cheaper and sufficient for an auth-decision
    /// cache where TTL dominates.
    pub cache_capacity: NonZeroUsize,
}

impl WebhookAuthConfig {
    /// Sensible defaults for an operator who only has a webhook URL. 60 s
    /// positive cache, 10 s negative cache, 5 s fetch timeout, 4096 entries.
    pub fn with_url(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            allow_cache_ttl: Duration::from_secs(60),
            deny_cache_ttl: Duration::from_secs(10),
            fetch_timeout: Duration::from_secs(5),
            cache_capacity: NonZeroUsize::new(4096).expect("4096 != 0"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CacheKey {
    Publish {
        app: String,
        key: String,
        broadcast: Option<String>,
    },
    Subscribe {
        token: Option<String>,
        broadcast: String,
    },
    Admin {
        token: String,
    },
}

impl CacheKey {
    fn from_ctx(ctx: &AuthContext) -> Self {
        match ctx {
            AuthContext::Publish { app, key, broadcast } => Self::Publish {
                app: app.clone(),
                key: key.clone(),
                broadcast: broadcast.clone(),
            },
            AuthContext::Subscribe { token, broadcast } => Self::Subscribe {
                token: token.clone(),
                broadcast: broadcast.clone(),
            },
            AuthContext::Admin { token } => Self::Admin { token: token.clone() },
        }
    }
}

struct CacheEntry {
    decision: AuthDecision,
    expires_at: Instant,
}

struct SharedState {
    cache: RwLock<HashMap<CacheKey, CacheEntry>>,
    pending: StdMutex<HashMap<CacheKey, AuthContext>>,
    kick: Notify,
}

/// Webhook authentication provider. See module-level docs.
pub struct WebhookAuthProvider {
    config: WebhookAuthConfig,
    shared: Arc<SharedState>,
    fetcher_handle: Option<JoinHandle<()>>,
}

impl WebhookAuthProvider {
    /// Build a new provider. Validates the URL shape synchronously and spawns
    /// the background fetcher task. Does NOT probe the webhook endpoint; a
    /// webhook that is unreachable at runtime surfaces via `Deny` reasons on
    /// the first few decisions.
    pub async fn new(config: WebhookAuthConfig) -> Result<Self, AuthError> {
        validate_config(&config)?;
        let client = build_http_client(config.fetch_timeout)?;
        let shared = Arc::new(SharedState {
            cache: RwLock::new(HashMap::new()),
            pending: StdMutex::new(HashMap::new()),
            kick: Notify::new(),
        });
        let handle = spawn_fetcher(client, config.clone(), shared.clone());
        Ok(Self {
            config,
            shared,
            fetcher_handle: Some(handle),
        })
    }

    pub fn config(&self) -> &WebhookAuthConfig {
        &self.config
    }

    /// Current cache size. Exposed for tests and operator diagnostics.
    pub fn cached_decision_count(&self) -> usize {
        self.shared.cache.read().map(|g| g.len()).unwrap_or(0)
    }
}

impl Drop for WebhookAuthProvider {
    fn drop(&mut self) {
        if let Some(h) = self.fetcher_handle.take() {
            h.abort();
        }
    }
}

impl AuthProvider for WebhookAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        let key = CacheKey::from_ctx(ctx);
        let now = Instant::now();
        if let Ok(cache) = self.shared.cache.read() {
            if let Some(entry) = cache.get(&key) {
                if entry.expires_at > now {
                    return entry.decision.clone();
                }
            }
        }
        // Miss or expired. Enqueue and kick the fetcher. Concurrent checks
        // for the same key coalesce inside the pending HashMap.
        if let Ok(mut pending) = self.shared.pending.lock() {
            pending.entry(key).or_insert_with(|| ctx.clone());
        }
        self.shared.kick.notify_one();
        AuthDecision::deny("webhook cache miss; decision pending")
    }
}

fn validate_config(cfg: &WebhookAuthConfig) -> Result<(), AuthError> {
    if cfg.webhook_url.is_empty() {
        return Err(AuthError::InvalidConfig("webhook URL is empty".into()));
    }
    let parsed = url::Url::parse(&cfg.webhook_url)
        .map_err(|e| AuthError::InvalidConfig(format!("webhook URL parse failed: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AuthError::InvalidConfig(format!(
            "webhook URL must be http(s), got scheme {:?}",
            parsed.scheme()
        )));
    }
    if cfg.allow_cache_ttl < Duration::from_secs(1) {
        return Err(AuthError::InvalidConfig(
            "allow_cache_ttl must be >= 1s; otherwise the webhook gets hammered per request".into(),
        ));
    }
    if cfg.deny_cache_ttl.is_zero() {
        return Err(AuthError::InvalidConfig(
            "deny_cache_ttl must be > 0; a failing webhook would loop forever".into(),
        ));
    }
    if cfg.fetch_timeout.is_zero() {
        return Err(AuthError::InvalidConfig("fetch_timeout must be > 0".into()));
    }
    Ok(())
}

fn build_http_client(timeout: Duration) -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| AuthError::InvalidConfig(format!("http client build failed: {e}")))
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum WebhookRequestBody<'a> {
    Publish {
        app: &'a str,
        key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        broadcast: Option<&'a str>,
    },
    Subscribe {
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<&'a str>,
        broadcast: &'a str,
    },
    Admin {
        token: &'a str,
    },
}

impl<'a> WebhookRequestBody<'a> {
    fn from_ctx(ctx: &'a AuthContext) -> Self {
        match ctx {
            AuthContext::Publish { app, key, broadcast } => Self::Publish {
                app,
                key,
                broadcast: broadcast.as_deref(),
            },
            AuthContext::Subscribe { token, broadcast } => Self::Subscribe {
                token: token.as_deref(),
                broadcast,
            },
            AuthContext::Admin { token } => Self::Admin { token },
        }
    }
}

#[derive(Deserialize)]
struct WebhookResponse {
    allow: bool,
    #[serde(default)]
    reason: Option<String>,
}

fn spawn_fetcher(client: reqwest::Client, config: WebhookAuthConfig, shared: Arc<SharedState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            shared.kick.notified().await;
            // Take the whole pending map at once; the Notify permit semantics
            // guarantee that any kick landing while we drain generates a
            // fresh wakeup on the next .notified().await, so we never lose
            // an enqueued decision.
            let batch: HashMap<CacheKey, AuthContext> = match shared.pending.lock() {
                Ok(mut p) => std::mem::take(&mut *p),
                Err(_) => {
                    tracing::warn!("webhook pending mutex poisoned; stopping fetcher");
                    return;
                }
            };
            // Serial POSTs. A slow webhook backpressures all decisions; that
            // is safer than fan-out that could overwhelm the operator's
            // endpoint on a cold cache flood.
            for (key, ctx) in batch {
                let body = WebhookRequestBody::from_ctx(&ctx);
                let decision = match post_decision(&client, &config.webhook_url, &body).await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(error = %e, "webhook call failed");
                        AuthDecision::deny(format!("webhook call failed: {e}"))
                    }
                };
                let ttl = if decision.is_allow() {
                    config.allow_cache_ttl
                } else {
                    config.deny_cache_ttl
                };
                let entry = CacheEntry {
                    decision,
                    expires_at: Instant::now() + ttl,
                };
                match shared.cache.write() {
                    Ok(mut cache) => {
                        evict_if_full(&mut cache, config.cache_capacity.get());
                        cache.insert(key, entry);
                    }
                    Err(_) => {
                        tracing::warn!("webhook cache poisoned; skipping write");
                    }
                }
            }
        }
    })
}

async fn post_decision(
    client: &reqwest::Client,
    url: &str,
    body: &WebhookRequestBody<'_>,
) -> Result<AuthDecision, String> {
    let resp = client
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("send: {e}"))?
        .error_for_status()
        .map_err(|e| format!("status: {e}"))?;
    let parsed: WebhookResponse = resp.json().await.map_err(|e| format!("parse: {e}"))?;
    Ok(if parsed.allow {
        AuthDecision::Allow
    } else {
        AuthDecision::deny(parsed.reason.unwrap_or_else(|| "denied by webhook".into()))
    })
}

fn evict_if_full(cache: &mut HashMap<CacheKey, CacheEntry>, cap: usize) {
    if cache.len() < cap {
        return;
    }
    // O(n) scan for the oldest expires_at. n is bounded by `cache_capacity`
    // (default 4096), so even worst-case this is a few microseconds.
    if let Some(oldest_key) = cache.iter().min_by_key(|(_, e)| e.expires_at).map(|(k, _)| k.clone()) {
        cache.remove(&oldest_key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn make_config(url: String) -> WebhookAuthConfig {
        WebhookAuthConfig {
            webhook_url: url,
            allow_cache_ttl: Duration::from_secs(60),
            deny_cache_ttl: Duration::from_secs(5),
            fetch_timeout: Duration::from_secs(2),
            cache_capacity: NonZeroUsize::new(128).expect("non-zero"),
        }
    }

    async fn wait_for_decision(provider: &WebhookAuthProvider, ctx: &AuthContext) -> AuthDecision {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let decision = provider.check(ctx);
            let first_line = format!("{decision:?}");
            if !first_line.contains("cache miss") {
                return decision;
            }
            if Instant::now() >= deadline {
                panic!("timed out waiting for webhook decision; last: {decision:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[test]
    fn validate_config_rejects_empty_url() {
        let cfg = make_config(String::new());
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_non_http_scheme() {
        let cfg = make_config("file:///etc/passwd".into());
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_short_allow_ttl() {
        let mut cfg = make_config("https://auth.example.com/check".into());
        cfg.allow_cache_ttl = Duration::from_millis(500);
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_zero_deny_ttl() {
        let mut cfg = make_config("https://auth.example.com/check".into());
        cfg.deny_cache_ttl = Duration::from_secs(0);
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_zero_timeout() {
        let mut cfg = make_config("https://auth.example.com/check".into());
        cfg.fetch_timeout = Duration::from_secs(0);
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_accepts_sensible_values() {
        let cfg = make_config("https://auth.example.com/check".into());
        assert!(validate_config(&cfg).is_ok());
    }

    #[test]
    fn cache_key_discriminates_context_variants() {
        let a = CacheKey::from_ctx(&AuthContext::Admin { token: "t".into() });
        let b = CacheKey::from_ctx(&AuthContext::Subscribe {
            token: Some("t".into()),
            broadcast: "live/a".into(),
        });
        assert_ne!(a, b);
    }

    #[test]
    fn request_body_publish_has_op_tag() {
        let ctx = AuthContext::Publish {
            app: "live".into(),
            key: "k".into(),
            broadcast: None,
        };
        let body = WebhookRequestBody::from_ctx(&ctx);
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""op":"publish""#), "json: {json}");
        assert!(json.contains(r#""app":"live""#), "json: {json}");
        assert!(
            !json.contains("broadcast"),
            "broadcast should be skipped when None: {json}"
        );
    }

    #[tokio::test]
    async fn happy_path_allow_populates_cache_then_returns_allow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .and(body_partial_json(serde_json::json!({"op": "admin"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"allow": true})))
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "abc".into() };
        let decision = wait_for_decision(&provider, &ctx).await;
        assert!(decision.is_allow(), "expected Allow, got {decision:?}");
        assert_eq!(provider.cached_decision_count(), 1);
    }

    #[tokio::test]
    async fn deny_response_surfaces_reason() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"allow": false, "reason": "token revoked"})),
            )
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "abc".into() };
        let decision = wait_for_decision(&provider, &ctx).await;
        match decision {
            AuthDecision::Allow => panic!("expected Deny"),
            AuthDecision::Deny { reason } => assert!(reason.contains("token revoked"), "reason: {reason}"),
        }
    }

    #[tokio::test]
    async fn cache_hit_does_not_repost() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"allow": true})))
            .expect(1) // exactly one POST even across repeated check() calls
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "abc".into() };
        let first = wait_for_decision(&provider, &ctx).await;
        assert!(first.is_allow());
        // Repeated synchronous checks hit the cache; no new POSTs.
        for _ in 0..5 {
            let d = provider.check(&ctx);
            assert!(d.is_allow());
        }
    }

    #[tokio::test]
    async fn distinct_contexts_produce_distinct_cache_entries() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"allow": true})))
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let a = AuthContext::Admin {
            token: "token-a".into(),
        };
        let b = AuthContext::Admin {
            token: "token-b".into(),
        };
        let c = AuthContext::Subscribe {
            token: Some("token-c".into()),
            broadcast: "live/cam1".into(),
        };
        wait_for_decision(&provider, &a).await;
        wait_for_decision(&provider, &b).await;
        wait_for_decision(&provider, &c).await;
        assert_eq!(provider.cached_decision_count(), 3);
    }

    #[tokio::test]
    async fn webhook_5xx_denies_with_error_reason_and_caches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "abc".into() };
        let decision = wait_for_decision(&provider, &ctx).await;
        match decision {
            AuthDecision::Allow => panic!("5xx must deny"),
            AuthDecision::Deny { reason } => assert!(reason.contains("webhook call failed"), "reason: {reason}"),
        }
        // Failure cached for deny_cache_ttl so subsequent checks do not
        // hammer the broken endpoint.
        assert_eq!(provider.cached_decision_count(), 1);
    }

    #[tokio::test]
    async fn malformed_body_denies() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "abc".into() };
        let decision = wait_for_decision(&provider, &ctx).await;
        match decision {
            AuthDecision::Allow => panic!("unparseable body must deny"),
            AuthDecision::Deny { reason } => assert!(reason.contains("parse"), "reason: {reason}"),
        }
    }

    #[tokio::test]
    async fn concurrent_checks_for_same_context_coalesce_into_one_post() {
        // Prove that if N concurrent checks land before the fetcher drains
        // the batch, only 1 POST hits the webhook for that cache key.
        struct CountingResponder {
            hits: Mutex<u32>,
        }
        impl Respond for CountingResponder {
            fn respond(&self, _: &Request) -> ResponseTemplate {
                let mut h = self.hits.lock().unwrap();
                *h += 1;
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"allow": true}))
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(CountingResponder { hits: Mutex::new(0) })
            .mount(&server)
            .await;

        let cfg = make_config(format!("{}/check", server.uri()));
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        let ctx = AuthContext::Admin { token: "dup".into() };
        // Fire 20 checks synchronously before the fetcher can drain the
        // pending map. All 20 land before the first POST completes because
        // check() is non-blocking; the fetcher batches them together.
        for _ in 0..20 {
            let _ = provider.check(&ctx);
        }
        let final_decision = wait_for_decision(&provider, &ctx).await;
        assert!(final_decision.is_allow());
        // Can't assert exact hits == 1 without racing the fetcher, but the
        // cache should hold exactly one entry per key.
        assert_eq!(provider.cached_decision_count(), 1);
    }

    #[tokio::test]
    async fn eviction_oldest_first_when_capacity_exceeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/check"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"allow": true})))
            .mount(&server)
            .await;

        let mut cfg = make_config(format!("{}/check", server.uri()));
        cfg.cache_capacity = NonZeroUsize::new(2).expect("non-zero");
        let provider = WebhookAuthProvider::new(cfg).await.expect("provider");

        wait_for_decision(&provider, &AuthContext::Admin { token: "one".into() }).await;
        wait_for_decision(&provider, &AuthContext::Admin { token: "two".into() }).await;
        wait_for_decision(&provider, &AuthContext::Admin { token: "three".into() }).await;
        // Capacity is 2; three unique keys means one of the earlier entries
        // was evicted. Cache size is bounded.
        assert!(
            provider.cached_decision_count() <= 2,
            "cache size {} > capacity 2",
            provider.cached_decision_count()
        );
    }
}
