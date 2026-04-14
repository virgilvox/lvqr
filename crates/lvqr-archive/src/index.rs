//! `SegmentIndex` trait and its redb-backed implementation.

use std::path::Path;
use std::sync::Arc;

use redb::{Database, TableDefinition};

use crate::{ArchiveError, SegmentRef};

/// Abstract segment index. Exists so tests and future in-memory
/// implementations can plug in without dragging redb's lifetime
/// model through callers. The redb impl is the only one that
/// ships today.
///
/// Every method is synchronous. DVR scrub callers running inside a
/// tokio runtime should wrap index operations in
/// `tokio::task::spawn_blocking`.
pub trait SegmentIndex: Send + Sync {
    /// Record a new segment. Overwrites any existing row with the
    /// same `(broadcast, track, start_dts)` key; the writer is
    /// expected to supply monotonically increasing `start_dts`
    /// values within a stream, so collisions are only possible on
    /// a crash-recovery replay where the idempotent overwrite is
    /// the desired behavior.
    fn record(&self, seg: &SegmentRef) -> Result<(), ArchiveError>;

    /// Return every segment for `(broadcast, track)` whose decode
    /// extent `[start_dts, end_dts)` overlaps `[query_start,
    /// query_end)`. Segments that start before `query_start` but
    /// extend into the window are included; segments that start at
    /// or after `query_end` are excluded.
    ///
    /// Result is ordered by `start_dts` ascending. An empty vec
    /// means the stream exists but has no overlap; it is not an
    /// error.
    fn find_range(
        &self,
        broadcast: &str,
        track: &str,
        query_start: u64,
        query_end: u64,
    ) -> Result<Vec<SegmentRef>, ArchiveError>;

    /// Return the segment with the largest `start_dts` for
    /// `(broadcast, track)`. Used by "live" DVR clients that want
    /// the most recent segment as the seeking anchor.
    fn latest(&self, broadcast: &str, track: &str) -> Result<Option<SegmentRef>, ArchiveError>;
}

/// The single redb table name. Keeping this out of a const def at
/// the module level so that a future schema migration can bump
/// the name and know at compile time which call sites refer to
/// the old schema.
const SEGMENTS_TABLE: TableDefinition<'static, &[u8], &[u8]> = TableDefinition::new("lvqr_archive_segments_v1");

/// redb-backed [`SegmentIndex`].
///
/// Opens a single redb database file and keeps the handle alive
/// for the lifetime of the struct. `Clone` gives cheap
/// multi-producer access via the inner `Arc<Database>`.
#[derive(Clone)]
pub struct RedbSegmentIndex {
    db: Arc<Database>,
}

impl RedbSegmentIndex {
    /// Open (or create) the index database at `path`. The parent
    /// directory must already exist.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ArchiveError> {
        let db = Database::create(path.as_ref()).map_err(storage)?;
        // Touch the table so the transaction machinery has the
        // schema registered on first use even if no writes land.
        {
            let write = db.begin_write().map_err(storage)?;
            {
                let _table = write.open_table(SEGMENTS_TABLE).map_err(storage)?;
            }
            write.commit().map_err(storage)?;
        }
        Ok(Self { db: Arc::new(db) })
    }

    /// Decode a key into its `(broadcast, track, start_dts)`
    /// components. Needed to reconstruct a full [`SegmentRef`] on
    /// range-scan results, since the encoded value carries only
    /// the fields not already in the key.
    fn decode_key(raw: &[u8]) -> Result<(String, String, u64), ArchiveError> {
        fn take<'a>(src: &mut &'a [u8], n: usize) -> Result<&'a [u8], ArchiveError> {
            if src.len() < n {
                return Err(ArchiveError::Corrupt(format!(
                    "key: need {n} bytes, have {}",
                    src.len()
                )));
            }
            let (head, tail) = src.split_at(n);
            *src = tail;
            Ok(head)
        }

        let mut rem = raw;
        let broadcast_len = u16::from_be_bytes(take(&mut rem, 2)?.try_into().unwrap()) as usize;
        let broadcast_bytes = take(&mut rem, broadcast_len)?;
        let broadcast = std::str::from_utf8(broadcast_bytes)
            .map_err(|_| ArchiveError::Corrupt("key broadcast not utf8".into()))?
            .to_string();
        let track_len = u16::from_be_bytes(take(&mut rem, 2)?.try_into().unwrap()) as usize;
        let track_bytes = take(&mut rem, track_len)?;
        let track = std::str::from_utf8(track_bytes)
            .map_err(|_| ArchiveError::Corrupt("key track not utf8".into()))?
            .to_string();
        let start_dts = u64::from_be_bytes(take(&mut rem, 8)?.try_into().unwrap());
        if !rem.is_empty() {
            return Err(ArchiveError::Corrupt(format!(
                "{} trailing bytes after key start_dts",
                rem.len()
            )));
        }
        Ok((broadcast, track, start_dts))
    }
}

