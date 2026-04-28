//! Subscriber-side join helper for `<broadcast>/0.timing` anchors.
//!
//! **Session 159 PATH-X**. The producer-side
//! [`lvqr_fragment::MoqTimingTrackSink`] emits one
//! `(group_id_u64_le, ingest_time_ms_u64_le)` anchor per video
//! keyframe on a sibling MoQ track. Subscribers (the
//! `lvqr-moq-sample-pusher` bin in this crate, plus future
//! TypeScript / Python clients) hold the most-recent anchors in a
//! ring buffer and look them up by `group_id` whenever a video
//! frame arrives, so they can compute
//! `latency_ms = now_unix_ms() - anchor.ingest_time_ms`.
//!
//! Lookup strategy:
//!
//! * **Exact match** is the common case (the timing track ships its
//!   anchor in the same group sequence as the matching video
//!   keyframe; subscribers see them at the same logical instant).
//! * **Largest-group_id-less-than fallback** handles the case where
//!   the timing track's group is delayed beyond the video group's
//!   first delta frame.
//! * **Skip-on-miss** handles cold-start (subscriber jumps in
//!   mid-broadcast and the video track's first group arrives before
//!   the timing track's catches up).
//!
//! 64 anchors is enough headroom for ~128 s of GoP at 2 s
//! (`max-keyframe-interval=60` from the GStreamer pipeline strings)
//! and the bin's intended push cadence (~5 s); evicting older
//! anchors keeps the ring buffer constant-memory.

use lvqr_fragment::TimingAnchor;
use std::collections::VecDeque;

/// Default ring-buffer capacity for [`TimingAnchorJoin`]. Matches the
/// session-159 brief's locked decision 6 size.
pub const DEFAULT_RING_CAPACITY: usize = 64;

/// Subscriber-side ring buffer of [`TimingAnchor`] values, indexed
/// by `group_id`. Evicts oldest-first past
/// [`DEFAULT_RING_CAPACITY`] (or the configured cap).
///
/// Anchors must be pushed in monotonically-non-decreasing
/// `group_id` order; the producer-side sink's `track.append_group()`
/// auto-incrementing sequence guarantees this on the wire. Out-of-
/// order pushes are tolerated (re-bucketed in place) but no
/// guarantees are made about which anchor wins on duplicate
/// `group_id`.
#[derive(Debug)]
pub struct TimingAnchorJoin {
    anchors: VecDeque<TimingAnchor>,
    capacity: usize,
}

impl TimingAnchorJoin {
    /// Build a join with the default 64-anchor capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_RING_CAPACITY)
    }

    /// Build a join with a custom capacity. `0` is silently clamped
    /// to `1` so callers cannot accidentally produce a no-op join.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            anchors: VecDeque::with_capacity(capacity.max(1)),
            capacity: capacity.max(1),
        }
    }

    /// Insert one anchor. Evicts the oldest when at capacity.
    pub fn push(&mut self, anchor: TimingAnchor) {
        if self.anchors.len() == self.capacity {
            self.anchors.pop_front();
        }
        self.anchors.push_back(anchor);
    }

    /// Number of anchors currently retained.
    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    /// Whether the join holds zero anchors. (Clippy wants both `len`
    /// and `is_empty` for collection-like types.)
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// Look up the anchor matching `group_id` exactly. Returns the
    /// anchor on hit, falls back to the largest retained anchor
    /// whose `group_id <= group_id` query on miss, and returns
    /// `None` when no retained anchor satisfies either condition
    /// (i.e. the query is older than every retained anchor).
    pub fn lookup(&self, group_id: u64) -> Option<TimingAnchor> {
        // Exact match first.
        if let Some(hit) = self.anchors.iter().find(|a| a.group_id == group_id) {
            return Some(*hit);
        }
        // Fallback: largest retained anchor with group_id < query.
        // The ring is push-back ordered, so the newest anchor is at
        // the back; iterate from newest to oldest and return the
        // first one whose group_id < query.
        self.anchors.iter().rev().find(|a| a.group_id < group_id).copied()
    }
}

impl Default for TimingAnchorJoin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(group_id: u64, ingest_time_ms: u64) -> TimingAnchor {
        TimingAnchor {
            group_id,
            ingest_time_ms,
        }
    }

    #[test]
    fn lookup_returns_none_on_empty() {
        let join = TimingAnchorJoin::new();
        assert!(join.is_empty());
        assert_eq!(join.lookup(5), None);
    }

    #[test]
    fn lookup_exact_match_returns_anchor() {
        let mut join = TimingAnchorJoin::new();
        join.push(anchor(1, 100));
        join.push(anchor(2, 200));
        join.push(anchor(3, 300));
        assert_eq!(join.lookup(2), Some(anchor(2, 200)));
        assert_eq!(join.lookup(1), Some(anchor(1, 100)));
        assert_eq!(join.lookup(3), Some(anchor(3, 300)));
    }

    #[test]
    fn lookup_falls_back_to_largest_less_than() {
        let mut join = TimingAnchorJoin::new();
        join.push(anchor(1, 100));
        join.push(anchor(3, 300));
        join.push(anchor(5, 500));
        // Query 4 falls back to 3 (largest < 4).
        assert_eq!(join.lookup(4), Some(anchor(3, 300)));
        // Query 6 falls back to 5 (largest < 6).
        assert_eq!(join.lookup(6), Some(anchor(5, 500)));
        // Query 2 falls back to 1.
        assert_eq!(join.lookup(2), Some(anchor(1, 100)));
    }

    #[test]
    fn lookup_returns_none_when_query_older_than_window() {
        let mut join = TimingAnchorJoin::new();
        join.push(anchor(10, 1000));
        join.push(anchor(11, 1100));
        // Query 5 has no retained anchor with group_id < 5.
        assert_eq!(join.lookup(5), None);
    }

    #[test]
    fn capacity_evicts_oldest() {
        let mut join = TimingAnchorJoin::with_capacity(3);
        join.push(anchor(1, 100));
        join.push(anchor(2, 200));
        join.push(anchor(3, 300));
        join.push(anchor(4, 400));
        // Anchor 1 was evicted; lookup(1) misses + falls back to None
        // because no retained anchor has group_id < 1.
        assert_eq!(join.lookup(1), None);
        assert_eq!(join.lookup(2), Some(anchor(2, 200)));
        assert_eq!(join.lookup(4), Some(anchor(4, 400)));
        assert_eq!(join.len(), 3);
    }

    #[test]
    fn capacity_zero_clamps_to_one() {
        // Callers passing 0 (e.g. CLI flag default) get a 1-slot ring
        // rather than a no-op.
        let mut join = TimingAnchorJoin::with_capacity(0);
        join.push(anchor(1, 100));
        join.push(anchor(2, 200));
        assert_eq!(join.len(), 1);
        assert_eq!(join.lookup(2), Some(anchor(2, 200)));
        assert_eq!(join.lookup(1), None);
    }
}
