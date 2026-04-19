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
mod noop;
mod provider;
mod static_provider;

#[cfg(feature = "jwt")]
mod jwt_provider;

pub use error::AuthError;
pub use noop::NoopAuthProvider;
pub use provider::{AuthContext, AuthDecision, AuthProvider, AuthScope, SharedAuth};
pub use static_provider::{StaticAuthConfig, StaticAuthProvider};

#[cfg(feature = "jwt")]
pub use jwt_provider::{JwtAuthConfig, JwtAuthProvider, JwtClaims};
