//! `SegmentRef`: the unit of record in the segment index.

use crate::ArchiveError;

/// One CMAF segment's on-disk footprint + its decode-time extent.
///
/// This is what the index maps `(broadcast, track, dts)` to. Every
/// field is authoritative: `start_dts` and `end_dts` are the real
/// boundaries of samples carried in the segment (in the track's
/// own timescale, not wallclock), `byte_offset` + `length` are the
/// exact slice of `path` that contains this segment, and
/// `keyframe_start` means the first sample of the segment is a
/// random-access point.
///
/// The `byte_offset` field lets a future writer pack many segments
/// into a single file (e.g. a rolling `.m4s` archive) and still
/// look them up individually. For today's one-file-per-segment
/// writer in `lvqr-record`, `byte_offset` is `0` and `length` is
/// the full file size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentRef {
    /// Broadcast name as the writer knew it (e.g. `live/test`).
    pub broadcast: String,
    /// Track name following the LVQR `0.mp4` / `1.mp4` convention.
    pub track: String,
    /// Monotonically increasing sequence number within the
    /// `(broadcast, track)` namespace. Not used for ordering by
    /// the index; it is retained for writer-side bookkeeping and
    /// for file-name reconstruction when a consumer wants the
    /// writer-generated segment name.
    pub segment_seq: u64,
    /// Decode timestamp of the first sample in the segment, in
    /// `timescale` units. Keys are sorted by this field.
    pub start_dts: u64,
    /// Exclusive end timestamp: the decode timestamp where the
    /// next segment picks up. `end_dts - start_dts` equals the
    /// total duration of samples in this segment, again in
    /// `timescale` units. Must be strictly greater than
    /// `start_dts`.
    pub end_dts: u64,
    /// The track's native timescale in Hz. 90000 for LVQR video,
    /// 44100 or 48000 for audio. Callers translating to wallclock
    /// use this.
    pub timescale: u32,
    /// True iff the first sample in the segment is a keyframe
    /// (AVC IDR / HEVC IDR-CRA-BLA / AAC always). DVR scrub that
    /// lands on a non-keyframe segment will need to walk backward
    /// until it finds one.
    pub keyframe_start: bool,
    /// UTF-8 filesystem path to the file containing this segment.
    /// Absolute or relative; the index stores it verbatim. If the
    /// writer moves files at rotation time, the caller is
    /// responsible for updating index rows.
    pub path: String,
    /// Byte offset within `path` where the segment begins. Zero
    /// for one-file-per-segment writers.
    pub byte_offset: u64,
    /// Length in bytes of the segment inside `path`. For
    /// one-file-per-segment writers, equal to the file size.
    pub length: u64,
}

impl SegmentRef {
    /// Encode the compound key used by the index.
    ///
    /// Layout: `[broadcast_len u16_be][broadcast_bytes][track_len
    /// u16_be][track_bytes][start_dts u64_be]`. The u16 length
    /// prefixes cap broadcast and track names at 65535 bytes,
    /// which is vastly more than any real deployment and keeps
    /// the format fixed-width enough to parse with no escaping.
    /// Big-endian `start_dts` makes byte order equal numeric
    /// order inside a single `(broadcast, track)` prefix.
    pub(crate) fn encode_key(broadcast: &str, track: &str, start_dts: u64) -> Result<Vec<u8>, ArchiveError> {
        if broadcast.len() > u16::MAX as usize {
            return Err(ArchiveError::Corrupt("broadcast name exceeds 65535 bytes".into()));
        }
        if track.len() > u16::MAX as usize {
            return Err(ArchiveError::Corrupt("track name exceeds 65535 bytes".into()));
        }
        let mut out = Vec::with_capacity(2 + broadcast.len() + 2 + track.len() + 8);
        out.extend_from_slice(&(broadcast.len() as u16).to_be_bytes());
        out.extend_from_slice(broadcast.as_bytes());
        out.extend_from_slice(&(track.len() as u16).to_be_bytes());
        out.extend_from_slice(track.as_bytes());
        out.extend_from_slice(&start_dts.to_be_bytes());
        Ok(out)
    }

