//! MoQ projection: consume [`Fragment`] values, push them into a
//! `lvqr_moq::TrackProducer`.
//!
//! This is the first concrete Fragment projection in the codebase and the
//! one the RTMP bridge uses today. The mapping is:
//!
//! * Every fragment with `flags.keyframe == true` opens a new MoQ group.
//! * If [`FragmentMeta::init_segment`] is set, it is written as the first
//!   frame of every new group. Late-joining subscribers then receive the
//!   init segment before any keyframe they decode.
//! * Fragments with `flags.keyframe == false` write into the current group
//!   if one is open, and are dropped with a debug log otherwise (no open
//!   group means no subscriber would be able to decode them anyway).
//! * Dropping the sink finishes any open group.
//!
//! This is deliberately the *minimum* projection logic. CMAF segmentation
//! (per-chunk duration policy, partial-segment alignment for LL-HLS) lives
//! in the future `lvqr-cmaf` crate and is layered above the sink, not
//! inside it.

use crate::fragment::{Fragment, FragmentMeta};
use lvqr_moq::{GroupProducer, TrackProducer};
use thiserror::Error;
use tracing::debug;

#[derive(Debug, Error)]
pub enum MoqSinkError {
    #[error("failed to append moq group: {0}")]
    AppendGroup(String),
    #[error("failed to write moq frame: {0}")]
    WriteFrame(String),
}

/// Sink that converts a stream of [`Fragment`] values into MoQ group/frame
/// writes on a single [`TrackProducer`].
///
/// Typical lifecycle:
///
/// ```ignore
/// let meta = FragmentMeta::new("avc1.640028", 90000)
///     .with_init_segment(init_bytes);
/// let mut sink = MoqTrackSink::new(track_producer, meta);
/// sink.push(&keyframe_fragment)?;   // opens group, writes init, writes payload
/// sink.push(&delta_fragment)?;      // writes into same group
/// sink.push(&next_keyframe)?;       // finishes old group, opens new one
/// drop(sink);                        // finishes the last group
/// ```
pub struct MoqTrackSink {
    track: TrackProducer,
    meta: FragmentMeta,
    current_group: Option<GroupProducer>,
}

impl MoqTrackSink {
    pub fn new(track: TrackProducer, meta: FragmentMeta) -> Self {
        Self {
            track,
            meta,
            current_group: None,
        }
    }

    /// Metadata the sink was constructed with. Useful for debug logging.
    pub fn meta(&self) -> &FragmentMeta {
        &self.meta
    }

    /// Replace the init segment carried on metadata. The RTMP bridge calls
    /// this once the FLV sequence header has been parsed, since the init
    /// segment bytes are not known until after the TrackProducer already
    /// exists. Subsequent keyframe pushes will prepend this bytes blob as
    /// frame 0 of each new MoQ group.
    pub fn set_init_segment(&mut self, init: bytes::Bytes) {
        self.meta.init_segment = Some(init);
    }

