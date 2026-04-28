//! WebRTC signaling for LVQR mesh bootstrap.
//!
//! [`SignalServer`] is the `/signal` WebSocket endpoint browser
//! peers connect to before any DataChannel is opened. It speaks
//! [`SignalMessage`] -- a tagged JSON envelope covering `Register`,
//! `Offer`, `Answer`, `IceCandidate`, `AssignParent`, `Heartbeat`,
//! and `ForwardReport` (session 141 actual-offload reporting). The
//! server routes messages between peers via the
//! [`PeerCallback`] / [`ForwardReportCallback`] hooks the
//! `lvqr-cli` composition root installs on top of a
//! [`lvqr_mesh::MeshCoordinator`].
//!
//! ## Subscribe-token gate
//!
//! `/signal` is gated behind the shared `lvqr_auth::SubscribeAuth`
//! provider via two paths the lvqr-cli middleware accepts:
//! `Sec-WebSocket-Protocol: lvqr.bearer.<token>` (preferred) and
//! `?token=<token>` (query fallback). `--no-auth-signal` is the
//! escape hatch for deployments that want open mesh signaling
//! alongside open ingest / live HLS.
//!
//! ## ICE server push
//!
//! [`IceServer`] entries are propagated to every browser peer
//! through the [`SignalMessage::AssignParent`] message at registration
//! time so operator-configured TURN credentials never have to live
//! in client JavaScript. Hot-reloadable via the session-148
//! `mesh_ice_servers` config-reload key.

pub mod error;
pub mod signaling;

pub use error::SignalError;
pub use signaling::{ForwardReportCallback, IceServer, PeerCallback, PeerEvent, SignalMessage, SignalServer};