    /// The first possible key for `(broadcast, track)`. Used as
    /// the lower bound on range scans when the caller wants
    /// everything for a stream.
    pub(crate) fn prefix_lower(broadcast: &str, track: &str) -> Result<Vec<u8>, ArchiveError> {
        Self::encode_key(broadcast, track, 0)
    }

    /// The first key beyond `(broadcast, track)`. Used as the
    /// upper bound on range scans. Returns a key that sorts
    /// strictly greater than any real key for the given
    /// `(broadcast, track)` pair by appending `u64::MAX + 1`'s
    /// worth of sentinel; since `start_dts` is a `u64` we cap
    /// the scan at `u64::MAX` and let the caller use
    /// `range(..=u64::MAX)` semantics.
    pub(crate) fn prefix_upper(broadcast: &str, track: &str) -> Result<Vec<u8>, ArchiveError> {
        Self::encode_key(broadcast, track, u64::MAX)
    }

    /// Encode the value body (everything except the compound key,
    /// which carries broadcast / track / start_dts).
    ///
    /// Layout: `[segment_seq u64_be][end_dts u64_be][timescale
    /// u32_be][keyframe u8][byte_offset u64_be][length
    /// u64_be][path_len u32_be][path_bytes]`. Hand-rolled to
    /// avoid a serde/bincode dependency; the format is owned by
    /// this crate and will stay stable while the on-disk schema
    /// is unversioned.
    pub(crate) fn encode_value(&self) -> Vec<u8> {
        let path_bytes = self.path.as_bytes();
        let mut out = Vec::with_capacity(8 + 8 + 4 + 1 + 8 + 8 + 4 + path_bytes.len());
        out.extend_from_slice(&self.segment_seq.to_be_bytes());
        out.extend_from_slice(&self.end_dts.to_be_bytes());
        out.extend_from_slice(&self.timescale.to_be_bytes());
        out.push(if self.keyframe_start { 1 } else { 0 });
        out.extend_from_slice(&self.byte_offset.to_be_bytes());
        out.extend_from_slice(&self.length.to_be_bytes());
        out.extend_from_slice(&(path_bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(path_bytes);
        out
    }

    /// Decode a value body previously produced by
    /// [`Self::encode_value`] plus the already-known `broadcast`,
    /// `track`, and `start_dts` (which live in the key).
    pub(crate) fn decode(broadcast: &str, track: &str, start_dts: u64, value: &[u8]) -> Result<Self, ArchiveError> {
        fn take<'a>(src: &mut &'a [u8], n: usize) -> Result<&'a [u8], ArchiveError> {
            if src.len() < n {
                return Err(ArchiveError::Corrupt(format!("need {n} bytes, have {}", src.len())));
            }
            let (head, tail) = src.split_at(n);
            *src = tail;
            Ok(head)
        }

        let mut rem = value;
        let segment_seq = u64::from_be_bytes(take(&mut rem, 8)?.try_into().unwrap());
        let end_dts = u64::from_be_bytes(take(&mut rem, 8)?.try_into().unwrap());
        let timescale = u32::from_be_bytes(take(&mut rem, 4)?.try_into().unwrap());
        let kf_byte = take(&mut rem, 1)?[0];
        if kf_byte > 1 {
            return Err(ArchiveError::Corrupt(format!("invalid keyframe byte {kf_byte}")));
        }
        let keyframe_start = kf_byte == 1;
        let byte_offset = u64::from_be_bytes(take(&mut rem, 8)?.try_into().unwrap());
        let length = u64::from_be_bytes(take(&mut rem, 8)?.try_into().unwrap());
        let path_len = u32::from_be_bytes(take(&mut rem, 4)?.try_into().unwrap()) as usize;
        let path_bytes = take(&mut rem, path_len)?;
        if !rem.is_empty() {
            return Err(ArchiveError::Corrupt(format!(
                "{} trailing bytes after path",
                rem.len()
            )));
        }
        let path = std::str::from_utf8(path_bytes)
            .map_err(|_| ArchiveError::NonUtf8Path)?
            .to_string();

        Ok(Self {
            broadcast: broadcast.to_string(),
            track: track.to_string(),
            segment_seq,
            start_dts,
            end_dts,
            timescale,
            keyframe_start,
            path,
            byte_offset,
            length,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(broadcast: &str, track: &str, start: u64, end: u64) -> SegmentRef {
        SegmentRef {
            broadcast: broadcast.to_string(),
            track: track.to_string(),
            segment_seq: 42,
            start_dts: start,
            end_dts: end,
            timescale: 90_000,
            keyframe_start: true,
            path: format!("/tmp/{broadcast}/{track}/{start}.m4s"),
            byte_offset: 0,
            length: 12345,
        }
    }

    #[test]
    fn round_trip_value_encoding() {
        let seg = sample("live/test", "0.mp4", 900_000, 990_000);
        let encoded = seg.encode_value();
        let decoded = SegmentRef::decode(&seg.broadcast, &seg.track, seg.start_dts, &encoded).expect("decode");
        assert_eq!(decoded, seg);
    }

    #[test]
    fn round_trip_value_encoding_non_keyframe() {
        let mut seg = sample("live/test", "0.mp4", 900_000, 990_000);
        seg.keyframe_start = false;
        let encoded = seg.encode_value();
        let decoded = SegmentRef::decode(&seg.broadcast, &seg.track, seg.start_dts, &encoded).expect("decode");
        assert_eq!(decoded, seg);
    }

    #[test]
    fn decode_rejects_invalid_keyframe_byte() {
        let seg = sample("live/test", "0.mp4", 900_000, 990_000);
        let mut encoded = seg.encode_value();
        // keyframe byte sits at offset 8 + 8 + 4 = 20.
        encoded[20] = 0x42;
        let err = SegmentRef::decode(&seg.broadcast, &seg.track, seg.start_dts, &encoded).unwrap_err();
        assert!(matches!(err, ArchiveError::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let seg = sample("live/test", "0.mp4", 900_000, 990_000);
        let mut encoded = seg.encode_value();
        encoded.push(0x00);
        let err = SegmentRef::decode(&seg.broadcast, &seg.track, seg.start_dts, &encoded).unwrap_err();
        assert!(matches!(err, ArchiveError::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn decode_rejects_truncated_header() {
        let err = SegmentRef::decode("live/test", "0.mp4", 0, &[0x00; 7]).unwrap_err();
        assert!(matches!(err, ArchiveError::Corrupt(_)), "got {err:?}");
    }

    #[test]
    fn key_prefix_is_sorted_by_start_dts_within_same_stream() {
        let a = SegmentRef::encode_key("live/test", "0.mp4", 100).unwrap();
        let b = SegmentRef::encode_key("live/test", "0.mp4", 200).unwrap();
        let c = SegmentRef::encode_key("live/test", "0.mp4", 300).unwrap();
        assert!(a < b);
        assert!(b < c);
    }

    #[test]
    fn key_separates_different_broadcasts() {
        let a = SegmentRef::encode_key("live/a", "0.mp4", u64::MAX).unwrap();
        let b = SegmentRef::encode_key("live/b", "0.mp4", 0).unwrap();
        assert!(a < b, "live/a sorts strictly before live/b regardless of dts");
    }

    #[test]
    fn key_separates_different_tracks() {
        let a = SegmentRef::encode_key("live/test", "0.mp4", u64::MAX).unwrap();
        let b = SegmentRef::encode_key("live/test", "1.mp4", 0).unwrap();
        assert!(a < b, "0.mp4 sorts strictly before 1.mp4 regardless of dts");
    }

    #[test]
    fn key_rejects_oversized_broadcast() {
        let giant = "x".repeat(70_000);
        let err = SegmentRef::encode_key(&giant, "0.mp4", 0).unwrap_err();
        assert!(matches!(err, ArchiveError::Corrupt(_)));
    }
}
