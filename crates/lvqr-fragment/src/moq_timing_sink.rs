//! Sibling `<broadcast>/0.timing` MoQ track for pure-MoQ
//! glass-to-glass latency sampling.
//!
//! **Phase A v1.1 #5 close-out (session 159).** The session-157 audit
//! confirmed the MoQ wire (`MoqTrackSink::push` writes
//! `frag.payload.clone()` only, `crates/lvqr-fragment/src/moq_sink.rs:99-100`)
//! carries no per-frame wall-clock anchor. HLS subscribers can already
//! push glass-to-glass samples by lifting the publisher wall-clock
//! through `#EXT-X-PROGRAM-DATE-TIME` (the
//! `@lvqr/dvr-player` SLO sampler from session 156 follow-up); pure-MoQ
//! subscribers cannot, because the wire has no manifest analog.
//!
//! This sink fixes that without touching the existing `0.mp4` wire
//! shape. For each video keyframe the producer side stamps a 16-byte
//! anchor on a sibling `<broadcast>/0.timing` MoQ track:
//!
//! ```text
//!   +----------------+----------------------+
//!   | group_id       | ingest_time_ms       |
//!   | (8 bytes LE)   | (8 bytes LE)         |
//!   +----------------+----------------------+
//! ```
//!
//! Each anchor is the only frame in its MoQ group, so a freshly-joined
//! subscriber sees the most recent anchor on the first frame.
//! Subscribers join anchors against video frames by `group_id` to
//! compute `latency_ms = now_unix_ms() - ingest_time_ms` and push
//! samples through `POST /api/v1/slo/client-sample`.
//!
//! Foreign MoQ clients ignore the unknown track name per the
//! `moq-lite` contract; the addition is purely additive.
//!
//! ## Why little-endian
//!
//! Matches `Rust`'s `u64::to_le_bytes()` ergonomics. The wire is
//! produced and consumed by LVQR-aware tools; there is no
//! cross-language byte-order convention to defer to.
//!
//! ## Wire shape evolution
//!
//! If a future session needs a different anchor shape (extra fields,
//! versioning), the additive-evolution path is a sibling track named
//! e.g. `0.timing.v2`; v1 readers ignore the unknown name and v2
//! readers use the new shape. Inline versioning (length prefix,
//! version byte) is intentionally rejected as YAGNI.

use lvqr_moq::TrackProducer;
use thiserror::Error;

/// Wire size of one anchor: `group_id` (u64 LE) + `ingest_time_ms`
/// (u64 LE) = 16 bytes. Subscribers can validate frame length
/// against this constant before parsing.
pub const TIMING_ANCHOR_SIZE: usize = 16;

/// Reserved track name for the sibling timing track, paired with
/// the existing `0.mp4` video-track convention. Foreign MoQ clients
/// ignore unknown track names per the `moq-lite` contract, so the
/// presence of this track is non-breaking for any subscriber that
/// does not opt in.
pub const TIMING_TRACK_NAME: &str = "0.timing";

/// One timing anchor pairing a wire-side MoQ group sequence with the
/// publisher's UNIX wall-clock at ingest. The sink emits these in
/// big-endian-ascending `group_id` order; the subscriber-side join
/// helper at `lvqr_test_utils::timing_anchor::TimingAnchorJoin`
/// holds the most-recent anchors in a ring buffer and looks them up
/// by `group_id`.
///
/// `ingest_time_ms` is identical in semantics to
/// [`crate::Fragment::ingest_time_ms`]: UNIX wall-clock milliseconds
/// stamped at the moment the publisher's frame entered the ingest
/// path. A value of `0` means unset and the producer-side wiring
/// drops the push (a `(group_id, 0)` anchor would compute 60-year
/// latency on the subscriber side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingAnchor {
    pub group_id: u64,
    pub ingest_time_ms: u64,
}

impl TimingAnchor {
    /// Encode the anchor as a 16-byte LE payload. Matches the wire
    /// shape consumers parse via [`Self::decode`].
    pub fn encode(&self) -> [u8; TIMING_ANCHOR_SIZE] {
        let mut buf = [0u8; TIMING_ANCHOR_SIZE];
        buf[0..8].copy_from_slice(&self.group_id.to_le_bytes());
        buf[8..16].copy_from_slice(&self.ingest_time_ms.to_le_bytes());
        buf
    }

    /// Decode a 16-byte LE payload into a [`TimingAnchor`]. Returns
    /// `None` when the slice length does not match
    /// [`TIMING_ANCHOR_SIZE`].
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != TIMING_ANCHOR_SIZE {
            return None;
        }
        let group_id = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let ingest_time_ms = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        Some(Self {
            group_id,
            ingest_time_ms,
        })
    }
}

/// Errors produced by the timing sink. Mirrors [`crate::MoqSinkError`]
/// shape so the bridge can use the same error-handling pattern on
/// both sinks.
#[derive(Debug, Error)]
pub enum TimingSinkError {
    #[error("failed to append timing group: {0}")]
    AppendGroup(String),
    #[error("failed to write timing frame: {0}")]
    WriteFrame(String),
}