    /// Write one fragment to the underlying MoQ track.
    ///
    /// A keyframe always opens a new group. A non-keyframe writes into the
    /// current group if one exists, and is dropped (with a debug log) if no
    /// group has been opened yet -- that state only happens before the first
    /// keyframe, when no subscriber could decode the fragment anyway.
    ///
    /// Returns `Ok(Some(seq))` when the call opened a new MoQ group on the
    /// underlying track (keyframe path); `seq` is the wire-side group
    /// sequence the producer just allocated. The session-159
    /// `MoqTimingTrackSink` uses this value to encode the matching anchor
    /// on the sibling `<broadcast>/0.timing` track so subscribers can
    /// join the two tracks by `group_id` without a side channel. Returns
    /// `Ok(None)` for delta-frame appends and dropped pre-keyframe deltas.
    pub fn push(&mut self, frag: &Fragment) -> Result<Option<u64>, MoqSinkError> {
        if frag.flags.keyframe {
            // Close any previous group and open a new one.
            if let Some(mut prev) = self.current_group.take() {
                let _ = prev.finish();
            }
            let mut group = self
                .track
                .append_group()
                .map_err(|e| MoqSinkError::AppendGroup(format!("{e:?}")))?;
            let group_seq = group.info.sequence;
            if let Some(init) = &self.meta.init_segment {
                group
                    .write_frame(init.clone())
                    .map_err(|e| MoqSinkError::WriteFrame(format!("{e:?}")))?;
            }
            group
                .write_frame(frag.payload.clone())
                .map_err(|e| MoqSinkError::WriteFrame(format!("{e:?}")))?;
            self.current_group = Some(group);
            Ok(Some(group_seq))
        } else if let Some(group) = self.current_group.as_mut() {
            group
                .write_frame(frag.payload.clone())
                .map_err(|e| MoqSinkError::WriteFrame(format!("{e:?}")))?;
            Ok(None)
        } else {
            debug!(
                track = %frag.track_id,
                dts = frag.dts,
                "dropping delta fragment: no open group yet"
            );
            Ok(None)
        }
    }

    /// Explicitly finish the current group. Called by `Drop` as well, but
    /// exposed so callers can force a boundary on stream end.
    pub fn finish_current_group(&mut self) {
        if let Some(mut g) = self.current_group.take() {
            let _ = g.finish();
        }
    }
}

impl Drop for MoqTrackSink {
    fn drop(&mut self) {
        self.finish_current_group();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::FragmentFlags;
    use bytes::Bytes;
    use lvqr_moq::{OriginProducer, Track};

    /// Build a sink backed by a real `OriginProducer`, push one keyframe and
    /// one delta fragment, and verify the payloads arrive on the subscriber
    /// side through the MoQ group/frame API. This exercises the real
    /// moq-lite fanout, not a mock.
    #[tokio::test]
    async fn sink_writes_keyframe_then_delta_to_real_moq() {
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("sink-test").expect("create broadcast");
        let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");

        let init = Bytes::from_static(b"init-segment-stub");
        let meta = FragmentMeta::new("avc1.640028", 90000).with_init_segment(init.clone());

        let mut sink = MoqTrackSink::new(track, meta);

        let kf = Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            0,
            0,
            3000,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(b"keyframe-payload"),
        );
        sink.push(&kf).expect("push keyframe");

        let delta = Fragment::new(
            "0.mp4",
            1,
            1,
            0,
            3000,
            3000,
            3000,
            FragmentFlags::DELTA,
            Bytes::from_static(b"delta-payload"),
        );
        sink.push(&delta).expect("push delta");

        // Finish the current group so the consumer sees it close cleanly.
        sink.finish_current_group();

        // Subscribe via the origin consumer side.
        let consumer = origin.consume();
        let bc = consumer.consume_broadcast("sink-test").expect("consume broadcast");
        let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

        let mut group = track_consumer
            .next_group()
            .await
            .expect("next group ok")
            .expect("one group");

        let f0: bytes::Bytes = group.read_frame().await.expect("frame 0 ok").expect("init");
        assert_eq!(f0.as_ref(), b"init-segment-stub");
        let f1: bytes::Bytes = group.read_frame().await.expect("frame 1 ok").expect("keyframe");
        assert_eq!(f1.as_ref(), b"keyframe-payload");
        let f2: bytes::Bytes = group.read_frame().await.expect("frame 2 ok").expect("delta");
        assert_eq!(f2.as_ref(), b"delta-payload");
    }

    #[tokio::test]
    async fn delta_without_keyframe_is_dropped() {
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("drop-test").expect("create broadcast");
        let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
        let mut sink = MoqTrackSink::new(track, FragmentMeta::new("avc1.640028", 90000));

        let delta = Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            0,
            0,
            3000,
            FragmentFlags::DELTA,
            Bytes::from_static(b"stray"),
        );
        // Must not error: the sink logs and drops.
        sink.push(&delta).expect("push delta returns ok");
    }
}
