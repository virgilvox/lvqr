//! HLS / LL-HLS manifest types and text renderer.
//!
//! The types in this module model an RFC 8216 media playlist plus the
//! LL-HLS extensions from Apple's 2020 draft. The renderer emits a
//! UTF-8 playlist compatible with `hls.js` 1.5+, Safari, and (when
//! the conformance slot is wired in) Apple's `mediastreamvalidator`.

use std::fmt::Write;
use std::time::Duration;

use lvqr_cmaf::CmafChunk;

/// Errors produced by the playlist builder.
#[derive(Debug, thiserror::Error)]
pub enum HlsError {
    /// A chunk arrived with a DTS earlier than the last chunk the
    /// builder has already consumed. HLS playlists are strictly
    /// monotonic in media sequence; going backwards is not
    /// recoverable without a discontinuity, which the day-one
    /// scaffold does not support.
    #[error("non-monotonic DTS: chunk dts {chunk_dts} < last dts {last_dts}")]
    NonMonotonic { last_dts: u64, chunk_dts: u64 },
    /// A chunk arrived with zero duration. HLS requires positive
    /// durations on every part and segment; a zero-duration chunk
    /// is either a producer bug or an end-of-stream marker, and
    /// either way the builder refuses to publish it.
    #[error("chunk has zero duration")]
    ZeroDuration,
}

/// One segment in a media playlist.
///
/// A segment is a fully closed DASH-sized boundary (by default 2 s).
/// It contains one or more [`Part`] entries; the last part in a
/// segment carries `independent = true` iff it starts with a
/// keyframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// Media sequence number. Starts at `PlaylistBuilderConfig::starting_sequence`
    /// and increments by 1 for every closed segment.
    pub sequence: u64,
    /// URI of the segment file relative to the playlist.
    pub uri: String,
    /// Total duration of all parts inside this segment, in
    /// timescale ticks. Rendered as seconds at `render_manifest`
    /// time.
    pub duration_ticks: u64,
    /// Parts that make up this segment, in DTS order.
    pub parts: Vec<Part>,
}

/// One partial segment (LL-HLS `#EXT-X-PART` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// URI of the partial file relative to the playlist.
    pub uri: String,
    /// Duration of the partial in timescale ticks.
    pub duration_ticks: u64,
    /// True if a decoder can start decoding at this part without
    /// any prior part. HLS uses this to serve a low-latency
    /// subscriber a cold start without waiting for the next
    /// segment boundary.
    pub independent: bool,
}

/// `#EXT-X-SERVER-CONTROL` tag values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerControl {
    /// `CAN-BLOCK-RELOAD=YES`. Required for a server to claim LL-HLS.
    pub can_block_reload: bool,
    /// `PART-HOLD-BACK` seconds. The client will not play back
    /// closer to the live edge than this.
    pub part_hold_back: Duration,
    /// `HOLD-BACK` seconds. Maximum distance from the live edge
    /// that a non-LL client will play.
    pub hold_back: Duration,
    /// `CAN-SKIP-UNTIL` seconds, if the server supports the
    /// LL-HLS delta-playlist (`_HLS_skip=YES`) delivery
    /// directive. `None` disables delta playlists; callers still
    /// get a correct full playlist when they ask for
    /// `_HLS_skip=YES` because the renderer falls back to the
    /// full variant when the delta window is too small to be
    /// emitted. Apple recommends a value of at least
    /// `6 * TARGETDURATION`.
    pub can_skip_until: Option<Duration>,
}

impl Default for ServerControl {
    fn default() -> Self {
        // Apple's LL-HLS draft recommends PART-HOLD-BACK >= 3 *
        // target part duration and HOLD-BACK >= 3 * target segment
        // duration. With the default 200 ms part / 2 s segment
        // policy from lvqr-cmaf, that's 0.6 s and 6 s respectively.
        // CAN-SKIP-UNTIL is set to 12 s, the 6 * TARGETDURATION
        // recommendation for a 2 s target duration.
        Self {
            can_block_reload: true,
            part_hold_back: Duration::from_millis(600),
            hold_back: Duration::from_secs(6),
            can_skip_until: Some(Duration::from_secs(12)),
        }
    }
}