/// Wrap any `redb::Error`-family type into [`ArchiveError::Storage`]
/// with its `Display` already collapsed to `String`. Using
/// `Display` rather than `Debug` keeps the error messages
/// presentable in logs and admin API bodies.
fn storage<E: std::fmt::Display>(e: E) -> ArchiveError {
    ArchiveError::Storage(e.to_string())
}

impl SegmentIndex for RedbSegmentIndex {
    fn record(&self, seg: &SegmentRef) -> Result<(), ArchiveError> {
        if seg.end_dts <= seg.start_dts {
            return Err(ArchiveError::Corrupt(format!(
                "segment end_dts {} must be > start_dts {}",
                seg.end_dts, seg.start_dts
            )));
        }
        let key = SegmentRef::encode_key(&seg.broadcast, &seg.track, seg.start_dts)?;
        let value = seg.encode_value();

        let write = self.db.begin_write().map_err(storage)?;
        {
            let mut table = write.open_table(SEGMENTS_TABLE).map_err(storage)?;
            table.insert(key.as_slice(), value.as_slice()).map_err(storage)?;
        }
        write.commit().map_err(storage)?;
        Ok(())
    }

    fn find_range(
        &self,
        broadcast: &str,
        track: &str,
        query_start: u64,
        query_end: u64,
    ) -> Result<Vec<SegmentRef>, ArchiveError> {
        if query_end <= query_start {
            return Ok(Vec::new());
        }
        let read = self.db.begin_read().map_err(storage)?;
        let table = read.open_table(SEGMENTS_TABLE).map_err(storage)?;

        let prefix_lo = SegmentRef::prefix_lower(broadcast, track)?;
        let prefix_hi = SegmentRef::prefix_upper(broadcast, track)?;

        // Step 1: find the greatest segment whose start_dts is
        // strictly less than query_start. If it ends after
        // query_start, it overlaps the window and must be
        // included. Range scans in redb are lower-bound inclusive
        // and upper-bound exclusive.
        let left_window_end = SegmentRef::encode_key(broadcast, track, query_start)?;
        let mut leading: Option<SegmentRef> = None;
        {
            let scan = table
                .range::<&[u8]>(prefix_lo.as_slice()..left_window_end.as_slice())
                .map_err(storage)?;
            // Iterate to the last entry of the scan (no
            // double-ended iteration assumption: just keep the
            // most recent value).
            for entry in scan {
                let (k, v) = entry.map_err(storage)?;
                let (br, tr, sdts) = Self::decode_key(k.value())?;
                let seg = SegmentRef::decode(&br, &tr, sdts, v.value())?;
                leading = Some(seg);
            }
        }
        let mut out: Vec<SegmentRef> = Vec::new();
        if let Some(seg) = leading
            && seg.end_dts > query_start
        {
            out.push(seg);
        }

        // Step 2: every segment with start_dts in [query_start,
        // query_end) is an overlap by construction.
        let window_start_key = SegmentRef::encode_key(broadcast, track, query_start)?;
        let window_end_key = SegmentRef::encode_key(broadcast, track, query_end)?;
        let _ = &prefix_hi; // suppress unused binding when the scan above short-circuits
        {
            let scan = table
                .range::<&[u8]>(window_start_key.as_slice()..window_end_key.as_slice())
                .map_err(storage)?;
            for entry in scan {
                let (k, v) = entry.map_err(storage)?;
                let (br, tr, sdts) = Self::decode_key(k.value())?;
                let seg = SegmentRef::decode(&br, &tr, sdts, v.value())?;
                out.push(seg);
            }
        }

        Ok(out)
    }

    fn latest(&self, broadcast: &str, track: &str) -> Result<Option<SegmentRef>, ArchiveError> {
        let read = self.db.begin_read().map_err(storage)?;
        let table = read.open_table(SEGMENTS_TABLE).map_err(storage)?;

        let lo = SegmentRef::prefix_lower(broadcast, track)?;
        let hi = SegmentRef::prefix_upper(broadcast, track)?;
        let scan = table.range::<&[u8]>(lo.as_slice()..=hi.as_slice()).map_err(storage)?;

        let mut latest: Option<SegmentRef> = None;
        for entry in scan {
            let (k, v) = entry.map_err(storage)?;
            let (br, tr, sdts) = Self::decode_key(k.value())?;
            let seg = SegmentRef::decode(&br, &tr, sdts, v.value())?;
            latest = Some(seg);
        }
        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh() -> (RedbSegmentIndex, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("archive.redb");
        let idx = RedbSegmentIndex::open(&db_path).expect("open");
        (idx, dir)
    }

    fn seg(broadcast: &str, track: &str, seq: u64, start: u64, end: u64) -> SegmentRef {
        SegmentRef {
            broadcast: broadcast.to_string(),
            track: track.to_string(),
            segment_seq: seq,
            start_dts: start,
            end_dts: end,
            timescale: 90_000,
            keyframe_start: seq.is_multiple_of(2),
            path: format!("/var/archive/{broadcast}/{track}/{seq:04}.m4s"),
            byte_offset: 0,
            length: 1024 * seq,
        }
    }

    #[test]
    fn record_and_latest() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        idx.record(&seg("live/a", "0.mp4", 2, 90_000, 180_000)).unwrap();
        idx.record(&seg("live/a", "0.mp4", 3, 180_000, 270_000)).unwrap();

        let latest = idx.latest("live/a", "0.mp4").unwrap().expect("latest exists");
        assert_eq!(latest.segment_seq, 3);
        assert_eq!(latest.start_dts, 180_000);
        assert_eq!(latest.end_dts, 270_000);
    }

