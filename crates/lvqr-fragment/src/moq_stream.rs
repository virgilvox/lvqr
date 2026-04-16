//! MoQ -> Fragment adapters: read [`Fragment`] values out of
//! `lvqr_moq::GroupConsumer` and `lvqr_moq::TrackConsumer`.
//!
//! These are the inverse of [`MoqTrackSink`](crate::MoqTrackSink) and close
//! the symmetry the roadmap's Tier 2.1 line 154 asks for: every MoQ group we
//! subscribe to upstream can be re-projected back into the unified Fragment
//! model, so downstream consumers (archive, LL-HLS, cross-node relay fanout)
//! can treat a MoQ-sourced broadcast exactly like a locally-ingested one.
//!
//! Semantics:
//!
//! * [`MoqGroupStream`] drains a single [`lvqr_moq::GroupConsumer`] and yields
//!   one [`Fragment`] per frame. If [`FragmentMeta::init_segment`] is set on
//!   the metadata passed at construction, the first frame of the group is
//!   treated as the init segment and skipped (not re-emitted as a payload),
//!   matching what [`MoqTrackSink`](crate::MoqTrackSink) writes. The first
//!   payload frame is emitted with [`FragmentFlags::KEYFRAME`]; subsequent
//!   frames use [`FragmentFlags::DELTA`]. `group_id` is taken from
//!   `Group::sequence`; `object_id` starts at 0 and increments per emitted
//!   payload.
//!
//! * [`MoqTrackStream`] composes over a [`lvqr_moq::TrackConsumer`]. Each
//!   inner group is drained via [`MoqGroupStream`]; when one group is
//!   exhausted the stream pulls the next group from the track. The adapter
//!   produces a single flat sequence of fragments across group boundaries,
//!   which is the shape every downstream `FragmentStream` consumer already
//!   handles.
//!
//! Round-trip properties:
//!
//! * Payload bytes are preserved exactly. Every byte a producer pushes into
//!   [`MoqTrackSink::push`](crate::MoqTrackSink::push) is readable via these
//!   adapters in the same order.
//!
//! * Timestamps (`dts`, `pts`, `duration`) and `priority` are **not**
//!   preserved across the MoQ projection because [`MoqTrackSink`] does not
//!   encode them onto the wire. The adapters emit zero for these fields.
//!   Callers that need real timestamps must carry them out-of-band, e.g.
//!   through a parallel catalog track or an init segment that contains the
//!   timing edit list. This matches the existing sink-side contract; the
//!   load-bearing guarantee is payload-byte lossless, not field-identity.
//!
//! * `group_id` on emitted fragments is the MoQ group sequence number, which
//!   the sink assigns via `TrackProducer::append_group` on every keyframe and
//!   which starts at 0 for each new track. The producer-side `group_id` the
//!   original [`Fragment`] carried is lost. `object_id` is recounted from 0
//!   per group, not carried across the round trip.

use crate::fragment::{Fragment, FragmentFlags, FragmentMeta};
use crate::stream::FragmentStream;
use lvqr_moq::{GroupConsumer, TrackConsumer};
use std::future::Future;
use std::pin::Pin;
use tracing::warn;

/// [`FragmentStream`] reading from one [`lvqr_moq::GroupConsumer`].
///
/// One group maps to one sequence of fragments. The adapter terminates (returns
/// `None`) when the underlying group finishes or is aborted. Keyframe /
/// delta flagging follows the convention the sink writes: the first emitted
/// payload is a keyframe, the rest are deltas.
pub struct MoqGroupStream {
    meta: FragmentMeta,
    group: GroupConsumer,
    track_id: String,
    emitted: u64,
    expect_init_prefix: bool,
}

impl MoqGroupStream {
    /// Adapt a group consumer where the first frame is an init segment (the
    /// pattern [`MoqTrackSink`](crate::MoqTrackSink) writes when
    /// [`FragmentMeta::init_segment`] is set). The first frame is skipped on
    /// the first call to [`FragmentStream::next_fragment`]; subsequent frames
    /// are emitted as fragments.
    pub fn new(track_id: impl Into<String>, meta: FragmentMeta, group: GroupConsumer) -> Self {
        let expect_init_prefix = meta.init_segment.is_some();
        Self {
            meta,
            group,
            track_id: track_id.into(),
            emitted: 0,
            expect_init_prefix,
        }
    }

    /// Adapt a group consumer where every frame is a payload (no init-segment
    /// prefix). Useful when the caller knows the init segment is carried
    /// out-of-band (e.g. on a sibling catalog track).
    pub fn without_init_prefix(track_id: impl Into<String>, meta: FragmentMeta, group: GroupConsumer) -> Self {
        Self {
            meta,
            group,
            track_id: track_id.into(),
            emitted: 0,
            expect_init_prefix: false,
        }
    }

