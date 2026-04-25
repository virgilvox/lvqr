/// Mesh coordinator that manages the viewer relay tree.
///
/// The coordinator assigns new peers to the tree, balancing load across
/// parent nodes. Root peers connect directly to the origin server;
/// other peers connect through the tree via WebRTC DataChannels.
///
/// Algorithm:
/// 1. New peers become root peers until root_peer_count is reached
/// 2. After that, find the shallowest peer with available child slots
/// 3. Assign the new peer as a child of that parent
/// 4. If a parent disconnects, reassign orphaned children
use crate::error::MeshError;
use crate::tree::{PeerAssignment, PeerId, PeerInfo, PeerRole};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

/// Configuration for the peer mesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshConfig {
    /// Maximum number of children each peer can relay to.
    pub max_children: usize,
    /// Number of root peers directly served by the origin server.
    pub root_peer_count: usize,
    /// Maximum tree depth (hops from server to leaf).
    pub max_depth: u32,
    /// Heartbeat timeout in seconds. Peers not heard from in this
    /// interval are considered dead.
    pub heartbeat_timeout_secs: u64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            max_children: 3,
            root_peer_count: 30,
            max_depth: 6,
            heartbeat_timeout_secs: 10,
        }
    }
}

/// Manages the relay tree topology for a single track.
///
/// Thread-safe: all methods take &self and use DashMap internally.
pub struct MeshCoordinator {
    config: MeshConfig,
    /// All peers in the mesh, keyed by peer ID.
    peers: Arc<DashMap<PeerId, PeerInfo>>,
}

impl MeshCoordinator {
    pub fn new(config: MeshConfig) -> Self {
        Self {
            config,
            peers: Arc::new(DashMap::new()),
        }
    }

    pub fn config(&self) -> &MeshConfig {
        &self.config
    }

    /// Add a new peer to the mesh and assign it a position in the tree.
    ///
    /// `capacity` is the per-peer self-reported child cap. `None`
    /// falls back to `MeshConfig.max_children`. Callers MUST clamp
    /// the value to `[0, MeshConfig.max_children]` before invoking
    /// (the lvqr-cli signal bridge does so at register time).
    /// `effective_capacity` clamps again as a defense in depth, so a
    /// missed clamp at the call site cannot exceed the ceiling.
    /// Session 144.
    pub fn add_peer(&self, id: PeerId, track: String, capacity: Option<u32>) -> Result<PeerAssignment, MeshError> {
        let mut peer = PeerInfo::new(id.clone(), track);
        peer.capacity = capacity;

        // Count current root peers
        let root_count = self
            .peers
            .iter()
            .filter(|entry| entry.value().role == PeerRole::Root)
            .count();

        if root_count < self.config.root_peer_count {
            // Assign as root peer (directly from server)
            peer.role = PeerRole::Root;
            peer.depth = 0;
            let assignment = PeerAssignment {
                peer_id: id.clone(),
                role: PeerRole::Root,
                parent: None,
                depth: 0,
            };
            self.peers.insert(id, peer);
            metrics::gauge!("lvqr_mesh_peers").increment(1.0);
            metrics::gauge!("lvqr_mesh_offload_percentage").set(self.offload_percentage());
            debug!(peer = %assignment.peer_id, "assigned as root peer");
            return Ok(assignment);
        }

        // Find the shallowest peer with available child slots
        let parent = self.find_best_parent()?;

        peer.role = PeerRole::Relay;
        peer.parent = Some(parent.clone());
        peer.depth = self.peers.get(&parent).map(|p| p.depth + 1).unwrap_or(1);

        if peer.depth > self.config.max_depth {
            return Err(MeshError::TreeDepthExceeded {
                max: self.config.max_depth as usize,
            });
        }

        let assignment = PeerAssignment {
            peer_id: id.clone(),
            role: PeerRole::Relay,
            parent: Some(parent.clone()),
            depth: peer.depth,
        };

        // Add the new peer
        self.peers.insert(id.clone(), peer);

        // Add as child of parent
        if let Some(mut parent_entry) = self.peers.get_mut(&parent) {
            parent_entry.children.push(id);
        }

        metrics::gauge!("lvqr_mesh_peers").increment(1.0);
        metrics::gauge!("lvqr_mesh_offload_percentage").set(self.offload_percentage());
        debug!(
            peer = %assignment.peer_id,
            parent = ?assignment.parent,
            depth = assignment.depth,
            "assigned to relay tree"
        );

        Ok(assignment)
    }

