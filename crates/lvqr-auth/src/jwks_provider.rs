//! JWKS-based authentication provider (behind the `jwks` feature flag).
//!
//! Validates JWTs against public keys discovered dynamically from a JWKS
//! (JSON Web Key Set) endpoint. Keys are fetched at provider construction,
//! cached by `kid`, and refreshed on a periodic timer plus on-demand when
//! an incoming token carries an unknown `kid`.
//!
//! Default allowed algorithm set is `RS256` + `ES256` + `EdDSA`. HS256 is
//! deliberately rejected: distributing a symmetric HMAC secret over a public
//! JWKS endpoint would let anyone with the public key forge tokens, and
//! accepting both asymmetric and symmetric algorithms on the same provider
//! invites the classic "use the public key as the HMAC secret" downgrade
//! attack.
//!
//! The `AuthProvider::check` trait is synchronous because every other provider
//! does pure CPU work; we honour the contract by doing all I/O out of band
//! (initial fetch during the async `new`, periodic refresh on a dedicated
//! tokio task, kick-on-miss via `Notify`). A request that arrives with an
//! unknown `kid` returns `Deny` and signals the refresh task; the next request
//! after the refresh lands will succeed.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;

use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::error::AuthError;
use crate::jwt_provider::JwtClaims;
use crate::provider::{AuthContext, AuthDecision, AuthProvider, AuthScope};

/// Configuration for the JWKS auth provider.
#[derive(Debug, Clone)]
pub struct JwksAuthConfig {
    /// JWKS endpoint URL. Must be a valid absolute URL (`http` or `https`).
    pub jwks_url: String,
    /// Expected `iss` claim. When `None`, issuer is not checked.
    pub issuer: Option<String>,
    /// Expected `aud` claim. When `None`, audience is not checked.
    pub audience: Option<String>,
    /// How often the background task refreshes the JWKS cache.
    pub refresh_interval: Duration,
    /// Per-fetch HTTP timeout.
    pub fetch_timeout: Duration,
    /// Algorithms the provider will accept. Tokens whose header `alg` is not
    /// in this set are rejected before signature verification.
    pub allowed_algs: Vec<Algorithm>,
}

impl JwksAuthConfig {
    /// Sensible default algorithm set: RS256, ES256, EdDSA. Explicitly excludes
    /// every HS* variant so that a JWKS publishing an RSA public key cannot be
    /// tricked into accepting a token signed with that public key as an HMAC
    /// secret.
    pub fn default_allowed_algs() -> Vec<Algorithm> {
        vec![Algorithm::RS256, Algorithm::ES256, Algorithm::EdDSA]
    }
}

struct CacheEntry {
    key: DecodingKey,
    alg: Algorithm,
}

struct SharedState {
    cache: RwLock<HashMap<String, CacheEntry>>,
    refresh_notify: Notify,
}

/// JWKS authentication provider.
pub struct JwksAuthProvider {
    config: JwksAuthConfig,
    shared: Arc<SharedState>,
    refresh_handle: Option<JoinHandle<()>>,
}

impl JwksAuthProvider {
    /// Build a new provider. Performs an initial synchronous fetch so that a
    /// misconfigured JWKS URL (unreachable, empty, malformed) fails at server
    /// startup rather than on the first authenticated request. Spawns a
    /// background task that refreshes the cache on `config.refresh_interval`
    /// and also whenever an incoming token kicks `refresh_notify`.
    pub async fn new(config: JwksAuthConfig) -> Result<Self, AuthError> {
        validate_config(&config)?;
        let client = build_http_client(config.fetch_timeout)?;
        let keys = fetch_and_parse(&client, &config.jwks_url).await?;
        let shared = Arc::new(SharedState {
            cache: RwLock::new(keys),
            refresh_notify: Notify::new(),
        });
        let handle = spawn_refresh(client, config.jwks_url.clone(), config.refresh_interval, shared.clone());
        Ok(Self {
            config,
            shared,
            refresh_handle: Some(handle),
        })
    }

    pub fn config(&self) -> &JwksAuthConfig {
        &self.config
    }

    /// Number of keys currently cached. Exposed for tests and operator
    /// diagnostics; the value is a snapshot and may change the next instant.
    pub fn cached_key_count(&self) -> usize {
        self.shared.cache.read().map(|g| g.len()).unwrap_or(0)
    }