    /// Sequence number of the underlying MoQ group.
    pub fn group_sequence(&self) -> u64 {
        self.group.info.sequence
    }

    /// Count of fragments emitted so far.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }
}

impl FragmentStream for MoqGroupStream {
    fn meta(&self) -> &FragmentMeta {
        &self.meta
    }

    fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
        Box::pin(async move {
            if self.expect_init_prefix {
                self.expect_init_prefix = false;
                match self.group.read_frame().await {
                    Ok(Some(_init)) => {}
                    Ok(None) => return None,
                    Err(e) => {
                        warn!(error = ?e, track = %self.track_id, "MoqGroupStream: error reading init frame");
                        return None;
                    }
                }
            }
            let payload = match self.group.read_frame().await {
                Ok(Some(b)) => b,
                Ok(None) => return None,
                Err(e) => {
                    warn!(error = ?e, track = %self.track_id, "MoqGroupStream: error reading frame");
                    return None;
                }
            };
            let flags = if self.emitted == 0 {
                FragmentFlags::KEYFRAME
            } else {
                FragmentFlags::DELTA
            };
            let frag = Fragment::new(
                self.track_id.clone(),
                self.group.info.sequence,
                self.emitted,
                0,
                0,
                0,
                0,
                flags,
                payload,
            );
            self.emitted += 1;
            Some(frag)
        })
    }
}

/// [`FragmentStream`] reading across every group in one
/// [`lvqr_moq::TrackConsumer`].
///
/// Internally the adapter holds an optional [`MoqGroupStream`] for the current
/// group. When that group drains, the next group is pulled from the track
/// consumer and wrapped in a fresh inner stream. Termination happens when the
/// track itself is finished (next group returns `None` / errors).
pub struct MoqTrackStream {
    meta: FragmentMeta,
    track: TrackConsumer,
    track_id: String,
    current: Option<MoqGroupStream>,
    expect_init_per_group: bool,
}

impl MoqTrackStream {
    /// Every group on the track is expected to start with an init-segment
    /// frame. This matches what [`MoqTrackSink`](crate::MoqTrackSink) writes
    /// when [`FragmentMeta::init_segment`] is set: the init is emitted as the
    /// first frame of *every* group, so late joiners can decode on any
    /// keyframe.
    pub fn new(track_id: impl Into<String>, meta: FragmentMeta, track: TrackConsumer) -> Self {
        let expect_init_per_group = meta.init_segment.is_some();
        Self {
            meta,
            track,
            track_id: track_id.into(),
            current: None,
            expect_init_per_group,
        }
    }

    /// Adapt a track consumer where no group carries an init-segment prefix.
    pub fn without_init_prefix(track_id: impl Into<String>, meta: FragmentMeta, track: TrackConsumer) -> Self {
        Self {
            meta,
            track,
            track_id: track_id.into(),
            current: None,
            expect_init_per_group: false,
        }
    }
}

impl FragmentStream for MoqTrackStream {
    fn meta(&self) -> &FragmentMeta {
        &self.meta
    }