    /// Remove a peer from the mesh. Returns the list of orphaned child peer IDs
    /// that need to be reassigned.
    pub fn remove_peer(&self, id: &str) -> Vec<PeerId> {
        let Some((_, peer)) = self.peers.remove(id) else {
            return Vec::new();
        };

        // Remove from parent's children list
        if let Some(parent_id) = &peer.parent {
            if let Some(mut parent_entry) = self.peers.get_mut(parent_id) {
                parent_entry.children.retain(|c| c != id);
            }
        }

        let orphans = peer.children.clone();

        // Clear parent references in orphaned children
        for orphan_id in &orphans {
            if let Some(mut orphan_entry) = self.peers.get_mut(orphan_id) {
                orphan_entry.parent = None;
            }
        }

        if !orphans.is_empty() {
            info!(peer = id, orphans = orphans.len(), "peer removed, children orphaned");
        }

        metrics::gauge!("lvqr_mesh_peers").decrement(1.0);
        metrics::gauge!("lvqr_mesh_offload_percentage").set(self.offload_percentage());
        orphans
    }

    /// Reassign a peer to a new parent.
    ///
    /// Used both for orphan reassignment (after the old parent was removed)
    /// and for live rebalance (moving a peer from an overloaded parent to
    /// an underloaded one without going through `remove_peer` first). The
    /// implementation handles both: it saves the peer's current parent
    /// before overwriting it and, if the current parent still exists,
    /// removes the stale child reference from its children list.
    ///
    /// Without the stale-child cleanup, live rebalance would leave every
    /// reassigned peer in its old parent's children vec, inflating
    /// `child_count()` and breaking `find_best_parent` load balancing.
    pub fn reassign_peer(&self, id: &str) -> Result<PeerAssignment, MeshError> {
        // Verify the peer exists
        if !self.peers.contains_key(id) {
            return Err(MeshError::PeerNotFound(id.to_string()));
        }

        // Find parent BEFORE acquiring any write locks to avoid DashMap deadlock
        let parent = self.find_best_parent()?;
        let parent_depth = self.peers.get(&parent).map(|p| p.depth).unwrap_or(0);

        // Capture the old parent before we overwrite it, and rewrite the
        // peer's own parent/depth fields in one short-lived entry borrow.
        // The old parent lookup happens after the entry is dropped so we do
        // not hold two DashMap references at once.
        let (assignment, old_parent) = {
            let Some(mut entry) = self.peers.get_mut(id) else {
                return Err(MeshError::PeerNotFound(id.to_string()));
            };

            let old_parent = entry.parent.clone();
            entry.parent = Some(parent.clone());
            entry.depth = parent_depth + 1;

            let assignment = PeerAssignment {
                peer_id: id.to_string(),
                role: entry.role,
                parent: Some(parent.clone()),
                depth: entry.depth,
            };
            (assignment, old_parent)
        };

        // Remove the stale child entry from the old parent's children list,
        // if the old parent is still in the mesh. No-op for the orphan case
        // because `remove_peer` already deleted the old parent entirely.
        if let Some(old_parent_id) = old_parent
            && old_parent_id != parent
            && let Some(mut old_parent_entry) = self.peers.get_mut(&old_parent_id)
        {
            old_parent_entry.children.retain(|c| c != id);
        }

        // Add as child of new parent (entry already dropped)
        if let Some(mut parent_entry) = self.peers.get_mut(&parent) {
            parent_entry.children.push(id.to_string());
        }

        debug!(
            peer = id,
            new_parent = %parent,
            depth = assignment.depth,
            "peer reassigned"
        );

        Ok(assignment)
    }

    /// Record a heartbeat from a peer.
    pub fn heartbeat(&self, id: &str) {
        if let Some(mut entry) = self.peers.get_mut(id) {
            entry.last_heartbeat = Some(std::time::Instant::now());
        }
    }

    /// Record the cumulative forwarded-frame count reported by a peer.
    ///
    /// The client sends its own running total on each report; the server
    /// replaces rather than accumulates so a reconnect (client counter
    /// resets to zero) cannot cause the displayed offload to drift
    /// upward forever. Unknown-peer reports are silently ignored: a
    /// client may briefly outlive its tree entry when `remove_peer`
    /// fires between the client's last emit and WS close.
    ///
    /// Session 141 -- actual-vs-intended offload reporting.
    pub fn record_forward_report(&self, id: &str, forwarded_frames: u64) {
        if let Some(mut entry) = self.peers.get_mut(id) {
            entry.forwarded_frames = forwarded_frames;
        }
    }