/// A complete in-memory HLS / LL-HLS media playlist.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// `#EXT-X-VERSION`. LL-HLS requires 9.
    pub version: u8,
    /// Track timescale. Used to convert tick durations to the
    /// seconds values HLS expects.
    pub timescale: u32,
    /// `#EXT-X-TARGETDURATION`. In seconds; HLS rounds up.
    pub target_duration_secs: u32,
    /// `#EXT-X-PART-INF:PART-TARGET`. In seconds as a float.
    pub part_target_secs: f32,
    /// `#EXT-X-SERVER-CONTROL` values.
    pub server_control: ServerControl,
    /// Media init segment URI emitted via `#EXT-X-MAP`.
    pub map_uri: String,
    /// Closed segments, oldest first.
    pub segments: Vec<Segment>,
    /// Partials that belong to the next not-yet-closed segment.
    pub preliminary_parts: Vec<Part>,
    /// URI of the next partial the server expects to emit, populated
    /// by the builder. Rendered as `#EXT-X-PRELOAD-HINT:TYPE=PART,URI="..."`
    /// so LL-HLS clients can issue a blocking request for the
    /// partial before the server has actually finished writing it.
    /// `None` before the first chunk has been pushed; always `Some`
    /// after that. Apple `mediastreamvalidator` flags low-latency
    /// playlists that omit this tag.
    pub preload_hint_uri: Option<String>,
}

impl Manifest {
    /// Render the playlist as UTF-8 text. Returns a `String` that is
    /// ready to serve as an `application/vnd.apple.mpegurl` response
    /// body.
    pub fn render(&self) -> String {
        self.render_with_skip(0)
    }

    /// Render a delta playlist that omits the first `skip_count`
    /// segments in favour of a single `#EXT-X-SKIP:SKIPPED-SEGMENTS=N`
    /// tag. Called by [`crate::server::render_playlist`] in response
    /// to a client `_HLS_skip=YES` query when the playlist is long
    /// enough for a delta update to be worth emitting. The caller
    /// decides `skip_count` via [`Self::delta_skip_count`] so the
    /// spec's "must not skip if remaining segments < 4 * target
    /// duration" clamp lives in exactly one place.
    ///
    /// `skip_count == 0` renders an ordinary full playlist; all
    /// other invariants (version, server control, media sequence,
    /// preload hint) are preserved unchanged.
    pub fn render_with_skip(&self, skip_count: usize) -> String {
        let skip_count = skip_count.min(self.segments.len());
        let mut out = String::with_capacity(512 + self.segments.len() * 128);
        let _ = writeln!(out, "#EXTM3U");
        let _ = writeln!(out, "#EXT-X-VERSION:{}", self.version);
        // Every segment the builder closes starts on a keyframe
        // (segment-kind chunks always carry an independent sample),
        // so every segment can be decoded without data from any
        // other segment. Advertising this explicitly satisfies the
        // Apple LL-HLS requirement that EXT-X-INDEPENDENT-SEGMENTS
        // be present whenever the invariant holds.
        let _ = writeln!(out, "#EXT-X-INDEPENDENT-SEGMENTS");
        let _ = writeln!(out, "#EXT-X-TARGETDURATION:{}", self.target_duration_secs);
        // EXT-X-SERVER-CONTROL line. CAN-SKIP-UNTIL is appended
        // when Some so LL-HLS clients know the server supports
        // the `_HLS_skip=YES` delivery directive.
        let _ = write!(
            out,
            "#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD={},PART-HOLD-BACK={:.3},HOLD-BACK={:.3}",
            if self.server_control.can_block_reload {
                "YES"
            } else {
                "NO"
            },
            self.server_control.part_hold_back.as_secs_f32(),
            self.server_control.hold_back.as_secs_f32(),
        );
        if let Some(skip) = self.server_control.can_skip_until {
            let _ = write!(out, ",CAN-SKIP-UNTIL={:.3}", skip.as_secs_f32());
        }
        out.push('\n');
        let _ = writeln!(out, "#EXT-X-PART-INF:PART-TARGET={:.3}", self.part_target_secs);
        let _ = writeln!(out, "#EXT-X-MAP:URI=\"{}\"", self.map_uri);
        // EXT-X-MEDIA-SEQUENCE stays pointed at the original first
        // segment per Apple spec: "The EXT-X-MEDIA-SEQUENCE is not
        // changed" in a delta playlist. The EXT-X-SKIP tag below
        // then declares how many of those original segments were
        // omitted.
        if let Some(first) = self.segments.first() {
            let _ = writeln!(out, "#EXT-X-MEDIA-SEQUENCE:{}", first.sequence);
        }
        if skip_count > 0 {
            let _ = writeln!(out, "#EXT-X-SKIP:SKIPPED-SEGMENTS={skip_count}");
        }
        for seg in &self.segments[skip_count..] {
            for part in &seg.parts {
                let _ = writeln!(
                    out,
                    "#EXT-X-PART:DURATION={:.6},URI=\"{}\"{}",
                    ticks_to_secs(part.duration_ticks, self.timescale),
                    part.uri,
                    if part.independent { ",INDEPENDENT=YES" } else { "" },
                );
            }
            let _ = writeln!(
                out,
                "#EXTINF:{:.6},\n{}",
                ticks_to_secs(seg.duration_ticks, self.timescale),
                seg.uri
            );
        }
        // Preliminary parts (the open segment): rendered after the
        // closed segments so a client walking the playlist top-down
        // encounters them in wall-clock order.
        for part in &self.preliminary_parts {
            let _ = writeln!(
                out,
                "#EXT-X-PART:DURATION={:.6},URI=\"{}\"{}",
                ticks_to_secs(part.duration_ticks, self.timescale),
                part.uri,
                if part.independent { ",INDEPENDENT=YES" } else { "" },
            );
        }
        // Preload hint for the next partial the builder expects to
        // emit. Rendered last per LL-HLS so a client walking the
        // playlist top-down encounters it immediately after the
        // trailing EXT-X-PART. `None` before the first chunk has
        // been pushed.
        if let Some(uri) = &self.preload_hint_uri {
            let _ = writeln!(out, "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"{uri}\"");
        }
        out
    }
}