    fn next_fragment<'a>(&'a mut self) -> Pin<Box<dyn Future<Output = Option<Fragment>> + Send + 'a>> {
        Box::pin(async move {
            loop {
                if let Some(stream) = self.current.as_mut() {
                    if let Some(frag) = stream.next_fragment().await {
                        return Some(frag);
                    }
                    self.current = None;
                }
                let group = match self.track.next_group().await {
                    Ok(Some(g)) => g,
                    Ok(None) => return None,
                    Err(e) => {
                        warn!(error = ?e, track = %self.track_id, "MoqTrackStream: error pulling next group");
                        return None;
                    }
                };
                let inner = if self.expect_init_per_group {
                    MoqGroupStream::new(self.track_id.clone(), self.meta.clone(), group)
                } else {
                    MoqGroupStream::without_init_prefix(self.track_id.clone(), self.meta.clone(), group)
                };
                self.current = Some(inner);
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragment::FragmentFlags;
    use bytes::Bytes;
    use lvqr_moq::{OriginProducer, Track};

    /// Build a group directly (no sink), write a stub init + two payloads,
    /// then read them back via `MoqGroupStream` with `expect_init_prefix`.
    #[tokio::test]
    async fn group_stream_skips_init_prefix_and_emits_payloads() {
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("gs-init").expect("create broadcast");
        let mut track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
        let mut group = track.append_group().expect("append group");
        group.write_frame(Bytes::from_static(b"init")).expect("init");
        group.write_frame(Bytes::from_static(b"kf")).expect("kf");
        group.write_frame(Bytes::from_static(b"d1")).expect("d1");
        group.finish().expect("finish");

        let consumer = origin.consume();
        let bc = consumer.consume_broadcast("gs-init").expect("consume broadcast");
        let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");
        let group_consumer = track_consumer
            .next_group()
            .await
            .expect("next group ok")
            .expect("one group");

        let meta = FragmentMeta::new("avc1.640028", 90000).with_init_segment(Bytes::from_static(b"init"));
        let mut stream = MoqGroupStream::new("0.mp4", meta, group_consumer);

        let f0 = stream.next_fragment().await.expect("frame 0");
        assert_eq!(f0.payload.as_ref(), b"kf");
        assert!(f0.flags.keyframe, "first payload flagged as keyframe");
        assert_eq!(f0.group_id, 0, "group id from Group::sequence");
        assert_eq!(f0.object_id, 0, "object_id starts at 0");

        let f1 = stream.next_fragment().await.expect("frame 1");
        assert_eq!(f1.payload.as_ref(), b"d1");
        assert!(!f1.flags.keyframe, "subsequent payload flagged as delta");
        assert_eq!(f1.object_id, 1);

        assert!(stream.next_fragment().await.is_none(), "group drained");
    }

    /// `without_init_prefix` emits every frame as a payload.
    #[tokio::test]
    async fn group_stream_without_init_prefix_emits_first_frame() {
        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("gs-noinit").expect("create broadcast");
        let mut track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
        let mut group = track.append_group().expect("append group");
        group.write_frame(Bytes::from_static(b"first")).expect("first");
        group.write_frame(Bytes::from_static(b"second")).expect("second");
        group.finish().expect("finish");

        let consumer = origin.consume();
        let bc = consumer.consume_broadcast("gs-noinit").expect("consume broadcast");
        let mut track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");
        let group_consumer = track_consumer
            .next_group()
            .await
            .expect("next group ok")
            .expect("one group");

        let meta = FragmentMeta::new("avc1.640028", 90000);
        let mut stream = MoqGroupStream::without_init_prefix("0.mp4", meta, group_consumer);

        let f0 = stream.next_fragment().await.expect("frame 0");
        assert_eq!(f0.payload.as_ref(), b"first");
        assert!(f0.flags.keyframe);

        let f1 = stream.next_fragment().await.expect("frame 1");
        assert_eq!(f1.payload.as_ref(), b"second");
        assert!(!f1.flags.keyframe);

        assert!(stream.next_fragment().await.is_none());
    }

    /// Drive the sink with a realistic sequence and read it back via the
    /// track stream. Verifies group boundaries are preserved and the init
    /// prefix on each group is stripped exactly once.
    #[tokio::test]
    async fn track_stream_roundtrips_two_groups_through_sink() {
        use crate::MoqTrackSink;

        let origin = OriginProducer::new();
        let mut broadcast = origin.create_broadcast("ts-rt").expect("create broadcast");
        let track = broadcast.create_track(Track::new("0.mp4")).expect("create track");
        let init = Bytes::from_static(b"INIT");
        let meta = FragmentMeta::new("avc1.640028", 90000).with_init_segment(init.clone());
        let mut sink = MoqTrackSink::new(track, meta.clone());

        let kf1 = Fragment::new(
            "0.mp4",
            1,
            0,
            0,
            0,
            0,
            3000,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(b"kf1"),
        );
        let d1 = Fragment::new(
            "0.mp4",
            1,
            1,
            0,
            3000,
            3000,
            3000,
            FragmentFlags::DELTA,
            Bytes::from_static(b"d1"),
        );
        let kf2 = Fragment::new(
            "0.mp4",
            2,
            0,
            0,
            6000,
            6000,
            3000,
            FragmentFlags::KEYFRAME,
            Bytes::from_static(b"kf2"),
        );
        sink.push(&kf1).expect("push kf1");
        sink.push(&d1).expect("push d1");
        sink.push(&kf2).expect("push kf2");
        sink.finish_current_group();

        // Subscribe while the sink (and its TrackProducer) is still alive.
        // Dropping the producer removes the track from the broadcast.
        let consumer = origin.consume();
        let bc = consumer.consume_broadcast("ts-rt").expect("consume broadcast");
        let track_consumer = bc.subscribe_track(&Track::new("0.mp4")).expect("subscribe");

        let mut stream = MoqTrackStream::new("0.mp4", meta, track_consumer);

        let f = stream.next_fragment().await.expect("g1 kf");
        assert_eq!(f.payload.as_ref(), b"kf1");
        assert!(f.flags.keyframe);
        let f = stream.next_fragment().await.expect("g1 d1");
        assert_eq!(f.payload.as_ref(), b"d1");
        assert!(!f.flags.keyframe);
        let f = stream.next_fragment().await.expect("g2 kf");
        assert_eq!(f.payload.as_ref(), b"kf2");
        assert!(f.flags.keyframe, "group boundary resets keyframe flag");
    }
}