    /// Find peers that have timed out (no heartbeat within timeout period).
    pub fn find_dead_peers(&self) -> Vec<PeerId> {
        let timeout = std::time::Duration::from_secs(self.config.heartbeat_timeout_secs);
        let now = std::time::Instant::now();

        self.peers
            .iter()
            .filter(|entry| {
                entry
                    .value()
                    .last_heartbeat
                    .map(|hb| now.duration_since(hb) > timeout)
                    .unwrap_or(true)
            })
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Get info about a specific peer.
    pub fn get_peer(&self, id: &str) -> Option<PeerInfo> {
        self.peers.get(id).map(|entry| entry.value().clone())
    }

    /// Total number of peers in the mesh.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Number of root peers.
    pub fn root_count(&self) -> usize {
        self.peers
            .iter()
            .filter(|entry| entry.value().role == PeerRole::Root)
            .count()
    }

    /// Calculate the server bandwidth offload percentage.
    /// Returns the fraction of total viewers served by the mesh (not directly by server).
    pub fn offload_percentage(&self) -> f64 {
        let total = self.peers.len();
        if total == 0 {
            return 0.0;
        }
        let root = self.root_count();
        ((total - root) as f64 / total as f64) * 100.0
    }

    /// Get a snapshot of the tree structure for debugging/monitoring.
    pub fn tree_snapshot(&self) -> Vec<PeerInfo> {
        self.peers.iter().map(|entry| entry.value().clone()).collect()
    }

    /// Find the best parent for a new peer.
    /// Strategy: pick the shallowest peer with available child slots.
    /// Ties broken by fewest current children (distribute load evenly).
    fn find_best_parent(&self) -> Result<PeerId, MeshError> {
        let mut best: Option<(PeerId, u32, usize)> = None; // (id, depth, child_count)

        for entry in self.peers.iter() {
            let peer = entry.value();
            if !peer.can_accept_child(self.config.max_children) {
                continue;
            }
            if peer.depth >= self.config.max_depth {
                continue;
            }

            let candidate = (peer.id.clone(), peer.depth, peer.child_count());
            match &best {
                None => best = Some(candidate),
                Some((_, best_depth, best_children)) => {
                    // Prefer shallower depth, then fewer children
                    if candidate.1 < *best_depth || (candidate.1 == *best_depth && candidate.2 < *best_children) {
                        best = Some(candidate);
                    }
                }
            }
        }

        best.map(|(id, _, _)| id).ok_or(MeshError::NoAvailableParent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_coordinator() -> MeshCoordinator {
        MeshCoordinator::new(MeshConfig {
            max_children: 3,
            root_peer_count: 2,
            max_depth: 4,
            heartbeat_timeout_secs: 10,
        })
    }

    #[test]
    fn add_root_peers() {
        let coord = default_coordinator();

        let a1 = coord.add_peer("peer-1".into(), "live/test".into(), None).unwrap();
        assert_eq!(a1.role, PeerRole::Root);
        assert!(a1.parent.is_none());
        assert_eq!(a1.depth, 0);

        let a2 = coord.add_peer("peer-2".into(), "live/test".into(), None).unwrap();
        assert_eq!(a2.role, PeerRole::Root);

        assert_eq!(coord.peer_count(), 2);
        assert_eq!(coord.root_count(), 2);
    }

    #[test]
    fn assign_to_tree_after_roots_full() {
        let coord = default_coordinator();

        // Fill root slots
        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();

        // Next peer should be assigned as relay child of a root
        let a3 = coord.add_peer("peer-3".into(), "live/test".into(), None).unwrap();
        assert_eq!(a3.role, PeerRole::Relay);
        assert!(a3.parent.is_some());
        assert_eq!(a3.depth, 1);

        // Verify parent has the child
        let parent_id = a3.parent.unwrap();
        let parent = coord.get_peer(&parent_id).unwrap();
        assert!(parent.children.contains(&"peer-3".to_string()));
    }

    #[test]
    fn tree_balances_across_parents() {
        let coord = default_coordinator();

        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();

        // Add 4 relay peers - should balance across the 2 roots
        for i in 0..4 {
            coord.add_peer(format!("relay-{i}"), "live/test".into(), None).unwrap();
        }

        let r1 = coord.get_peer("root-1").unwrap();
        let r2 = coord.get_peer("root-2").unwrap();

        // Each root should have ~2 children (balanced)
        assert!(r1.child_count() <= 3);
        assert!(r2.child_count() <= 3);
        assert_eq!(r1.child_count() + r2.child_count(), 4);
    }

    #[test]
    fn max_children_enforced() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 2,
            root_peer_count: 1,
            max_depth: 4,
            heartbeat_timeout_secs: 10,
        });

        coord.add_peer("root".into(), "live/test".into(), None).unwrap();

        // Fill root's children
        coord.add_peer("c1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("c2".into(), "live/test".into(), None).unwrap();

        // Next peer should go deeper (child of c1 or c2)
        let a = coord.add_peer("c3".into(), "live/test".into(), None).unwrap();
        assert_eq!(a.depth, 2); // grandchild of root
    }

    #[test]
    fn remove_peer_returns_orphans() {
        let coord = default_coordinator();

        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();
        coord.add_peer("child-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("child-2".into(), "live/test".into(), None).unwrap();

        // Find which root has children
        let r1 = coord.get_peer("root-1").unwrap();
        if !r1.children.is_empty() {
            let orphans = coord.remove_peer("root-1");
            assert!(!orphans.is_empty());
            // Orphaned children should have parent cleared
            for orphan_id in &orphans {
                let orphan = coord.get_peer(orphan_id).unwrap();
                assert!(orphan.parent.is_none());
            }
        }
    }

    #[test]
    fn reassign_orphaned_peer() {
        let coord = default_coordinator();

        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();
        coord.add_peer("child-1".into(), "live/test".into(), None).unwrap();

        // Get child-1's parent
        let child = coord.get_peer("child-1").unwrap();
        let parent_id = child.parent.clone().unwrap();

        // Remove the parent
        let orphans = coord.remove_peer(&parent_id);
        assert!(orphans.contains(&"child-1".to_string()));

        // Reassign child-1 to the other root
        let new_assignment = coord.reassign_peer("child-1").unwrap();
        assert!(new_assignment.parent.is_some());
        assert_ne!(new_assignment.parent.unwrap(), parent_id);
    }

    #[test]
    fn offload_percentage() {
        let coord = default_coordinator();

        // No peers = 0% offload
        assert_eq!(coord.offload_percentage(), 0.0);

        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();

        // All roots = 0% offload
        assert_eq!(coord.offload_percentage(), 0.0);

        // Add relay peers
        coord.add_peer("relay-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("relay-2".into(), "live/test".into(), None).unwrap();

        // 2 roots + 2 relays = 50% offload
        assert_eq!(coord.offload_percentage(), 50.0);
    }

    #[test]
    fn depth_limit_enforced() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 1,
            root_peer_count: 1,
            max_depth: 2,
            heartbeat_timeout_secs: 10,
        });

        coord.add_peer("root".into(), "live/test".into(), None).unwrap();
        coord.add_peer("d1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("d2".into(), "live/test".into(), None).unwrap();

        // d2 is at depth 2, which equals max_depth. Next peer should fail.
        let result = coord.add_peer("d3".into(), "live/test".into(), None);
        assert!(result.is_err());
    }

    #[test]
    fn tree_snapshot() {
        let coord = default_coordinator();

        coord.add_peer("root-1".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-2".into(), "live/test".into(), None).unwrap();
        coord.add_peer("relay-1".into(), "live/test".into(), None).unwrap();

        let snapshot = coord.tree_snapshot();
        assert_eq!(snapshot.len(), 3);
    }

    #[test]
    fn heartbeat_keeps_peer_alive() {
        // 1-second timeout so we can observe the full lifecycle within a
        // single test without relying on sub-second granularity that
        // `heartbeat_timeout_secs` does not support.
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 3,
            root_peer_count: 2,
            max_depth: 4,
            heartbeat_timeout_secs: 1,
        });

        coord.add_peer("peer-1".into(), "live/test".into(), None).unwrap();

        // PeerInfo::new stamps last_heartbeat at construction time, so a
        // freshly registered peer is alive until the timeout elapses.
        let dead = coord.find_dead_peers();
        assert!(
            !dead.contains(&"peer-1".to_string()),
            "freshly registered peer should be alive"
        );

        // Sleep past the timeout. The peer should now be considered dead.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let dead = coord.find_dead_peers();
        assert!(
            dead.contains(&"peer-1".to_string()),
            "peer with stale heartbeat should be dead after timeout"
        );

        // Heartbeat resets the liveness clock. The peer is alive again.
        coord.heartbeat("peer-1");
        let dead = coord.find_dead_peers();
        assert!(
            !dead.contains(&"peer-1".to_string()),
            "heartbeat should reset the liveness clock"
        );
    }

