use crate::types::{Frame, Gop};
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::sync::Arc;

/// Cache of recent GOPs for late-join support.
///
/// When a new subscriber joins, they need to start from a keyframe.
/// The GOP cache stores the N most recent complete GOPs so late-joiners
/// can immediately receive the latest keyframe and its dependent frames.
#[derive(Debug, Clone)]
pub struct GopCache {
    inner: Arc<RwLock<GopCacheInner>>,
}

#[derive(Debug)]
struct GopCacheInner {
    /// Completed GOPs, oldest first.
    complete: VecDeque<Gop>,
    /// The GOP currently being built (frames arriving between keyframes).
    current: Option<Gop>,
    /// Maximum number of complete GOPs to retain.
    max_gops: usize,
    /// Next GOP sequence number.
    next_sequence: u64,
}

impl GopCache {
    /// Create a new GOP cache that retains up to `max_gops` complete GOPs.
    pub fn new(max_gops: usize) -> Self {
        assert!(max_gops > 0, "max_gops must be > 0");
        Self {
            inner: Arc::new(RwLock::new(GopCacheInner {
                complete: VecDeque::with_capacity(max_gops),
                current: None,
                max_gops,
                next_sequence: 0,
            })),
        }
    }

    /// Push a frame into the cache.
    ///
    /// If the frame is a keyframe, the current in-progress GOP (if any) is
    /// finalized and a new one starts. When finalized GOPs exceed `max_gops`,
    /// the oldest is evicted.
    pub fn push_frame(&self, frame: Frame) {
        let mut inner = self.inner.write();

        if frame.keyframe {
            // Finalize the current GOP if it has frames
            if let Some(current) = inner.current.take() {
                if !current.is_empty() {
                    if inner.complete.len() >= inner.max_gops {
                        inner.complete.pop_front();
                    }
                    inner.complete.push_back(current);
                }
            }
            // Start a new GOP
            let seq = inner.next_sequence;
            inner.next_sequence += 1;
            let mut gop = Gop::new(seq);
            gop.push(frame);
            inner.current = Some(gop);
        } else {
            // Append to the current GOP (if one exists)
            if let Some(ref mut current) = inner.current {
                current.push(frame);
            }
            // If no current GOP exists (stream started mid-GOP), we drop
            // non-keyframes until the first keyframe arrives.
        }
    }

    /// Get the most recent complete GOP for late-join.
    /// Returns `None` if no complete GOP is available yet.
    pub fn latest_gop(&self) -> Option<Gop> {
        self.inner.read().complete.back().cloned()
    }

    /// Get all complete GOPs plus the current in-progress one.
    /// Useful for building a full catch-up buffer.
    pub fn all_gops(&self) -> Vec<Gop> {
        let inner = self.inner.read();
        let mut result: Vec<Gop> = inner.complete.iter().cloned().collect();
        if let Some(ref current) = inner.current {
            result.push(current.clone());
        }
        result
    }

    /// Get the current in-progress GOP (partial, not yet finalized).
    pub fn current_gop(&self) -> Option<Gop> {
        self.inner.read().current.clone()
    }

    /// Number of complete GOPs stored.
    pub fn complete_count(&self) -> usize {
        self.inner.read().complete.len()
    }

    /// Total number of frames across all cached GOPs (complete + current).
    pub fn total_frames(&self) -> usize {
        let inner = self.inner.read();
        let complete: usize = inner.complete.iter().map(|g| g.len()).sum();
        let current = inner.current.as_ref().map_or(0, |g| g.len());
        complete + current
    }

    /// Total payload size across all cached GOPs.
    pub fn total_size(&self) -> usize {
        let inner = self.inner.read();
        let complete: usize = inner.complete.iter().map(|g| g.total_size()).sum();
        let current = inner.current.as_ref().map_or(0, |g| g.total_size());
        complete + current
    }

    /// Clear all cached GOPs.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.complete.clear();
        inner.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn make_frame(seq: u64, keyframe: bool) -> Frame {
        Frame::new(seq, seq * 3000, keyframe, Bytes::from(vec![0u8; 1000]))
    }

    #[test]
    fn no_gops_initially() {
        let cache = GopCache::new(4);
        assert!(cache.latest_gop().is_none());
        assert_eq!(cache.complete_count(), 0);
        assert_eq!(cache.total_frames(), 0);
    }

