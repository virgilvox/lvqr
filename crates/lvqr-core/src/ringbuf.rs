use bytes::Bytes;
use parking_lot::RwLock;
use std::sync::Arc;

/// A fixed-capacity circular buffer of `Bytes` entries.
///
/// Designed for high-throughput media relay: a publisher writes frames in,
/// and multiple subscribers read them out via cheap `Bytes::clone()` (ref-counted,
/// no data copy).
///
/// When the buffer is full, the oldest entry is overwritten. This is intentional:
/// for live video, old frames are useless once the next keyframe arrives.
#[derive(Debug)]
pub struct RingBuffer {
    inner: Arc<RwLock<RingInner>>,
}

#[derive(Debug)]
struct RingInner {
    /// Backing storage. Each slot holds an optional Bytes.
    slots: Vec<Option<Bytes>>,
    /// Total number of items ever written (monotonic).
    /// The write position in the ring is `write_cursor % capacity`.
    write_cursor: u64,
    /// Capacity of the ring.
    capacity: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity.
    ///
    /// Capacity determines how many frames are retained before the oldest is overwritten.
    /// For 30fps video, capacity=256 retains ~8.5 seconds of frames.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ring buffer capacity must be > 0");
        let slots = (0..capacity).map(|_| None).collect();
        Self {
            inner: Arc::new(RwLock::new(RingInner {
                slots,
                write_cursor: 0,
                capacity,
            })),
        }
    }

    /// Write a new entry to the ring buffer.
    /// Returns the sequence number assigned to this entry.
    pub fn push(&self, data: Bytes) -> u64 {
        let mut inner = self.inner.write();
        let seq = inner.write_cursor;
        let idx = (seq as usize) % inner.capacity;
        inner.slots[idx] = Some(data);
        inner.write_cursor = seq + 1;
        seq
    }

    /// Read an entry by sequence number.
    /// Returns `None` if the entry has been overwritten or never written.
    pub fn get(&self, sequence: u64) -> Option<Bytes> {
        let inner = self.inner.read();
        // Check if the sequence is still in the valid range
        if inner.write_cursor == 0 {
            return None;
        }
        let oldest_available = inner.write_cursor.saturating_sub(inner.capacity as u64);
        if sequence < oldest_available || sequence >= inner.write_cursor {
            return None;
        }
        let idx = (sequence as usize) % inner.capacity;
        inner.slots[idx].clone()
    }

    /// Returns a snapshot of all currently valid entries, from oldest to newest.
    pub fn snapshot(&self) -> Vec<Bytes> {
        let inner = self.inner.read();
        if inner.write_cursor == 0 {
            return Vec::new();
        }
        let oldest = inner.write_cursor.saturating_sub(inner.capacity as u64);
        let mut result = Vec::with_capacity((inner.write_cursor - oldest) as usize);
        for seq in oldest..inner.write_cursor {
            let idx = (seq as usize) % inner.capacity;
            if let Some(ref data) = inner.slots[idx] {
                result.push(data.clone());
            }
        }
        result
    }

    /// Current write cursor (total items ever written).
    pub fn write_cursor(&self) -> u64 {
        self.inner.read().write_cursor
    }

    /// Number of entries currently stored (may be less than capacity if not yet full).
    pub fn len(&self) -> usize {
        let inner = self.inner.read();
        std::cmp::min(inner.write_cursor as usize, inner.capacity)
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().write_cursor == 0
    }

    /// Buffer capacity.
    pub fn capacity(&self) -> usize {
        self.inner.read().capacity
    }
}

impl Clone for RingBuffer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_get() {
        let rb = RingBuffer::new(4);
        let s0 = rb.push(Bytes::from_static(b"frame0"));
        let s1 = rb.push(Bytes::from_static(b"frame1"));

        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
        assert_eq!(rb.get(0).unwrap(), Bytes::from_static(b"frame0"));
        assert_eq!(rb.get(1).unwrap(), Bytes::from_static(b"frame1"));
        assert!(rb.get(2).is_none());
    }

    #[test]
    fn wraparound_overwrites_oldest() {
        let rb = RingBuffer::new(3);
        rb.push(Bytes::from_static(b"a"));
        rb.push(Bytes::from_static(b"b"));
        rb.push(Bytes::from_static(b"c"));
        // buffer is full: [a, b, c]

        rb.push(Bytes::from_static(b"d"));
        // now: [d, b, c], oldest valid is seq=1

        assert!(rb.get(0).is_none(), "seq 0 should be overwritten");
        assert_eq!(rb.get(1).unwrap(), Bytes::from_static(b"b"));
        assert_eq!(rb.get(2).unwrap(), Bytes::from_static(b"c"));
        assert_eq!(rb.get(3).unwrap(), Bytes::from_static(b"d"));
    }

    #[test]
    fn snapshot_returns_valid_entries() {
        let rb = RingBuffer::new(3);
        rb.push(Bytes::from_static(b"a"));
        rb.push(Bytes::from_static(b"b"));
        rb.push(Bytes::from_static(b"c"));
        rb.push(Bytes::from_static(b"d"));

        let snap = rb.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0], Bytes::from_static(b"b"));
        assert_eq!(snap[1], Bytes::from_static(b"c"));
        assert_eq!(snap[2], Bytes::from_static(b"d"));
    }

    #[test]
    fn len_and_capacity() {
        let rb = RingBuffer::new(4);
        assert_eq!(rb.len(), 0);
        assert!(rb.is_empty());
        assert_eq!(rb.capacity(), 4);

        rb.push(Bytes::from_static(b"x"));
        rb.push(Bytes::from_static(b"y"));
        assert_eq!(rb.len(), 2);

        rb.push(Bytes::from_static(b"z"));
        rb.push(Bytes::from_static(b"w"));
        assert_eq!(rb.len(), 4);

        rb.push(Bytes::from_static(b"v"));
        assert_eq!(rb.len(), 4); // still 4, oldest was overwritten
    }

    #[test]
    fn concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(RingBuffer::new(1024));
        let rb_writer = rb.clone();
        let rb_reader = rb.clone();

        let writer = thread::spawn(move || {
            for i in 0..10_000u64 {
                let data = Bytes::from(i.to_le_bytes().to_vec());
                rb_writer.push(data);
            }
        });

        let reader = thread::spawn(move || {
            let mut reads = 0u64;
            // Try reading various sequences; some will succeed, some won't
            for seq in 0..10_000u64 {
                if rb_reader.get(seq).is_some() {
                    reads += 1;
                }
            }
            reads
        });

        writer.join().unwrap();
        let reads = reader.join().unwrap();
        // The reader should have gotten some reads (exact count depends on timing)
        assert!(reads > 0, "reader should have succeeded on some reads");
    }

    #[test]
    fn clone_shares_state() {
        let rb1 = RingBuffer::new(4);
        let rb2 = rb1.clone();

        rb1.push(Bytes::from_static(b"shared"));
        assert_eq!(rb2.get(0).unwrap(), Bytes::from_static(b"shared"));
    }

    #[test]
    #[should_panic(expected = "capacity must be > 0")]
    fn zero_capacity_panics() {
        RingBuffer::new(0);
    }

    #[test]
    fn write_cursor_is_monotonic() {
        let rb = RingBuffer::new(2);
        assert_eq!(rb.write_cursor(), 0);
        rb.push(Bytes::new());
        assert_eq!(rb.write_cursor(), 1);
        rb.push(Bytes::new());
        assert_eq!(rb.write_cursor(), 2);
        rb.push(Bytes::new()); // wraps, but cursor keeps going
        assert_eq!(rb.write_cursor(), 3);
    }
}