    fn decode(&self, token: &str) -> Result<JwtClaims, AuthError> {
        let header = decode_header(token).map_err(|e| AuthError::InvalidToken(e.to_string()))?;
        if !self.config.allowed_algs.contains(&header.alg) {
            return Err(AuthError::InvalidToken(format!(
                "algorithm {:?} not in allowed set",
                header.alg
            )));
        }
        let cache = self
            .shared
            .cache
            .read()
            .map_err(|_| AuthError::InvalidToken("jwks cache poisoned".into()))?;
        let entry = match header.kid.as_deref() {
            Some(kid) => cache.get(kid).ok_or_else(|| {
                self.shared.refresh_notify.notify_one();
                AuthError::InvalidToken(format!("kid not found in jwks cache: {kid}"))
            })?,
            None if cache.len() == 1 => cache.values().next().expect("len checked"),
            None => {
                return Err(AuthError::InvalidToken(
                    "token header missing kid and jwks has multiple keys".into(),
                ));
            }
        };
        if entry.alg != header.alg {
            return Err(AuthError::InvalidToken(format!(
                "algorithm mismatch: header={:?} jwk={:?}",
                header.alg, entry.alg
            )));
        }
        let mut validation = Validation::new(header.alg);
        if let Some(iss) = &self.config.issuer {
            validation.set_issuer(&[iss.as_str()]);
        }
        if let Some(aud) = &self.config.audience {
            validation.set_audience(&[aud.as_str()]);
        }
        decode::<JwtClaims>(token, &entry.key, &validation)
            .map(|d| d.claims)
            .map_err(|e| AuthError::InvalidToken(e.to_string()))
    }
}

impl Drop for JwksAuthProvider {
    fn drop(&mut self) {
        if let Some(h) = self.refresh_handle.take() {
            h.abort();
        }
    }
}

impl AuthProvider for JwksAuthProvider {
    fn check(&self, ctx: &AuthContext) -> AuthDecision {
        let (token, required_scope, broadcast_filter): (&str, AuthScope, Option<&str>) = match ctx {
            AuthContext::Publish { app, key, broadcast } => {
                let _ = app;
                (key.as_str(), AuthScope::Publish, broadcast.as_deref())
            }
            AuthContext::Subscribe { token, broadcast } => {
                let Some(t) = token.as_deref() else {
                    return AuthDecision::deny("subscribe token missing");
                };
                (t, AuthScope::Subscribe, Some(broadcast.as_str()))
            }
            AuthContext::Admin { token } => (token.as_str(), AuthScope::Admin, None),
        };

        match self.decode(token) {
            Ok(claims) => {
                if !claims.scope.includes(required_scope) {
                    return AuthDecision::deny(format!(
                        "token scope {:?} insufficient for {:?}",
                        claims.scope, required_scope
                    ));
                }
                if let (Some(filter), Some(broadcast)) = (claims.broadcast.as_deref(), broadcast_filter) {
                    if filter != broadcast {
                        return AuthDecision::deny("token bound to different broadcast");
                    }
                }
                AuthDecision::Allow
            }
            Err(e) => AuthDecision::deny(format!("jwks decode failed: {e}")),
        }
    }
}

fn validate_config(cfg: &JwksAuthConfig) -> Result<(), AuthError> {
    if cfg.jwks_url.is_empty() {
        return Err(AuthError::InvalidConfig("JWKS URL is empty".into()));
    }
    let parsed =
        url::Url::parse(&cfg.jwks_url).map_err(|e| AuthError::InvalidConfig(format!("JWKS URL parse failed: {e}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AuthError::InvalidConfig(format!(
            "JWKS URL must be http(s), got scheme {:?}",
            parsed.scheme()
        )));
    }
    if cfg.refresh_interval < Duration::from_secs(10) {
        return Err(AuthError::InvalidConfig(format!(
            "refresh interval {}s too small (minimum 10s)",
            cfg.refresh_interval.as_secs()
        )));
    }
    if cfg.allowed_algs.is_empty() {
        return Err(AuthError::InvalidConfig("allowed_algs is empty".into()));
    }
    for alg in &cfg.allowed_algs {
        if matches!(alg, Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512) {
            return Err(AuthError::InvalidConfig(format!(
                "HS* symmetric algorithm {alg:?} cannot be used with JWKS"
            )));
        }
    }
    Ok(())
}