    #[test]
    fn latest_returns_none_for_unknown_stream() {
        let (idx, _dir) = fresh();
        assert!(idx.latest("live/a", "0.mp4").unwrap().is_none());
    }

    #[test]
    fn find_range_returns_full_overlap_in_order() {
        let (idx, _dir) = fresh();
        for i in 1..=5 {
            idx.record(&seg("live/a", "0.mp4", i, (i - 1) * 90_000, i * 90_000))
                .unwrap();
        }
        // Query covers segments 2, 3, 4 by start_dts in range.
        let got = idx.find_range("live/a", "0.mp4", 90_000, 360_000).unwrap();
        assert_eq!(got.len(), 3, "got {got:#?}");
        assert_eq!(got[0].segment_seq, 2);
        assert_eq!(got[1].segment_seq, 3);
        assert_eq!(got[2].segment_seq, 4);
    }

    #[test]
    fn find_range_includes_leading_segment_that_overlaps() {
        let (idx, _dir) = fresh();
        // seg 1: [0, 90_000). seg 2: [90_000, 180_000).
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        idx.record(&seg("live/a", "0.mp4", 2, 90_000, 180_000)).unwrap();
        // Query starts inside seg 1 -- the leading seg must come back.
        let got = idx.find_range("live/a", "0.mp4", 45_000, 135_000).unwrap();
        assert_eq!(got.len(), 2, "got {got:#?}");
        assert_eq!(got[0].segment_seq, 1);
        assert_eq!(got[1].segment_seq, 2);
    }

    #[test]
    fn find_range_excludes_leading_segment_that_does_not_overlap() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        idx.record(&seg("live/a", "0.mp4", 2, 200_000, 290_000)).unwrap();
        // Query window [100_000, 180_000) is strictly after seg 1
        // ends and strictly before seg 2 starts, so we get nothing.
        let got = idx.find_range("live/a", "0.mp4", 100_000, 180_000).unwrap();
        assert!(got.is_empty(), "got {got:#?}");
    }

    #[test]
    fn find_range_does_not_cross_track_boundary() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        idx.record(&seg("live/a", "1.mp4", 1, 0, 90_000)).unwrap();
        let got = idx.find_range("live/a", "0.mp4", 0, 90_000).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].track, "0.mp4");
    }

    #[test]
    fn find_range_does_not_cross_broadcast_boundary() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        idx.record(&seg("live/b", "0.mp4", 1, 0, 90_000)).unwrap();
        let got = idx.find_range("live/a", "0.mp4", 0, 90_000).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].broadcast, "live/a");
    }

    #[test]
    fn find_range_empty_window_is_empty() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        assert!(idx.find_range("live/a", "0.mp4", 1000, 1000).unwrap().is_empty());
        assert!(idx.find_range("live/a", "0.mp4", 1000, 500).unwrap().is_empty());
    }

    #[test]
    fn record_rejects_zero_duration_segment() {
        let (idx, _dir) = fresh();
        let mut s = seg("live/a", "0.mp4", 1, 100, 100);
        s.end_dts = 100;
        let err = idx.record(&s).unwrap_err();
        assert!(matches!(err, ArchiveError::Corrupt(_)));
    }

    #[test]
    fn reopen_preserves_rows() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("archive.redb");
        {
            let idx = RedbSegmentIndex::open(&db_path).unwrap();
            idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
            idx.record(&seg("live/a", "0.mp4", 2, 90_000, 180_000)).unwrap();
        }
        let idx = RedbSegmentIndex::open(&db_path).unwrap();
        let got = idx.find_range("live/a", "0.mp4", 0, 180_000).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].segment_seq, 1);
        assert_eq!(got[1].segment_seq, 2);
    }

    #[test]
    fn record_overwrites_on_duplicate_key() {
        let (idx, _dir) = fresh();
        idx.record(&seg("live/a", "0.mp4", 1, 0, 90_000)).unwrap();
        let mut updated = seg("live/a", "0.mp4", 1, 0, 90_000);
        updated.length = 9_999;
        idx.record(&updated).unwrap();
        let got = idx.latest("live/a", "0.mp4").unwrap().unwrap();
        assert_eq!(got.length, 9_999);
    }
}