/// Free-standing version of [`Manifest::render`] so callers that
/// want to inspect or mutate the rendered string do not need a
/// full `Manifest` value in a local.
pub fn render_manifest(manifest: &Manifest) -> String {
    manifest.render()
}

impl Manifest {
    /// Decide how many leading segments may be replaced by a single
    /// `#EXT-X-SKIP:SKIPPED-SEGMENTS=N` tag in response to a client
    /// `_HLS_skip=YES` directive. Returns zero when no delta
    /// playlist should be emitted: the spec forbids them when
    /// `CAN-SKIP-UNTIL` is unset, when the total playlist duration
    /// is less than `6 * TARGETDURATION`, or when the remaining
    /// non-skipped window would drop below `4 * TARGETDURATION`.
    ///
    /// The target-duration floor is enforced in seconds rather than
    /// ticks because `TARGETDURATION` is an integer-seconds HLS tag
    /// while segment durations are in the track timescale.
    pub fn delta_skip_count(&self) -> usize {
        let Some(skip_until) = self.server_control.can_skip_until else {
            return 0;
        };
        if self.timescale == 0 || self.segments.is_empty() {
            return 0;
        }
        let ts = self.timescale as u64;
        let skip_until_ticks = (skip_until.as_secs_f64() * ts as f64) as u64;
        let td_ticks = self.target_duration_secs as u64 * ts;

        let total: u64 = self.segments.iter().map(|s| s.duration_ticks).sum();
        // Apple spec 6.2.5.1: total playlist duration must be at
        // least 6 * TARGETDURATION before a delta playlist is
        // allowed at all.
        if total < 6 * td_ticks {
            return 0;
        }

        // Walk the segments oldest first; any segment that ends
        // more than `skip_until` seconds before the end of the
        // playlist is a candidate for skipping. The spec also
        // bounds the remaining-after-skip window from below at
        // `4 * TARGETDURATION`; truncate the candidate count if
        // it would cross that floor.
        let min_remaining_ticks = 4 * td_ticks;
        let mut elapsed = 0u64;
        let mut candidate = 0usize;
        for seg in &self.segments {
            elapsed += seg.duration_ticks;
            let remaining_after = total - elapsed;
            if remaining_after > skip_until_ticks && remaining_after >= min_remaining_ticks {
                candidate += 1;
            } else {
                break;
            }
        }
        candidate
    }
}