fn build_http_client(timeout: Duration) -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| AuthError::InvalidConfig(format!("http client build failed: {e}")))
}

async fn fetch_and_parse(client: &reqwest::Client, url: &str) -> Result<HashMap<String, CacheEntry>, AuthError> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AuthError::InvalidConfig(format!("jwks fetch failed: {e}")))?
        .error_for_status()
        .map_err(|e| AuthError::InvalidConfig(format!("jwks fetch status: {e}")))?;
    let body = resp
        .bytes()
        .await
        .map_err(|e| AuthError::InvalidConfig(format!("jwks body read failed: {e}")))?;
    let set: JwkSet =
        serde_json::from_slice(&body).map_err(|e| AuthError::InvalidConfig(format!("jwks parse failed: {e}")))?;
    let mut out = HashMap::new();
    for jwk in set.keys {
        let Some(kid) = jwk.common.key_id.clone() else {
            tracing::debug!("skipping JWKS entry with no kid");
            continue;
        };
        match algorithm_for(&jwk) {
            Some(alg) => match DecodingKey::from_jwk(&jwk) {
                Ok(key) => {
                    out.insert(kid, CacheEntry { key, alg });
                }
                Err(e) => {
                    tracing::warn!(kid = %kid, error = %e, "failed to decode JWKS key; skipping");
                }
            },
            None => {
                tracing::debug!(kid = %kid, "unsupported JWKS key type or algorithm; skipping");
            }
        }
    }
    if out.is_empty() {
        return Err(AuthError::InvalidConfig("JWKS contains no usable keys".into()));
    }
    Ok(out)
}

/// Map a JWK's declared parameters onto one of the asymmetric algorithms this
/// provider accepts. Returns `None` for symmetric keys, unsupported curves, or
/// any RSA key whose JWK does not carry an explicit `alg` signal (we cannot
/// safely guess between RS256 and PS256 without the hint).
fn algorithm_for(jwk: &Jwk) -> Option<Algorithm> {
    match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => match jwk.common.key_algorithm {
            Some(jsonwebtoken::jwk::KeyAlgorithm::RS256) => Some(Algorithm::RS256),
            Some(jsonwebtoken::jwk::KeyAlgorithm::RS384) => Some(Algorithm::RS384),
            Some(jsonwebtoken::jwk::KeyAlgorithm::RS512) => Some(Algorithm::RS512),
            Some(jsonwebtoken::jwk::KeyAlgorithm::PS256) => Some(Algorithm::PS256),
            Some(jsonwebtoken::jwk::KeyAlgorithm::PS384) => Some(Algorithm::PS384),
            Some(jsonwebtoken::jwk::KeyAlgorithm::PS512) => Some(Algorithm::PS512),
            _ => None,
        },
        AlgorithmParameters::EllipticCurve(params) => match params.curve {
            EllipticCurve::P256 => Some(Algorithm::ES256),
            EllipticCurve::P384 => Some(Algorithm::ES384),
            _ => None,
        },
        AlgorithmParameters::OctetKeyPair(params) => match params.curve {
            EllipticCurve::Ed25519 => Some(Algorithm::EdDSA),
            _ => None,
        },
        AlgorithmParameters::OctetKey(_) => None,
    }
}