    #[test]
    fn single_gop() {
        let cache = GopCache::new(4);

        // Push a keyframe followed by delta frames
        cache.push_frame(make_frame(0, true));
        cache.push_frame(make_frame(1, false));
        cache.push_frame(make_frame(2, false));

        // GOP is still in-progress, not yet complete
        assert_eq!(cache.complete_count(), 0);
        assert!(cache.latest_gop().is_none());
        assert_eq!(cache.total_frames(), 3);

        // Current GOP should have 3 frames
        let current = cache.current_gop().unwrap();
        assert_eq!(current.len(), 3);
    }

    #[test]
    fn gop_finalized_on_next_keyframe() {
        let cache = GopCache::new(4);

        // GOP 0: I P P
        cache.push_frame(make_frame(0, true));
        cache.push_frame(make_frame(1, false));
        cache.push_frame(make_frame(2, false));

        // GOP 1 starts: I
        cache.push_frame(make_frame(3, true));

        // Now GOP 0 should be complete
        assert_eq!(cache.complete_count(), 1);
        let latest = cache.latest_gop().unwrap();
        assert_eq!(latest.len(), 3);
        assert_eq!(latest.sequence, 0);

        // Current GOP (GOP 1) has 1 frame
        let current = cache.current_gop().unwrap();
        assert_eq!(current.len(), 1);
    }

    #[test]
    fn eviction_when_full() {
        let cache = GopCache::new(2); // keep only 2 complete GOPs

        // GOP 0: I P
        cache.push_frame(make_frame(0, true));
        cache.push_frame(make_frame(1, false));

        // GOP 1: I P
        cache.push_frame(make_frame(2, true));
        cache.push_frame(make_frame(3, false));

        // GOP 2: I P
        cache.push_frame(make_frame(4, true));
        cache.push_frame(make_frame(5, false));

        // GOP 3: I (finalizes GOP 2)
        cache.push_frame(make_frame(6, true));

        // Should have 2 complete GOPs (GOP 1 and GOP 2, GOP 0 evicted)
        assert_eq!(cache.complete_count(), 2);
        let all = cache.all_gops();
        assert_eq!(all.len(), 3); // 2 complete + 1 current
        assert_eq!(all[0].sequence, 1); // oldest kept
        assert_eq!(all[1].sequence, 2);
    }

    #[test]
    fn drops_frames_before_first_keyframe() {
        let cache = GopCache::new(4);

        // Push delta frames before any keyframe
        cache.push_frame(make_frame(0, false));
        cache.push_frame(make_frame(1, false));

        // These should be silently dropped
        assert_eq!(cache.total_frames(), 0);
        assert!(cache.current_gop().is_none());

        // Now push a keyframe
        cache.push_frame(make_frame(2, true));
        assert_eq!(cache.total_frames(), 1);
    }

    #[test]
    fn total_size_accumulates() {
        let cache = GopCache::new(4);

        cache.push_frame(Frame::new(0, 0, true, Bytes::from(vec![0u8; 500])));
        cache.push_frame(Frame::new(1, 1, false, Bytes::from(vec![0u8; 200])));

        assert_eq!(cache.total_size(), 700);
    }

    #[test]
    fn clear_resets_everything() {
        let cache = GopCache::new(4);
        cache.push_frame(make_frame(0, true));
        cache.push_frame(make_frame(1, false));
        cache.push_frame(make_frame(2, true));

        cache.clear();
        assert_eq!(cache.complete_count(), 0);
        assert!(cache.current_gop().is_none());
        assert_eq!(cache.total_frames(), 0);
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(GopCache::new(4));

        let writer = {
            let cache = cache.clone();
            thread::spawn(move || {
                for i in 0..1000u64 {
                    let keyframe = i % 30 == 0;
                    cache.push_frame(make_frame(i, keyframe));
                }
            })
        };

        let reader = {
            let cache = cache.clone();
            thread::spawn(move || {
                let mut reads = 0;
                for _ in 0..500 {
                    if cache.latest_gop().is_some() {
                        reads += 1;
                    }
                    let _ = cache.total_frames();
                }
                reads
            })
        };

        writer.join().unwrap();
        reader.join().unwrap();

        // After all writes, we should have some complete GOPs
        assert!(cache.complete_count() > 0);
    }
}