/// Configuration for [`PlaylistBuilder`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistBuilderConfig {
    /// Track timescale. Must match the `CmafChunk::dts` timescale.
    pub timescale: u32,
    /// Starting media sequence number. Bump past zero if the
    /// playlist needs to pick up where a previous LVQR instance
    /// left off (e.g. after a hot restart).
    pub starting_sequence: u64,
    /// Init segment URI for `#EXT-X-MAP`.
    pub map_uri: String,
    /// Prefix for segment / partial URIs. Segments are named
    /// `{prefix}seg-{sequence}.m4s`; partials are named
    /// `{prefix}part-{sequence}-{part_index}.m4s`.
    pub uri_prefix: String,
    /// `#EXT-X-TARGETDURATION` in seconds. Must be >= the longest
    /// segment the builder is allowed to emit.
    pub target_duration_secs: u32,
    /// `#EXT-X-PART-INF:PART-TARGET` in seconds.
    pub part_target_secs: f32,
    /// Maximum number of closed segments the builder is allowed to
    /// hold in `manifest.segments`. `None` preserves the day-one
    /// unbounded behaviour; `Some(n)` makes `close_pending_segment`
    /// evict oldest-first until the segment count is at most `n`.
    /// Evicted segment URIs (and each evicted segment's constituent
    /// part URIs) are stashed in `evicted_uris` so the server layer
    /// can purge them from its byte cache after releasing the
    /// builder lock.
    pub max_segments: Option<usize>,
}

impl Default for PlaylistBuilderConfig {
    fn default() -> Self {
        Self {
            timescale: 90_000,
            starting_sequence: 0,
            map_uri: "init.mp4".into(),
            uri_prefix: String::new(),
            target_duration_secs: 2,
            part_target_secs: 0.2,
            max_segments: None,
        }
    }
}

/// Pure state machine that consumes [`CmafChunk`] values in DTS
/// order and produces an updated [`Manifest`] after every push.
///
/// The builder holds the authoritative view of the playlist and is
/// the type the axum router will wrap in an `Arc<Mutex<...>>` when
/// it lands. Today callers can drive it directly from tests.
#[derive(Debug)]
pub struct PlaylistBuilder {
    config: PlaylistBuilderConfig,
    manifest: Manifest,
    /// Parts belonging to the segment currently being built but not
    /// yet closed. When the next `Segment`-kind chunk arrives, the
    /// builder converts these into a closed [`Segment`] entry and
    /// appends it to `manifest.segments`.
    pending_parts: Vec<Part>,
    pending_duration_ticks: u64,
    /// Media sequence number the next closed segment will carry.
    next_sequence: u64,
    /// Last-seen DTS, used for monotonicity enforcement.
    last_dts: Option<u64>,
    /// Part index inside the currently-open segment, used to build
    /// unique partial URIs.
    part_index: u32,
    /// URIs evicted by the sliding-window policy in
    /// `close_pending_segment`. The server layer drains this after
    /// every mutation and removes each entry from the byte cache so
    /// closed-segment and partial bytes do not outlive the rendered
    /// playlist. Contains both segment URIs and the constituent
    /// part URIs of every evicted segment.
    evicted_uris: Vec<String>,
}

impl PlaylistBuilder {
    pub fn new(config: PlaylistBuilderConfig) -> Self {
        let manifest = Manifest {
            version: 9,
            timescale: config.timescale,
            target_duration_secs: config.target_duration_secs,
            part_target_secs: config.part_target_secs,
            server_control: ServerControl::default(),
            map_uri: config.map_uri.clone(),
            segments: Vec::new(),
            preliminary_parts: Vec::new(),
            preload_hint_uri: None,
        };
        let next_sequence = config.starting_sequence;
        Self {
            config,
            manifest,
            pending_parts: Vec::new(),
            pending_duration_ticks: 0,
            next_sequence,
            last_dts: None,
            part_index: 0,
            evicted_uris: Vec::new(),
        }
    }

