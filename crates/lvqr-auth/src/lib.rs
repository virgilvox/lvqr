//! LVQR authentication.
//!
//! Provides a small trait-based authentication layer used by every LVQR entry point
//! (RTMP publish, WebSocket relay/ingest, MoQ session, admin API). The default
//! implementation `NoopAuthProvider` allows everything; users opt into stricter
//! checks by configuring `StaticAuthProvider` (env-var driven) or, behind the `jwt`
//! feature, `JwtAuthProvider`.
//!
//! ```no_run
//! use lvqr_auth::{AuthContext, AuthDecision, AuthProvider, StaticAuthConfig, StaticAuthProvider};
//!
//! let provider = StaticAuthProvider::new(StaticAuthConfig {
//!     admin_token: Some("secret".into()),
//!     publish_key: None,
//!     subscribe_token: None,
//! });
//!
//! let decision = provider.check(&AuthContext::Admin { token: "secret".into() });
//! assert!(matches!(decision, AuthDecision::Allow));
//! ```

mod error;
pub mod extract;
mod multi_key_provider;
mod noop;
mod provider;
mod static_provider;
mod stream_key_store;

#[cfg(feature = "jwt")]
mod jwt_provider;

#[cfg(feature = "jwks")]
mod jwks_provider;

#[cfg(feature = "webhook")]
mod webhook_provider;

pub use error::AuthError;
pub use multi_key_provider::MultiKeyAuthProvider;
pub use noop::NoopAuthProvider;
pub use provider::{AuthContext, AuthDecision, AuthProvider, AuthScope, SharedAuth};
pub use static_provider::{StaticAuthConfig, StaticAuthProvider};
pub use stream_key_store::{
    InMemoryStreamKeyStore, STREAM_KEY_TOKEN_PREFIX, SharedStreamKeyStore, StreamKey, StreamKeySpec, StreamKeyStore,
};

#[cfg(feature = "jwt")]
pub use jwt_provider::{JwtAuthConfig, JwtAuthProvider, JwtClaims};

#[cfg(feature = "jwks")]
pub use jwks_provider::{JwksAuthConfig, JwksAuthProvider};

#[cfg(feature = "webhook")]
pub use webhook_provider::{WebhookAuthConfig, WebhookAuthProvider};
