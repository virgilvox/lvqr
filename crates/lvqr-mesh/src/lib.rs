//! LVQR peer mesh topology planner.
//!
//! `MeshCoordinator` assigns peers to a relay tree, balances children
//! across parents, reassigns orphans, tracks heartbeat liveness, and
//! consults each peer's self-reported `capacity` (session 144) when
//! picking a parent. The `PeerAssignment` returned by `add_peer` and
//! `reassign_peer` is coordination state that `lvqr-signal` pushes to
//! the client via `SignalMessage::AssignParent`.
//!
//! ## Scope of this Rust crate
//!
//! This crate owns the *topology* side of the mesh: which peer should
//! relay to which other peer. It does not embed a WebRTC stack. The
//! actual peer-to-peer media forwarding lives in the browser SDK
//! (`bindings/js/packages/core/src/mesh.ts`'s `MeshPeer`), which opens
//! `RTCPeerConnection` to its assigned parent and forwards bytes over a
//! DataChannel using the 8-byte big-endian `object_id` framing the data
//! plane locked in session 111-B1.
//!
//! ## System-level mesh status: shipped (sessions 141-144)
//!
//! * Two-peer + three-peer Playwright E2Es exercise the full
//!   ingest-to-leaf path on every CI push (`mesh-e2e.yml`).
//! * Session 141 closed actual-vs-intended offload reporting: browser
//!   peers emit a `ForwardReport` signal message every second with
//!   their cumulative forwarded-frame counter; the coordinator
//!   aggregates per peer and surfaces the count via
//!   `MeshPeerStats.forwarded_frames` on `GET /api/v1/mesh` alongside
//!   the planner's `intended_children`.
//! * Session 143 added `--mesh-ice-servers <JSON>` so operators can
//!   push STUN/TURN config to every browser peer through
//!   `AssignParent`.
//! * Session 144 added per-peer capacity advertisement: browsers may
//!   self-report `capacity: u32` on `Register`, the lvqr-cli signal
//!   bridge clamps the claim to `--max-peers`, and
//!   `MeshCoordinator::find_best_parent` consults `PeerInfo.capacity`
//!   so a peer self-reporting `capacity: 1` forces subsequent peers
//!   to descend even when the global ceiling is higher.
//!
//! See `docs/mesh.md` (status: **IMPLEMENTED**) for the operator runbook
//! and the deployment recipe under `deploy/turn/`.

pub mod coordinator;
pub mod error;
pub mod tree;

pub use coordinator::{MeshConfig, MeshCoordinator};
pub use error::MeshError;
pub use tree::{PeerAssignment, PeerInfo, PeerRole};