    /// Push one chunk. Returns the updated manifest view on every
    /// call so the axum router can re-serialize without an extra
    /// borrow.
    pub fn push(&mut self, chunk: &CmafChunk) -> Result<&Manifest, HlsError> {
        if chunk.duration == 0 {
            return Err(HlsError::ZeroDuration);
        }
        if let Some(last) = self.last_dts
            && chunk.dts < last
        {
            return Err(HlsError::NonMonotonic {
                last_dts: last,
                chunk_dts: chunk.dts,
            });
        }
        self.last_dts = Some(chunk.dts);

        let part = Part {
            uri: format!(
                "{}part-{}-{}.m4s",
                self.config.uri_prefix, self.next_sequence, self.part_index
            ),
            duration_ticks: chunk.duration,
            independent: chunk.kind.is_independent(),
        };
        self.part_index += 1;

        // Segment-kind chunks close the PREVIOUS segment and start a
        // new one. Partial-kind chunks append to the current open
        // segment. Everything is a partial under LL-HLS; the
        // difference is which boundary the chunk falls on.
        if chunk.kind.is_segment_start() && !self.pending_parts.is_empty() {
            self.close_pending_segment();
        }
        self.pending_parts.push(part);
        self.pending_duration_ticks += chunk.duration;

        // Refresh the manifest's view of the preliminary parts so
        // any renderer run after this call sees the latest state.
        self.manifest.preliminary_parts = self.pending_parts.clone();
        self.manifest.preload_hint_uri = Some(self.next_part_uri());

        Ok(&self.manifest)
    }

    /// Build the URI of the next partial the builder is about to
    /// emit, using the same `{prefix}part-{sequence}-{part_index}.m4s`
    /// template `push` uses. Kept private because the only caller is
    /// the builder's own `EXT-X-PRELOAD-HINT` updater, and exposing
    /// it publicly would tempt consumers to depend on the URI shape.
    fn next_part_uri(&self) -> String {
        format!(
            "{}part-{}-{}.m4s",
            self.config.uri_prefix, self.next_sequence, self.part_index
        )
    }

    /// Borrow the current manifest without pushing anything new.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Force-close the currently open segment. Useful at
    /// end-of-stream and at hot-restart time; normal operation
    /// closes segments automatically when the next `Segment` chunk
    /// arrives.
    pub fn close_pending_segment(&mut self) {
        if self.pending_parts.is_empty() {
            return;
        }
        let sequence = self.next_sequence;
        let uri = format!("{}seg-{}.m4s", self.config.uri_prefix, sequence);
        let seg = Segment {
            sequence,
            uri,
            duration_ticks: self.pending_duration_ticks,
            parts: std::mem::take(&mut self.pending_parts),
        };
        self.manifest.segments.push(seg);
        self.pending_duration_ticks = 0;
        self.next_sequence += 1;
        self.part_index = 0;
        self.manifest.preliminary_parts.clear();
        // The next partial will land in a fresh segment, so update
        // the preload hint so a client that polls the playlist
        // immediately after a segment boundary still knows which
        // URI to pre-fetch.
        self.manifest.preload_hint_uri = Some(self.next_part_uri());

        // Sliding-window eviction: if the builder is configured
        // with a bounded segment window, drain any overflow from
        // the front of `segments`. Each evicted segment's own URI
        // plus every part URI it carries is pushed into
        // `evicted_uris` for the server layer to purge from the
        // byte cache after the builder lock drops.
        if let Some(max) = self.config.max_segments
            && self.manifest.segments.len() > max
        {
            let overflow = self.manifest.segments.len() - max;
            for dropped in self.manifest.segments.drain(..overflow) {
                for p in &dropped.parts {
                    self.evicted_uris.push(p.uri.clone());
                }
                self.evicted_uris.push(dropped.uri);
            }
        }
    }

    /// Drain the URIs the sliding-window eviction has queued since
    /// the last call. The server layer calls this after every push
    /// / close so the cache and the rendered playlist stay in
    /// lock-step. Returns segment URIs and the part URIs that lived
    /// inside each evicted segment, in eviction order.
    pub fn drain_evicted_uris(&mut self) -> Vec<String> {
        std::mem::take(&mut self.evicted_uris)
    }
}