fn spawn_refresh(client: reqwest::Client, url: String, interval: Duration, shared: Arc<SharedState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {}
                _ = shared.refresh_notify.notified() => {}
            }
            match fetch_and_parse(&client, &url).await {
                Ok(keys) => match shared.cache.write() {
                    Ok(mut guard) => {
                        tracing::debug!(count = keys.len(), "jwks refresh ok");
                        *guard = keys;
                    }
                    Err(_) => {
                        tracing::warn!("jwks cache poisoned; skipping write");
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "jwks refresh failed; retaining cached keys");
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unix_exp_in(seconds: u64) -> usize {
        (SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + seconds) as usize
    }

    fn make_claims(scope: AuthScope, broadcast: Option<&str>) -> JwtClaims {
        make_claims_with_exp(scope, broadcast, unix_exp_in(600))
    }

    fn make_claims_with_exp(scope: AuthScope, broadcast: Option<&str>, exp: usize) -> JwtClaims {
        JwtClaims {
            sub: "alice".into(),
            exp,
            scope,
            iss: None,
            aud: None,
            broadcast: broadcast.map(String::from),
        }
    }

    /// A self-contained Ed25519 signer used by every integration test below.
    /// Backed by rcgen so we do not add a second crypto crate to the dep
    /// graph. `public_key_raw()` on an Ed25519 `KeyPair` returns the 32 raw
    /// public-key bytes that the JWK's `x` field needs.
    struct Ed25519TestKey {
        encoding: EncodingKey,
        jwk_x_b64: String,
    }

    impl Ed25519TestKey {
        fn new() -> Self {
            use base64::Engine;
            use base64::engine::general_purpose::URL_SAFE_NO_PAD;
            let kp = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("keypair");
            let pkcs8 = kp.serialize_der();
            let encoding = EncodingKey::from_ed_der(&pkcs8);
            let jwk_x_b64 = URL_SAFE_NO_PAD.encode(kp.public_key_raw());
            Self { encoding, jwk_x_b64 }
        }

        fn sign(&self, kid: &str, claims: &JwtClaims) -> String {
            let mut header = Header::new(Algorithm::EdDSA);
            header.kid = Some(kid.to_string());
            encode(&header, claims, &self.encoding).expect("encode jwt")
        }

        fn jwk_json(&self, kid: &str) -> serde_json::Value {
            serde_json::json!({
                "kty": "OKP",
                "crv": "Ed25519",
                "alg": "EdDSA",
                "use": "sig",
                "kid": kid,
                "x": self.jwk_x_b64,
            })
        }
    }

    fn make_provider_config(url: String) -> JwksAuthConfig {
        JwksAuthConfig {
            jwks_url: url,
            issuer: None,
            audience: None,
            refresh_interval: Duration::from_secs(10),
            fetch_timeout: Duration::from_secs(5),
            allowed_algs: JwksAuthConfig::default_allowed_algs(),
        }
    }

    #[test]
    fn config_default_algs_excludes_hs() {
        let algs = JwksAuthConfig::default_allowed_algs();
        assert!(algs.contains(&Algorithm::RS256));
        assert!(algs.contains(&Algorithm::ES256));
        assert!(algs.contains(&Algorithm::EdDSA));
        assert!(!algs.contains(&Algorithm::HS256));
        assert!(!algs.contains(&Algorithm::HS384));
        assert!(!algs.contains(&Algorithm::HS512));
    }

    #[test]
    fn validate_config_rejects_empty_url() {
        let cfg = make_provider_config(String::new());
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_non_http_scheme() {
        let cfg = make_provider_config("file:///etc/passwd".into());
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_hmac_algs() {
        let mut cfg = make_provider_config("https://example.invalid/jwks".into());
        cfg.allowed_algs = vec![Algorithm::RS256, Algorithm::HS256];
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_rejects_short_refresh_interval() {
        let mut cfg = make_provider_config("https://example.invalid/jwks".into());
        cfg.refresh_interval = Duration::from_secs(1);
        assert!(validate_config(&cfg).is_err());
    }

    #[test]
    fn validate_config_accepts_sensible_values() {
        let cfg = make_provider_config("https://idp.example.com/.well-known/jwks.json".into());
        assert!(validate_config(&cfg).is_ok());
    }

    #[tokio::test]
    async fn happy_path_accepts_signed_ed25519_token() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");
        assert_eq!(provider.cached_key_count(), 1);

        let token = signer.sign("kid-1", &make_claims(AuthScope::Admin, None));
        let decision = provider.check(&AuthContext::Admin { token });
        assert!(decision.is_allow(), "expected Allow, got {decision:?}");
    }

    #[tokio::test]
    async fn unknown_kid_denies_and_kicks_refresh() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-known")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        let token = signer.sign("kid-unknown", &make_claims(AuthScope::Admin, None));
        let decision = provider.check(&AuthContext::Admin { token });
        assert!(!decision.is_allow(), "expected Deny on unknown kid");
    }

    /// Adversarial: the JWKS provider must reject tokens whose
    /// `exp` claim is in the past, even though the kid + signature
    /// + alg are otherwise valid. Mirrors the JWT-provider hardening
    /// added in this audit cycle. A regression here would silently
    /// accept stale tokens past their declared lifetime.
    #[tokio::test]
    async fn expired_token_is_rejected_for_admin_subscribe_and_publish() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        // exp = 1 (1970-01-01 UTC + 1s); jsonwebtoken's default
        // leeway is 0, so the gate fires unambiguously.
        let admin = signer.sign("kid-1", &make_claims_with_exp(AuthScope::Admin, None, 1));
        let sub = signer.sign("kid-1", &make_claims_with_exp(AuthScope::Subscribe, Some("live/x"), 1));
        let pub_ = signer.sign("kid-1", &make_claims_with_exp(AuthScope::Publish, Some("live/x"), 1));

        assert!(
            !provider.check(&AuthContext::Admin { token: admin }).is_allow(),
            "expired admin token must be denied",
        );
        assert!(
            !provider
                .check(&AuthContext::Subscribe {
                    token: Some(sub),
                    broadcast: "live/x".into(),
                })
                .is_allow(),
            "expired subscribe token must be denied",
        );
        assert!(
            !provider
                .check(&AuthContext::Publish {
                    app: "whip".into(),
                    key: pub_,
                    broadcast: Some("live/x".into()),
                })
                .is_allow(),
            "expired publish token must be denied",
        );
    }

    /// Adversarial: a token signed with a different keypair than
    /// any kid-1 in the JWKS must be denied. Catches a regression
    /// where the JWKS path stops verifying signatures (e.g. by
    /// accepting any matching kid header without re-validating
    /// against the cached public key).
    #[tokio::test]
    async fn token_signed_with_different_keypair_is_denied() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let trusted = Ed25519TestKey::new();
        let attacker = Ed25519TestKey::new();
        // JWKS publishes only the TRUSTED key under kid-1.
        let jwks = serde_json::json!({ "keys": [trusted.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        // Attacker signs with their OWN key but stamps the kid as
        // kid-1, hoping the provider accepts on kid-match alone.
        let token = attacker.sign("kid-1", &make_claims(AuthScope::Admin, None));
        let decision = provider.check(&AuthContext::Admin { token });
        assert!(
            !decision.is_allow(),
            "token signed with non-trusted keypair must be denied even when kid matches",
        );
    }

    #[tokio::test]
    async fn tampered_token_denied() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        let mut token = signer.sign("kid-1", &make_claims(AuthScope::Admin, None));
        // Flip the last byte of the signature (after the final `.`).
        let last = token.pop().expect("non-empty token");
        let flipped = if last == 'A' { 'B' } else { 'A' };
        token.push(flipped);
        let decision = provider.check(&AuthContext::Admin { token });
        assert!(!decision.is_allow(), "expected Deny on tampered token");
    }

    #[tokio::test]
    async fn scope_enforcement_matches_jwt_provider() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        // Subscribe-scoped token cannot publish.
        let subscribe_token = signer.sign("kid-1", &make_claims(AuthScope::Subscribe, None));
        let publish_decision = provider.check(&AuthContext::Publish {
            app: "live".into(),
            key: subscribe_token,
            broadcast: None,
        });
        assert!(!publish_decision.is_allow());

        // Subscribe-scoped token binds to its broadcast claim.
        let broadcast_token = signer.sign("kid-1", &make_claims(AuthScope::Subscribe, Some("live/a")));
        let good = provider.check(&AuthContext::Subscribe {
            token: Some(broadcast_token.clone()),
            broadcast: "live/a".into(),
        });
        assert!(good.is_allow());
        let bad = provider.check(&AuthContext::Subscribe {
            token: Some(broadcast_token),
            broadcast: "live/b".into(),
        });
        assert!(!bad.is_allow());
    }

    #[tokio::test]
    async fn hs256_header_rejected_pre_signature_check() {
        // A token whose header claims alg=HS256 must be denied before the
        // signature gets inspected. We cannot ask jsonwebtoken to sign this
        // for us (it refuses to sign HS* with an asymmetric key, which is
        // itself the correct library-level check); instead we hand-craft a
        // token with a forged HS256 header and a junk signature. The
        // allowed_algs gate in decode() trips first, so the junk signature
        // never gets verified.
        use base64::Engine;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        let header_json = r#"{"alg":"HS256","typ":"JWT","kid":"kid-1"}"#;
        let header_b64 = URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let claims = make_claims(AuthScope::Admin, None);
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        let token = format!("{header_b64}.{payload_b64}.AAAA");

        let decision = provider.check(&AuthContext::Admin { token });
        assert!(!decision.is_allow(), "HS256 alg must be rejected by JWKS provider");
    }

    #[tokio::test]
    async fn key_rotation_refresh_picks_up_new_kid() {
        use std::sync::Mutex;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

        // Dynamic responder: first call returns JWKS with kid-a only; every
        // subsequent call returns JWKS with both kid-a + kid-b so the refresh
        // triggered by a miss on kid-b finds it.
        struct RotatingJwks {
            hits: Mutex<u32>,
            first: serde_json::Value,
            after: serde_json::Value,
        }
        impl Respond for RotatingJwks {
            fn respond(&self, _: &Request) -> ResponseTemplate {
                let mut hits = self.hits.lock().unwrap();
                *hits += 1;
                let body = if *hits == 1 { &self.first } else { &self.after };
                ResponseTemplate::new(200).set_body_json(body)
            }
        }

        let server = MockServer::start().await;
        let signer_a = Ed25519TestKey::new();
        let signer_b = Ed25519TestKey::new();
        let first = serde_json::json!({ "keys": [signer_a.jwk_json("kid-a")] });
        let after = serde_json::json!({
            "keys": [signer_a.jwk_json("kid-a"), signer_b.jwk_json("kid-b")]
        });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(RotatingJwks {
                hits: Mutex::new(0),
                first,
                after,
            })
            .mount(&server)
            .await;

        // Short refresh so the periodic tick picks up the second JWKS shape
        // quickly without dragging test wall-clock. Minimum validated at 10s,
        // so the kick-on-miss path is what we are actually exercising here.
        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");
        assert_eq!(provider.cached_key_count(), 1);

        // First request with kid-b: deny + kick.
        let token_b = signer_b.sign("kid-b", &make_claims(AuthScope::Admin, None));
        let first_decision = provider.check(&AuthContext::Admin { token: token_b.clone() });
        assert!(!first_decision.is_allow(), "unknown kid must deny");

        // Wait for the refresh task to land the new JWKS. Poll the cached
        // count with a generous timeout so CI on slow hosts does not flake.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while provider.cached_key_count() < 2 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(provider.cached_key_count(), 2, "refresh did not pick up kid-b");

        // Second request with the same token now succeeds.
        let second_decision = provider.check(&AuthContext::Admin { token: token_b });
        assert!(second_decision.is_allow(), "post-refresh token should verify");
    }

    #[tokio::test]
    async fn missing_kid_with_single_key_accepts() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let signer = Ed25519TestKey::new();
        let jwks = serde_json::json!({ "keys": [signer.jwk_json("kid-1")] });
        Mock::given(method("GET"))
            .and(path("/jwks.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&jwks))
            .mount(&server)
            .await;

        let cfg = make_provider_config(format!("{}/jwks.json", server.uri()));
        let provider = JwksAuthProvider::new(cfg).await.expect("provider");

        // Emit a token with NO kid header; OIDC allows this when there is
        // exactly one key in the set.
        let header = Header::new(Algorithm::EdDSA);
        let token = encode(&header, &make_claims(AuthScope::Admin, None), &signer.encoding).expect("encode");
        let decision = provider.check(&AuthContext::Admin { token });
        assert!(decision.is_allow());
    }

    #[tokio::test]
    async fn initial_fetch_failure_surfaces_error() {
        // Pointing at a closed port proves `new()` fails fast instead of
        // silently starting with an empty cache.
        let cfg = make_provider_config("http://127.0.0.1:1/jwks.json".into());
        let mut short = cfg.clone();
        short.fetch_timeout = Duration::from_millis(500);
        let result = JwksAuthProvider::new(short).await;
        match result {
            Ok(_) => panic!("expected new() to fail against a closed port"),
            Err(AuthError::InvalidConfig(_)) => {}
            Err(other) => panic!("expected InvalidConfig, got {other:?}"),
        }
    }
}