/// Sink that emits one MoQ group per anchor on a sibling
/// `<broadcast>/0.timing` track.
///
/// Lifecycle: the producer-side bridge constructs one sink per
/// broadcast at the same lifecycle hook that creates the `0.mp4`
/// video track, then calls [`Self::push_anchor`] on every video
/// keyframe (in lockstep with [`crate::MoqTrackSink::push`] on the
/// video sink). Both sinks call `track.append_group()` once per
/// keyframe so the auto-incrementing MoQ group sequences align by
/// construction.
pub struct MoqTimingTrackSink {
    track: TrackProducer,
}

impl MoqTimingTrackSink {
    /// Build a timing sink around a freshly-created
    /// [`TrackProducer`]. The caller owns track creation; this sink
    /// only drives appends.
    pub fn new(track: TrackProducer) -> Self {
        Self { track }
    }

    /// Emit one timing anchor as a single-frame MoQ group on the
    /// underlying track. The frame payload is the 16-byte LE
    /// encoding of `(group_id, ingest_time_ms)`.
    ///
    /// Returns `Ok(())` on success. Errors propagate from
    /// `moq-lite`'s underlying append / write paths and indicate the
    /// MoQ session is unhealthy; callers may log and continue
    /// (subsequent keyframes will retry).
    pub fn push_anchor(&mut self, group_id: u64, ingest_time_ms: u64) -> Result<(), TimingSinkError> {
        let anchor = TimingAnchor {
            group_id,
            ingest_time_ms,
        };
        let mut group = self
            .track
            .append_group()
            .map_err(|e| TimingSinkError::AppendGroup(format!("{e:?}")))?;
        group
            .write_frame(bytes::Bytes::copy_from_slice(&anchor.encode()))
            .map_err(|e| TimingSinkError::WriteFrame(format!("{e:?}")))?;
        // Each anchor is its own group; finish immediately so a
        // freshly-joined subscriber sees the most recent anchor on
        // the first read.
        let _ = group.finish();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lvqr_moq::{OriginProducer, Track};

    #[test]
    fn anchor_encode_round_trips() {
        let anchor = TimingAnchor {
            group_id: 0x0102_0304_0506_0708,
            ingest_time_ms: 0x1112_1314_1516_1718,
        };
        let bytes = anchor.encode();
        // First 8 bytes are group_id LE, next 8 are ingest_time_ms LE.
        assert_eq!(&bytes[0..8], &[0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(&bytes[8..16], &[0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11]);
        let decoded = TimingAnchor::decode(&bytes).expect("decode");
        assert_eq!(decoded, anchor);
    }

    #[test]
    fn anchor_decode_rejects_wrong_length() {
        assert!(TimingAnchor::decode(&[0u8; 15]).is_none());
        assert!(TimingAnchor::decode(&[0u8; 17]).is_none());
        assert!(TimingAnchor::decode(&[]).is_none());
    }

    #[tokio::test]
    async fn push_anchor_writes_one_frame_per_group_through_real_moq() {
        // Build a real moq-lite origin + broadcast + timing track and
        // push three anchors. Drain the consumer side to verify each
        // anchor lives in its own group with a 16-byte LE payload.
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("timing-test").expect("create broadcast");
        let track = broadcast
            .create_track(Track::new(TIMING_TRACK_NAME))
            .expect("create timing track");

        let mut sink = MoqTimingTrackSink::new(track);
        sink.push_anchor(1, 1_700_000_001_000).expect("push 1");
        sink.push_anchor(2, 1_700_000_002_000).expect("push 2");
        sink.push_anchor(3, 1_700_000_003_000).expect("push 3");

        let consumer = origin.consume();
        let bc = consumer.consume_broadcast("timing-test").expect("consume broadcast");
        let mut track_consumer = bc.subscribe_track(&Track::new(TIMING_TRACK_NAME)).expect("subscribe");

        for expected in [
            TimingAnchor {
                group_id: 1,
                ingest_time_ms: 1_700_000_001_000,
            },
            TimingAnchor {
                group_id: 2,
                ingest_time_ms: 1_700_000_002_000,
            },
            TimingAnchor {
                group_id: 3,
                ingest_time_ms: 1_700_000_003_000,
            },
        ] {
            let mut group = track_consumer
                .next_group()
                .await
                .expect("next group ok")
                .expect("group present");
            let frame: bytes::Bytes = group.read_frame().await.expect("frame ok").expect("frame present");
            assert_eq!(frame.len(), TIMING_ANCHOR_SIZE);
            let decoded = TimingAnchor::decode(&frame).expect("decode");
            assert_eq!(decoded, expected);
            // Group is single-framed: next read returns None.
            let extra = group.read_frame().await.expect("read after frame ok");
            assert!(extra.is_none(), "timing group should contain exactly one frame");
        }
    }

    #[test]
    fn timing_track_name_constant_matches_convention() {
        // Pin the public constant: subscribers and tests rely on the
        // exact string "0.timing".
        assert_eq!(TIMING_TRACK_NAME, "0.timing");
        assert_eq!(TIMING_ANCHOR_SIZE, 16);
    }
}