/// Convert a tick count in the playlist's timescale to fractional
/// seconds for `#EXTINF` / `#EXT-X-PART:DURATION`.
fn ticks_to_secs(ticks: u64, timescale: u32) -> f64 {
    if timescale == 0 {
        return 0.0;
    }
    ticks as f64 / timescale as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use lvqr_cmaf::{CmafChunk, CmafChunkKind};

    fn mk_chunk(dts: u64, duration: u64, kind: CmafChunkKind) -> CmafChunk {
        CmafChunk {
            track_id: "0.mp4".into(),
            payload: Bytes::from_static(b""),
            dts,
            duration,
            kind,
        }
    }

    #[test]
    fn builder_closes_segment_on_next_segment_chunk() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        // First chunk is always a Segment-kind (new stream).
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(30_000, 30_000, CmafChunkKind::Partial)).unwrap();
        b.push(&mk_chunk(60_000, 30_000, CmafChunkKind::Partial)).unwrap();
        // Second Segment-kind closes the prior segment.
        b.push(&mk_chunk(90_000, 30_000, CmafChunkKind::Segment)).unwrap();

        let m = b.manifest();
        assert_eq!(m.segments.len(), 1);
        assert_eq!(m.segments[0].sequence, 0);
        assert_eq!(m.segments[0].parts.len(), 3);
        assert!(m.segments[0].parts[0].independent, "first part is a keyframe");
        assert_eq!(m.preliminary_parts.len(), 1, "one pending part in the open segment");
    }

    #[test]
    fn builder_rejects_zero_duration() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        assert!(matches!(
            b.push(&mk_chunk(0, 0, CmafChunkKind::Segment)),
            Err(HlsError::ZeroDuration)
        ));
    }

    #[test]
    fn builder_rejects_non_monotonic_dts() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(100, 10, CmafChunkKind::Segment)).unwrap();
        match b.push(&mk_chunk(50, 10, CmafChunkKind::Partial)) {
            Err(HlsError::NonMonotonic {
                last_dts: 100,
                chunk_dts: 50,
            }) => {}
            other => panic!("expected NonMonotonic, got {other:?}"),
        }
    }

    #[test]
    fn render_emits_required_tags() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 180_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(180_000, 180_000, CmafChunkKind::Segment)).unwrap();
        let text = b.manifest().render();
        assert!(text.starts_with("#EXTM3U"));
        assert!(text.contains("#EXT-X-VERSION:9"));
        assert!(text.contains("#EXT-X-INDEPENDENT-SEGMENTS"));
        assert!(text.contains("#EXT-X-TARGETDURATION:2"));
        assert!(text.contains("#EXT-X-SERVER-CONTROL:CAN-BLOCK-RELOAD=YES"));
        assert!(text.contains("#EXT-X-PART-INF:PART-TARGET=0.200"));
        assert!(text.contains("#EXT-X-MAP:URI=\"init.mp4\""));
        assert!(text.contains("#EXT-X-MEDIA-SEQUENCE:0"));
        assert!(text.contains("#EXTINF:"));
        assert!(text.contains("seg-0.m4s"));
    }

    #[test]
    fn render_emits_preload_hint_after_first_chunk() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        // Before any chunk, preload hint is None and absent.
        let empty = b.manifest().render();
        assert!(!empty.contains("#EXT-X-PRELOAD-HINT"));

        // After the first Segment chunk (sequence 0, part 0): the
        // builder advances `part_index` to 1, so the next URI is
        // `part-0-1.m4s`.
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-0-1.m4s\""),
            "expected preload hint for part-0-1; got:\n{text}"
        );

        // After one more Partial chunk in the same segment: next
        // URI advances to part index 2.
        b.push(&mk_chunk(30_000, 30_000, CmafChunkKind::Partial)).unwrap();
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-0-2.m4s\""),
            "expected preload hint for part-0-2; got:\n{text}"
        );

        // Close the segment. Next hint must jump to the next
        // sequence's part 0, `part-1-0.m4s`, so a client that polls
        // immediately after the boundary still sees a valid URI.
        b.close_pending_segment();
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"part-1-0.m4s\""),
            "expected preload hint for part-1-0 after close; got:\n{text}"
        );
    }

    #[test]
    fn render_server_control_advertises_can_skip_until() {
        let b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        let text = b.manifest().render();
        assert!(
            text.contains("CAN-SKIP-UNTIL=12.000"),
            "server control must advertise 6*TARGETDURATION skip boundary; got:\n{text}"
        );
    }

    #[test]
    fn delta_skip_count_is_zero_below_spec_floor() {
        // Short playlist: 3 segments * 2 s each = 6 s total.
        // Below the 6 * TARGETDURATION = 12 s floor, so the spec
        // forbids a delta playlist.
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        for i in 0..3 {
            let dts = i * 180_000;
            b.push(&mk_chunk(dts, 180_000, CmafChunkKind::Segment)).unwrap();
        }
        // Force the last segment to close so it counts toward the
        // total duration.
        b.close_pending_segment();
        assert_eq!(b.manifest().delta_skip_count(), 0);
    }

    #[test]
    fn delta_skip_count_respects_can_skip_until_window() {
        // 10 segments * 2 s each = 20 s total. CAN-SKIP-UNTIL = 12 s
        // (default). Walk oldest first: a segment is skippable iff
        // the time remaining after its end (total - elapsed) is
        // strictly greater than CAN-SKIP-UNTIL. At i=0 the remaining
        // is 18 s, i=1 -> 16 s, i=2 -> 14 s (all > 12 s, skip);
        // at i=3 the remaining hits 12 s exactly, which is NOT
        // strictly greater so the walk stops. Three segments are
        // therefore skip candidates, and the kept window is
        // 14 s which comfortably clears the 4 * TARGETDURATION = 8 s
        // lower bound.
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        for i in 0..10 {
            let dts = i * 180_000;
            b.push(&mk_chunk(dts, 180_000, CmafChunkKind::Segment)).unwrap();
        }
        b.close_pending_segment();
        let skip = b.manifest().delta_skip_count();
        assert_eq!(skip, 3, "expected 3 segments to be skip candidates");

        // Delta render: EXT-X-SKIP tag carries the count, older
        // segment URIs are absent, newer segment URIs are present.
        let delta = b.manifest().render_with_skip(skip);
        assert!(delta.contains("#EXT-X-SKIP:SKIPPED-SEGMENTS=3"), "delta body:\n{delta}");
        assert!(!delta.contains("seg-0.m4s"), "skipped segment leaked:\n{delta}");
        assert!(!delta.contains("seg-2.m4s"), "skipped segment leaked:\n{delta}");
        assert!(delta.contains("seg-3.m4s"), "kept segment missing:\n{delta}");
        assert!(delta.contains("seg-9.m4s"), "kept segment missing:\n{delta}");
        // EXT-X-MEDIA-SEQUENCE must still point at the ORIGINAL
        // first segment sequence (0), unchanged by the delta.
        assert!(delta.contains("#EXT-X-MEDIA-SEQUENCE:0"), "delta body:\n{delta}");

        // Full render (skip == 0) still contains every segment.
        let full = b.manifest().render_with_skip(0);
        assert!(full.contains("seg-0.m4s") && full.contains("seg-9.m4s"));
        assert!(!full.contains("#EXT-X-SKIP"));
    }

    #[test]
    fn delta_skip_count_is_zero_when_can_skip_until_unset() {
        // Build a standalone Manifest so the test can exercise
        // the `can_skip_until: None` branch without routing a
        // separate constructor through `PlaylistBuilder`.
        let segments: Vec<Segment> = (0..10u64)
            .map(|i| Segment {
                sequence: i,
                uri: format!("seg-{i}.m4s"),
                duration_ticks: 180_000,
                parts: Vec::new(),
            })
            .collect();
        let m = Manifest {
            version: 9,
            timescale: 90_000,
            target_duration_secs: 2,
            part_target_secs: 0.2,
            server_control: ServerControl {
                can_skip_until: None,
                ..ServerControl::default()
            },
            map_uri: "init.mp4".into(),
            segments,
            preliminary_parts: Vec::new(),
            preload_hint_uri: None,
        };
        assert_eq!(m.delta_skip_count(), 0);
        // Rendered server-control line must omit CAN-SKIP-UNTIL in
        // this case so a client that reads the playlist knows not
        // to issue a _HLS_skip directive against this server.
        let text = m.render();
        assert!(
            !text.contains("CAN-SKIP-UNTIL"),
            "server control should omit CAN-SKIP-UNTIL when None; got:\n{text}"
        );
    }

    #[test]
    fn preload_hint_respects_uri_prefix() {
        // The audio rendition in MultiHlsServer sets
        // `uri_prefix: "audio-"`, so the preload hint must carry
        // the same prefix or a client fetching the hint URI
        // against the audio playlist's HTTP handler will 404.
        let cfg = PlaylistBuilderConfig {
            uri_prefix: "audio-".into(),
            ..PlaylistBuilderConfig::default()
        };
        let mut b = PlaylistBuilder::new(cfg);
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"audio-part-0-1.m4s\""),
            "audio preload hint must carry the audio- prefix; got:\n{text}"
        );
    }

    #[test]
    fn sliding_window_evicts_oldest_segments_and_reports_uris() {
        let cfg = PlaylistBuilderConfig {
            max_segments: Some(3),
            ..PlaylistBuilderConfig::default()
        };
        let mut b = PlaylistBuilder::new(cfg);
        // Push 6 segments. Each Segment-kind chunk after the first
        // closes the prior segment, so the builder produces 5 closed
        // segments during the pushes; the 6th close is explicit so
        // all six segments exist before eviction math runs.
        for i in 0..6u64 {
            b.push(&mk_chunk(i * 30_000, 30_000, CmafChunkKind::Segment)).unwrap();
        }
        b.close_pending_segment();

        let m = b.manifest();
        assert_eq!(m.segments.len(), 3, "sliding window must cap at max_segments");
        assert_eq!(
            m.segments.first().unwrap().sequence,
            3,
            "oldest retained segment is sequence 3 after evicting 0..=2"
        );
        assert_eq!(
            m.segments.last().unwrap().sequence,
            5,
            "newest segment is the most recent force-closed one",
        );

        let evicted = b.drain_evicted_uris();
        // One part per segment (each Segment-kind chunk closes the
        // prior pending segment, leaving one part behind). The part
        // URI encodes the `(next_sequence, part_index)` tuple at the
        // moment `push` built the part, which is why the part index
        // inside seg-1 is 1 rather than 0: it was minted before the
        // close that advanced `next_sequence`. Order: every evicted
        // segment's parts come before the segment URI.
        assert_eq!(
            evicted,
            vec![
                "part-0-0.m4s".to_string(),
                "seg-0.m4s".to_string(),
                "part-0-1.m4s".to_string(),
                "seg-1.m4s".to_string(),
                "part-1-0.m4s".to_string(),
                "seg-2.m4s".to_string(),
            ],
        );
        // Drain is one-shot.
        assert!(b.drain_evicted_uris().is_empty());

        // Rendered playlist reflects the new head sequence.
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-MEDIA-SEQUENCE:3"),
            "rendered playlist must reflect evicted head; got:\n{text}"
        );
        assert!(!text.contains("seg-0.m4s"));
        assert!(text.contains("seg-3.m4s"));
        assert!(text.contains("seg-5.m4s"));
    }

    #[test]
    fn sliding_window_default_is_unbounded() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        for i in 0..10u64 {
            b.push(&mk_chunk(i * 30_000, 30_000, CmafChunkKind::Segment)).unwrap();
        }
        b.close_pending_segment();
        assert_eq!(b.manifest().segments.len(), 10);
        assert!(b.drain_evicted_uris().is_empty());
    }

    #[test]
    fn render_emits_independent_flag_only_on_keyframes() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(30_000, 30_000, CmafChunkKind::Partial)).unwrap();
        b.push(&mk_chunk(60_000, 30_000, CmafChunkKind::PartialIndependent))
            .unwrap();
        // Close the segment so the parts appear in the rendered
        // output.
        b.close_pending_segment();
        let text = b.manifest().render();
        // The first and third parts carry INDEPENDENT=YES; the
        // middle one does not.
        let indep_count = text.matches(",INDEPENDENT=YES").count();
        assert_eq!(indep_count, 2);
    }
}
