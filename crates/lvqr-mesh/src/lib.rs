//! LVQR peer mesh topology planner.
//!
//! **Status**: topology planner only. `MeshCoordinator` assigns peers to a
//! relay tree, balances children across parents, reassigns orphans, and
//! tracks heartbeat liveness. It does not yet drive real WebRTC peer
//! connections. The `PeerAssignment` returned by `add_peer` and
//! `reassign_peer` is coordination state that lvqr-signal pushes to the
//! client via `SignalMessage::AssignParent`; the client is currently
//! responsible for interpreting it and there is no server-side code that
//! opens a DataChannel to a parent peer and forwards media.
//!
//! The topology logic here is correct and ships the load-balancing,
//! depth-limit, and orphan reassignment behavior. It will be reused as-is
//! when the actual peer-to-peer media forwarding lands in Tier 4 of the
//! LVQR roadmap. Until then, the mesh offload percentage exposed via the
//! admin API reports *intended* offload, not *actual* offload.
//!
//! See `tracking/AUDIT-INTERNAL-2026-04-13.md` for the full architectural
//! note behind this scaffolding flag.

pub mod coordinator;
pub mod error;
pub mod tree;

pub use coordinator::{MeshConfig, MeshCoordinator};
pub use error::MeshError;
pub use tree::{PeerAssignment, PeerInfo, PeerRole};