    /// Regression test for the v0.4 audit finding: `reassign_peer` must
    /// remove the stale child reference from the old parent's children list
    /// when called on a live (not-yet-orphaned) peer. The orphan-reassign
    /// path already worked because `remove_peer` had deleted the old parent
    /// entirely; this tests the live rebalance case.
    #[test]
    fn reassign_live_peer_removes_stale_child_from_old_parent() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 10, // room for everyone so the test stays deterministic
            root_peer_count: 2,
            max_depth: 4,
            heartbeat_timeout_secs: 10,
        });

        // Two roots and one child attached to one of them.
        coord.add_peer("root-A".into(), "live/test".into(), None).unwrap();
        coord.add_peer("root-B".into(), "live/test".into(), None).unwrap();
        coord.add_peer("child".into(), "live/test".into(), None).unwrap();

        let child = coord.get_peer("child").unwrap();
        let original_parent = child.parent.clone().unwrap();

        // Sanity: the child is in the original parent's children list.
        let parent = coord.get_peer(&original_parent).unwrap();
        assert!(parent.children.contains(&"child".to_string()));

        // Reassign without removing. find_best_parent will pick whichever
        // root has fewer children, which may or may not be the same as
        // original_parent. If it picks the same parent, we cannot prove
        // the bug, so retry until we get a real move.
        //
        // In practice with two roots and one child, the new parent is
        // guaranteed to be the *other* root because find_best_parent
        // prefers fewer children first (the other root has 0, the
        // original parent has 1).
        coord.reassign_peer("child").unwrap();
        let reassigned = coord.get_peer("child").unwrap();
        let new_parent = reassigned.parent.clone().unwrap();
        assert_ne!(
            new_parent, original_parent,
            "reassign_peer should move the child to the less-loaded root"
        );

        // The old parent must no longer have the stale child reference.
        let old_parent_after = coord.get_peer(&original_parent).unwrap();
        assert!(
            !old_parent_after.children.contains(&"child".to_string()),
            "old parent still has stale child reference after live reassign"
        );

        // The new parent must own the child.
        let new_parent_after = coord.get_peer(&new_parent).unwrap();
        assert!(
            new_parent_after.children.contains(&"child".to_string()),
            "new parent missing the reassigned child"
        );
    }

    #[test]
    fn record_forward_report_sets_counter() {
        let coord = default_coordinator();
        coord.add_peer("peer-1".into(), "live/test".into(), None).unwrap();

        // Default is zero.
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 0);

        // Record a running total.
        coord.record_forward_report("peer-1", 42);
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 42);

        // Later reports replace rather than accumulate.
        coord.record_forward_report("peer-1", 100);
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 100);
    }

    #[test]
    fn record_forward_report_on_unknown_peer_is_noop() {
        let coord = default_coordinator();
        coord.add_peer("peer-1".into(), "live/test".into(), None).unwrap();

        // Should not panic; unknown IDs are silently ignored.
        coord.record_forward_report("nonexistent", 999);

        // Existing peer's counter is untouched.
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 0);
    }

    #[test]
    fn record_forward_report_handles_reconnect_reset() {
        let coord = default_coordinator();
        coord.add_peer("peer-1".into(), "live/test".into(), None).unwrap();

        // Client reports a running total, then reconnects (counter
        // drops back to zero on the client side). The server-visible
        // counter follows the wire value rather than clamping to the
        // previous max.
        coord.record_forward_report("peer-1", 500);
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 500);
        coord.record_forward_report("peer-1", 5);
        assert_eq!(coord.get_peer("peer-1").unwrap().forwarded_frames, 5);
    }

    #[test]
    fn record_forward_report_isolates_peers() {
        let coord = default_coordinator();
        coord.add_peer("peer-a".into(), "live/test".into(), None).unwrap();
        coord.add_peer("peer-b".into(), "live/test".into(), None).unwrap();

        coord.record_forward_report("peer-a", 100);
        coord.record_forward_report("peer-b", 250);

        assert_eq!(coord.get_peer("peer-a").unwrap().forwarded_frames, 100);
        assert_eq!(coord.get_peer("peer-b").unwrap().forwarded_frames, 250);
    }

    /// Session 144 regression: a peer that self-reports a capacity
    /// smaller than `MeshConfig.max_children` must cap its own
    /// children at that lower number, forcing find_best_parent to
    /// descend to the next available parent rather than over-loading
    /// the constrained peer.
    #[test]
    fn find_best_parent_respects_per_peer_capacity() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 5,
            root_peer_count: 1,
            max_depth: 4,
            heartbeat_timeout_secs: 10,
        });

        // peer-1 is the only Root; it self-reports capacity=1 so it
        // can host at most one child.
        let a1 = coord.add_peer("peer-1".into(), "live/test".into(), Some(1)).unwrap();
        assert_eq!(a1.role, PeerRole::Root);

        // peer-2 (no capacity) joins as the lone child of peer-1.
        let a2 = coord.add_peer("peer-2".into(), "live/test".into(), None).unwrap();
        assert_eq!(a2.role, PeerRole::Relay);
        assert_eq!(a2.parent.as_deref(), Some("peer-1"));
        assert_eq!(a2.depth, 1);

        // peer-3 must descend to peer-2 because peer-1 is at its
        // self-reported capacity of 1, even though MeshConfig.max_children
        // is 5.
        let a3 = coord.add_peer("peer-3".into(), "live/test".into(), None).unwrap();
        assert_eq!(a3.role, PeerRole::Relay);
        assert_eq!(
            a3.parent.as_deref(),
            Some("peer-2"),
            "peer-3 should descend to peer-2 because peer-1 hit capacity=1"
        );
        assert_eq!(a3.depth, 2);

        // PeerInfo.capacity is preserved on the Root entry.
        assert_eq!(coord.get_peer("peer-1").unwrap().capacity, Some(1));
        assert_eq!(coord.get_peer("peer-2").unwrap().capacity, None);
    }

    /// Session 144: a self-reported capacity larger than
    /// `MeshConfig.max_children` is automatically clamped at consult
    /// time by `effective_capacity`. Lvqr-cli also clamps at register
    /// time so on-disk values are bounded; this test exercises the
    /// defense-in-depth path where a programmatic caller forgot to
    /// clamp.
    #[test]
    fn find_best_parent_clamps_oversize_capacity() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 2,
            root_peer_count: 1,
            max_depth: 4,
            heartbeat_timeout_secs: 10,
        });

        // peer-1 claims a wildly inflated capacity. effective_capacity
        // clamps to MeshConfig.max_children = 2.
        coord
            .add_peer("peer-1".into(), "live/test".into(), Some(u32::MAX))
            .unwrap();

        // Two children fit on peer-1.
        coord.add_peer("peer-2".into(), "live/test".into(), None).unwrap();
        coord.add_peer("peer-3".into(), "live/test".into(), None).unwrap();

        // The next peer must descend, proving the clamp held.
        let a4 = coord.add_peer("peer-4".into(), "live/test".into(), None).unwrap();
        assert_eq!(
            a4.depth, 2,
            "expected descent because peer-1 must respect global ceiling"
        );
    }

    #[test]
    fn large_tree_formation() {
        let coord = MeshCoordinator::new(MeshConfig {
            max_children: 3,
            root_peer_count: 5,
            max_depth: 6,
            heartbeat_timeout_secs: 10,
        });

        // Add 50 peers
        for i in 0..50 {
            let result = coord.add_peer(format!("peer-{i}"), "live/test".into(), None);
            assert!(result.is_ok(), "failed to add peer-{i}: {:?}", result.err());
        }

        assert_eq!(coord.peer_count(), 50);
        assert_eq!(coord.root_count(), 5);

        // Offload should be 90% (45/50 are relays)
        assert_eq!(coord.offload_percentage(), 90.0);

        // Verify no peer exceeds max depth
        for entry in coord.tree_snapshot() {
            assert!(
                entry.depth <= 6,
                "peer {} has depth {}, exceeds max 6",
                entry.id,
                entry.depth
            );
        }
    }
}
