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
    /// Wall-clock time for this segment as milliseconds since the
    /// UNIX epoch. `Some` when the builder's
    /// `PlaylistBuilderConfig::program_date_time_base` is set;
    /// `None` otherwise. Rendered as an ISO 8601
    /// `#EXT-X-PROGRAM-DATE-TIME` tag before the segment's first
    /// `#EXT-X-PART` line. RFC 8216bis requires this tag on every
    /// segment when `CAN-SKIP-UNTIL` is advertised.
    pub program_date_time_millis: Option<u64>,
    /// `true` when this segment marks a codec / init-segment / encoder
    /// boundary. Set by [`PlaylistBuilder::mark_discontinuity_pending`]
    /// (called by the HLS server when a publisher reconnects with new
    /// init bytes) and consumed by the renderer, which emits
    /// `#EXT-X-DISCONTINUITY` immediately before this segment's first
    /// `#EXT-X-PROGRAM-DATE-TIME` / `#EXT-X-PART` / `#EXTINF` lines.
    /// RFC 8216bis §4.4.4.4 requires this tag on the first segment that
    /// follows any change in codec, file format, or timestamp sequence;
    /// strict players (hls.js, Shaka) glitch or fail playback on the
    /// boundary when it is missing.
    pub discontinuity: bool,
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

/// SCTE-35 ad-marker carriage attribute on a DATERANGE entry per
/// HLS spec section 4.4.5.1 (draft-pantos-hls-rfc8216bis).
///
/// Each DATERANGE may carry exactly one of the three SCTE35-*
/// attributes; the choice is driven by the splice_command_type and
/// out_of_network_indicator on the underlying splice_info_section:
///
/// * `SpliceOut` -- splice_insert with out_of_network_indicator = 1
///   (going to ad). Renders `SCTE35-OUT="0x..."`.
/// * `SpliceIn` -- splice_insert with out_of_network_indicator = 0
///   (returning from ad). Renders `SCTE35-IN="0x..."`.
/// * `Cmd` -- everything else (splice_null, time_signal,
///   bandwidth_reservation, private_command, splice_schedule).
///   Renders `SCTE35-CMD="0x..."`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRangeKind {
    SpliceOut,
    SpliceIn,
    Cmd,
}

impl DateRangeKind {
    fn attribute_name(self) -> &'static str {
        match self {
            Self::SpliceOut => "SCTE35-OUT",
            Self::SpliceIn => "SCTE35-IN",
            Self::Cmd => "SCTE35-CMD",
        }
    }
}

/// CLASS attribute value for SCTE-35 ad-marker DATERANGE entries.
/// Industry convention (Wowza, Akamai, AWS Elemental, JW Player) so
/// client-side ad-decisioning pipelines recognise the entry as a
/// SCTE-35 marker without parsing the SCTE35-* hex blob. Per
/// SCTE 35-2024 section 12.1 + HLS spec section 4.4.5.1.3 (CLASS is
/// OPTIONAL but RECOMMENDED for SCTE-35 carriages).
pub const SCTE35_DATERANGE_CLASS: &str = "urn:scte:scte35:2014:bin";

/// One `#EXT-X-DATERANGE` entry per HLS spec section 4.4.5
/// (draft-pantos-hls-rfc8216bis). Used by LVQR to surface SCTE-35
/// splice events as in-playlist ad markers; the egress-side drain
/// task on the registry's `"scte35"` track converts each
/// `lvqr_codec::SpliceInfo` to one of these.
#[derive(Debug, Clone, PartialEq)]
pub struct DateRange {
    /// `ID` attribute. Must be unique within the playlist + date
    /// range pair. LVQR derives it from the SCTE-35 splice_event_id
    /// when available, falling back to the splice PTS when not.
    pub id: String,
    /// `CLASS` attribute. `None` omits the attribute on the wire;
    /// `Some` renders as a quoted-string. SCTE-35 ad markers should
    /// set this to [`SCTE35_DATERANGE_CLASS`] so client-side ad
    /// pipelines can filter for it.
    pub class: Option<String>,
    /// `START-DATE` attribute as wall-clock milliseconds since the
    /// UNIX epoch. Rendered as an RFC 3339 ISO 8601 string at
    /// playlist render time.
    pub start_date_millis: u64,
    /// `DURATION` attribute in seconds. `None` when the underlying
    /// splice_insert has no break_duration (and for time_signal /
    /// splice_null / etc.).
    pub duration_secs: Option<f64>,
    /// Which SCTE35-* attribute to render.
    pub kind: DateRangeKind,
    /// Raw SCTE-35 splice_info_section bytes encoded as a hex
    /// string with a leading `0x` per HLS spec 4.4.5.1. Set by the
    /// drain task from `SpliceInfo::raw`.
    pub scte35_hex: String,
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
    /// When `true` the renderer appends `#EXT-X-ENDLIST` at the
    /// bottom of the playlist. This signals to HLS clients that no
    /// more segments will appear and the playlist is final. Set by
    /// [`PlaylistBuilder::finalize`] when the broadcast ends.
    pub ended: bool,
    /// SCTE-35 ad-marker `#EXT-X-DATERANGE` entries scoped to the
    /// playlist's current segment window. Rendered after
    /// `#EXT-X-MAP` and before the first segment so a client
    /// walking top-down sees the ad-marker block before any media.
    /// Pruned in lock-step with segment eviction in
    /// [`PlaylistBuilder::close_pending_segment`].
    pub date_ranges: Vec<DateRange>,
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
        // RFC 8216bis §4.4.3.1: TARGETDURATION must be the integer
        // ceiling of the longest Media Segment in the playlist. Trust
        // the configured value as a FLOOR (operators can over-declare
        // a roomier target) but never under-declare; if a segment
        // closed slightly longer than the configured target (e.g. 2.001 s
        // when the configured target is 2 s), strict players including
        // hls.js + Apple mediastreamvalidator reject the manifest with
        // a fatal MANIFEST_PARSING_ERROR. Compute the observed ceiling
        // off `Segment.duration_ticks / timescale` and take the max.
        let observed_ceil = self
            .segments
            .iter()
            .map(|s| s.duration_ticks)
            .max()
            .map(|ticks| {
                let ts = self.timescale.max(1) as u64;
                ticks.div_ceil(ts) as u32
            })
            .unwrap_or(0);
        let effective_target = self.target_duration_secs.max(observed_ceil);
        let _ = writeln!(out, "#EXT-X-TARGETDURATION:{}", effective_target);
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
        for dr in &self.date_ranges {
            let _ = write!(out, "#EXT-X-DATERANGE:ID=\"{}\"", escape_attr(&dr.id));
            if let Some(class) = &dr.class {
                let _ = write!(out, ",CLASS=\"{}\"", escape_attr(class));
            }
            let _ = write!(
                out,
                ",START-DATE=\"{}\"",
                format_program_date_time(dr.start_date_millis),
            );
            if let Some(d) = dr.duration_secs {
                let _ = write!(out, ",DURATION={d:.3}");
            }
            let _ = writeln!(out, ",{}={}", dr.kind.attribute_name(), dr.scte35_hex);
        }
        for seg in &self.segments[skip_count..] {
            // RFC 8216bis §4.4.4.4: the discontinuity tag appears
            // BEFORE the segment's PDT / PART / EXTINF lines. Strict
            // players (hls.js, Shaka) use it to drop the audio /
            // video timestamp continuity check across the boundary,
            // which is exactly what a publisher reconnect with new
            // init bytes needs. The first segment never carries it
            // (see `pending_discontinuity` semantics in the builder).
            if seg.discontinuity {
                let _ = writeln!(out, "#EXT-X-DISCONTINUITY");
            }
            if let Some(millis) = seg.program_date_time_millis {
                let _ = writeln!(out, "#EXT-X-PROGRAM-DATE-TIME:{}", format_program_date_time(millis));
            }
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
        // Preload hint and endlist are mutually exclusive: a
        // finalized playlist has no "next partial" to hint at, and
        // an active playlist must not carry EXT-X-ENDLIST.
        if self.ended {
            let _ = writeln!(out, "#EXT-X-ENDLIST");
        } else if let Some(uri) = &self.preload_hint_uri {
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
    /// Wall-clock timestamp of the first media sample (DTS = 0) as
    /// milliseconds since the UNIX epoch. When `Some`, the builder
    /// computes an `#EXT-X-PROGRAM-DATE-TIME` for every closed
    /// segment by adding the cumulative segment-duration offset to
    /// this base. `None` omits the tag entirely.
    ///
    /// RFC 8216bis requires `EXT-X-PROGRAM-DATE-TIME` on every
    /// segment when `CAN-SKIP-UNTIL` is advertised. Callers that
    /// enable delta playlists should therefore always set this.
    pub program_date_time_base: Option<u64>,
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
            program_date_time_base: None,
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
    /// Cumulative duration of all closed segments in milliseconds,
    /// used to compute each segment's `program_date_time_millis`
    /// offset from the config base. Only meaningful when
    /// `config.program_date_time_base` is `Some`.
    cumulative_duration_millis: u64,
    /// Latch flag consumed by `close_pending_segment`. Set to `true`
    /// by [`Self::mark_discontinuity_pending`] (called by
    /// [`crate::HlsServer::push_init`] on every replacement init,
    /// i.e. publisher reconnect with new codec params), cleared after
    /// the next segment closes carrying the flag. The first init push
    /// of a stream does NOT set this; only subsequent re-pushes do.
    pending_discontinuity: bool,
}

impl PlaylistBuilder {
    pub fn new(config: PlaylistBuilderConfig) -> Self {
        let manifest = Manifest {
            version: 9,
            timescale: config.timescale,
            target_duration_secs: config.target_duration_secs,
            part_target_secs: config.part_target_secs,
            server_control: ServerControl {
                can_block_reload: true,
                part_hold_back: Duration::from_secs_f32(config.part_target_secs * 3.0),
                hold_back: Duration::from_secs(config.target_duration_secs as u64 * 3),
                can_skip_until: Some(Duration::from_secs(config.target_duration_secs as u64 * 6)),
            },
            map_uri: config.map_uri.clone(),
            segments: Vec::new(),
            preliminary_parts: Vec::new(),
            preload_hint_uri: None,
            ended: false,
            date_ranges: Vec::new(),
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
            cumulative_duration_millis: 0,
            pending_discontinuity: false,
        }
    }

    /// Mark the next-to-close segment as a discontinuity boundary.
    /// Called by [`crate::HlsServer::push_init`] when a publisher
    /// reconnects with new init bytes (different codec, different
    /// encoding parameters, different timestamp sequence). RFC
    /// 8216bis §4.4.4.4 requires the playlist to emit
    /// `#EXT-X-DISCONTINUITY` before the first Media Segment that
    /// follows the change; without it, hls.js + Shaka glitch
    /// playback at the boundary.
    ///
    /// Idempotent. Multiple calls before the next segment closes
    /// collapse to a single discontinuity. Cleared by
    /// [`Self::close_pending_segment`] after stamping it on the
    /// newly closed segment.
    pub fn mark_discontinuity_pending(&mut self) {
        self.pending_discontinuity = true;
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
        let pdt_millis = self
            .config
            .program_date_time_base
            .map(|base| base + self.cumulative_duration_millis);
        let duration_millis = if self.config.timescale > 0 {
            self.pending_duration_ticks * 1000 / self.config.timescale as u64
        } else {
            0
        };
        let discontinuity = std::mem::take(&mut self.pending_discontinuity);
        let seg = Segment {
            sequence,
            uri,
            duration_ticks: self.pending_duration_ticks,
            parts: std::mem::take(&mut self.pending_parts),
            program_date_time_millis: pdt_millis,
            discontinuity,
        };
        self.manifest.segments.push(seg);
        self.cumulative_duration_millis += duration_millis;
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
        // Prune date ranges whose START-DATE precedes the playlist's
        // earliest live PROGRAM-DATE-TIME. Only meaningful when the
        // builder is wall-clock-aligned (`program_date_time_base`).
        // Without that anchor every DATERANGE is held; the egress
        // drain task is responsible for not pushing far-stale events.
        if let Some(first_pdt) = self.manifest.segments.first().and_then(|s| s.program_date_time_millis) {
            self.manifest.date_ranges.retain(|dr| dr.start_date_millis >= first_pdt);
        }
    }

    /// Append a SCTE-35 `#EXT-X-DATERANGE` entry to the playlist.
    /// Called by the egress drain task on the registry's `"scte35"`
    /// track. Duplicates (same ID + same SCTE35-* attribute) are
    /// dropped so a publisher that re-emits the same event_id does
    /// not double-render.
    ///
    /// The pruning policy lives in [`Self::close_pending_segment`]:
    /// any entry whose `start_date_millis` precedes the playlist's
    /// earliest live `PROGRAM-DATE-TIME` ages out alongside the
    /// segment that owned it.
    pub fn push_date_range(&mut self, dr: DateRange) {
        if self
            .manifest
            .date_ranges
            .iter()
            .any(|existing| existing.id == dr.id && existing.kind == dr.kind)
        {
            return;
        }
        self.manifest.date_ranges.push(dr);
    }

    /// Drain the URIs the sliding-window eviction has queued since
    /// the last call. The server layer calls this after every push
    /// / close so the cache and the rendered playlist stay in
    /// lock-step. Returns segment URIs and the part URIs that lived
    /// inside each evicted segment, in eviction order.
    pub fn drain_evicted_uris(&mut self) -> Vec<String> {
        std::mem::take(&mut self.evicted_uris)
    }

    /// Mark the playlist as ended. Closes the pending segment (so
    /// the last few partials become a proper closed segment with
    /// coalesced bytes), clears the preload hint (there is no
    /// "next partial" after end-of-stream), and sets `ended = true`
    /// so the renderer appends `#EXT-X-ENDLIST`. Once finalized the
    /// builder still accepts `manifest()` reads but will reject
    /// further `push` calls with `HlsError::NonMonotonic` (the
    /// DTS would need to go backwards to fit into an already-closed
    /// stream). Calling `finalize()` twice is harmless.
    pub fn finalize(&mut self) {
        self.close_pending_segment();
        self.manifest.preliminary_parts.clear();
        self.manifest.preload_hint_uri = None;
        self.manifest.ended = true;
    }
}

/// Escape a string for inclusion as a quoted-string HLS attribute
/// value per RFC 8216 section 4.2: backslashes and double-quotes
/// get backslash-escaped; control characters are stripped (LF / CR
/// would terminate the tag line). The common case (alphanumeric
/// plus `-_./:`) returns unchanged.
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' | '\r' => {}
            other => out.push(other),
        }
    }
    out
}

/// Convert a tick count in the playlist's timescale to fractional
/// seconds for `#EXTINF` / `#EXT-X-PART:DURATION`.
fn ticks_to_secs(ticks: u64, timescale: u32) -> f64 {
    if timescale == 0 {
        return 0.0;
    }
    ticks as f64 / timescale as f64
}

/// Format milliseconds since the UNIX epoch as an ISO 8601 UTC
/// datetime string for `#EXT-X-PROGRAM-DATE-TIME`. Uses Howard
/// Hinnant's civil_from_days algorithm to avoid a chrono/time
/// dependency.
fn format_program_date_time(epoch_millis: u64) -> String {
    let total_secs = (epoch_millis / 1000) as i64;
    let millis = epoch_millis % 1000;
    let day_secs = total_secs.rem_euclid(86400) as u32;
    let h = day_secs / 3600;
    let min = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    let z = total_secs.div_euclid(86400) + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{millis:03}Z")
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

    fn mk_date_range(id: &str, start_ms: u64, kind: DateRangeKind) -> DateRange {
        DateRange {
            id: id.into(),
            class: Some(SCTE35_DATERANGE_CLASS.into()),
            start_date_millis: start_ms,
            duration_secs: Some(30.0),
            kind,
            scte35_hex: "0xFC301100".into(),
        }
    }

    #[test]
    fn date_range_renders_with_id_class_start_date_duration_and_scte35_attr() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        b.push_date_range(mk_date_range("splice-1", 1_700_000_000_000, DateRangeKind::SpliceOut));
        let rendered = b.manifest().render();
        assert!(
            rendered.contains("#EXT-X-DATERANGE:ID=\"splice-1\","),
            "playlist:\n{rendered}"
        );
        assert!(
            rendered.contains("CLASS=\"urn:scte:scte35:2014:bin\""),
            "playlist:\n{rendered}"
        );
        assert!(
            rendered.contains("START-DATE=\"2023-11-14T22:13:20.000Z\""),
            "playlist:\n{rendered}"
        );
        assert!(rendered.contains("DURATION=30.000"), "playlist:\n{rendered}");
        assert!(rendered.contains("SCTE35-OUT=0xFC301100"), "playlist:\n{rendered}");
    }

    #[test]
    fn date_range_omits_class_when_none() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 30_000, CmafChunkKind::Segment)).unwrap();
        b.push_date_range(DateRange {
            id: "no-class".into(),
            class: None,
            start_date_millis: 1_700_000_000_000,
            duration_secs: None,
            kind: DateRangeKind::Cmd,
            scte35_hex: "0xFC".into(),
        });
        let rendered = b.manifest().render();
        assert!(
            rendered.contains("#EXT-X-DATERANGE:ID=\"no-class\",START-DATE="),
            "playlist:\n{rendered}"
        );
        assert!(!rendered.contains("CLASS="), "no class attribute expected:\n{rendered}");
    }

    #[test]
    fn date_range_dedupe_drops_duplicate_id_and_kind() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push_date_range(mk_date_range("splice-1", 1, DateRangeKind::SpliceOut));
        b.push_date_range(mk_date_range("splice-1", 1, DateRangeKind::SpliceOut));
        assert_eq!(b.manifest().date_ranges.len(), 1);
        // Same ID with a DIFFERENT kind (the matching SCTE35-IN) is
        // a distinct render entry and is kept.
        b.push_date_range(mk_date_range("splice-1", 30_000, DateRangeKind::SpliceIn));
        assert_eq!(b.manifest().date_ranges.len(), 2);
    }

    #[test]
    fn date_range_pruned_when_segment_window_evicts_below_start_date() {
        let base = 1_700_000_000_000u64;
        let cfg = PlaylistBuilderConfig {
            max_segments: Some(3),
            program_date_time_base: Some(base),
            ..PlaylistBuilderConfig::default()
        };
        let mut b = PlaylistBuilder::new(cfg);
        // Push three 1-s segments and explicit-close so the playlist
        // currently holds segments 0, 1, 2 (max=3, no eviction yet).
        // First segment PDT = base. Push two date ranges, then push a
        // fourth segment + close so the eviction drops segment 0 and
        // first-PDT advances to base + 1000. The range whose start is
        // before that bound prunes; the later one survives.
        b.push(&mk_chunk(0, 90_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(90_000, 90_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(180_000, 90_000, CmafChunkKind::Segment)).unwrap();
        b.close_pending_segment();
        b.push_date_range(mk_date_range("old", base, DateRangeKind::Cmd));
        b.push_date_range(mk_date_range("new", base + 1_500, DateRangeKind::Cmd));
        b.push(&mk_chunk(270_000, 90_000, CmafChunkKind::Segment)).unwrap();
        b.close_pending_segment();
        assert_eq!(
            b.manifest().date_ranges.len(),
            1,
            "expected one survivor; got {:?}",
            b.manifest().date_ranges
        );
        assert_eq!(b.manifest().date_ranges[0].id, "new");
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
    fn render_emits_discontinuity_only_on_segment_after_mark() {
        // RFC 8216bis §4.4.4.4: EXT-X-DISCONTINUITY appears before the
        // first Media Segment that follows any change in codec, file
        // format, or timestamp sequence. The flag is latched by
        // PlaylistBuilder::mark_discontinuity_pending and consumed
        // exactly once on the next close. Subsequent segments without
        // a fresh mark do NOT repeat the tag.
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        // Segment 0: pre-discontinuity, must render without the tag.
        b.push(&mk_chunk(0, 90_000, CmafChunkKind::Segment)).unwrap();
        b.close_pending_segment();
        let text0 = b.manifest().render();
        assert!(
            !text0.contains("#EXT-X-DISCONTINUITY"),
            "first segment must not be a discontinuity boundary; got:\n{text0}"
        );
        // Operator (or HlsServer::push_init on reconnect) marks
        // discontinuity; the next closed segment carries it.
        b.mark_discontinuity_pending();
        b.push(&mk_chunk(90_000, 90_000, CmafChunkKind::Segment)).unwrap();
        b.close_pending_segment();
        let text1 = b.manifest().render();
        assert!(
            text1.contains("#EXT-X-DISCONTINUITY"),
            "second segment must mark discontinuity; got:\n{text1}"
        );
        // The tag must precede the second segment's URI line, not
        // the first. Locate the discontinuity marker and seg-0 +
        // seg-1; assert seg-0 < discontinuity < seg-1.
        let disc = text1.find("#EXT-X-DISCONTINUITY").expect("discontinuity present");
        let s0 = text1.find("seg-0.m4s").expect("seg-0 present");
        let s1 = text1.find("seg-1.m4s").expect("seg-1 present");
        assert!(s0 < disc && disc < s1, "discontinuity must sit between seg-0 and seg-1");
        // Push a third segment WITHOUT another mark; the tag must
        // not repeat (the latch cleared after the first consumption).
        b.push(&mk_chunk(180_000, 90_000, CmafChunkKind::Segment)).unwrap();
        b.close_pending_segment();
        let text2 = b.manifest().render();
        assert_eq!(
            text2.matches("#EXT-X-DISCONTINUITY").count(),
            1,
            "discontinuity must not repeat on subsequent segments; got:\n{text2}"
        );
    }

    #[test]
    fn render_target_duration_ceils_observed_segment_when_over_configured() {
        // Configured target is 2 s but the encoder closes a segment at
        // 2.1 s (189_000 ticks at the default 90 kHz timescale). RFC
        // 8216bis §4.4.3.1 requires TARGETDURATION to round UP to the
        // longest segment in the playlist; under-declaring is a fatal
        // parse error in hls.js and Apple mediastreamvalidator. The
        // renderer must emit `:3` here, not the configured `:2`.
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        // Two parts inside the segment summing to 189_000 ticks
        // (105_000 + 84_000). The first carries the keyframe so the
        // segment closes on the next push.
        b.push(&mk_chunk(0, 105_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(105_000, 84_000, CmafChunkKind::Partial)).unwrap();
        // Force the segment to close so it lands in `segments`.
        b.close_pending_segment();
        let text = b.manifest().render();
        assert!(
            text.contains("#EXT-X-TARGETDURATION:3"),
            "expected ceil(2.1)=3; got:\n{text}"
        );
        // Configured floor still applies when no segment exceeds it.
        let mut b2 = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b2.push(&mk_chunk(0, 90_000, CmafChunkKind::Segment)).unwrap();
        b2.close_pending_segment();
        let text2 = b2.manifest().render();
        assert!(
            text2.contains("#EXT-X-TARGETDURATION:2"),
            "configured floor not honored; got:\n{text2}"
        );
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
                program_date_time_millis: None,
                discontinuity: false,
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
            ended: false,
            date_ranges: Vec::new(),
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
    fn render_emits_program_date_time_per_segment() {
        // 2026-04-16T00:00:00.000Z in millis since epoch.
        let base: u64 = 1_776_297_600_000;
        let cfg = PlaylistBuilderConfig {
            program_date_time_base: Some(base),
            ..PlaylistBuilderConfig::default()
        };
        let mut b = PlaylistBuilder::new(cfg);
        // Push 3 segments at 2 s each (180_000 ticks at 90 kHz).
        for i in 0..3u64 {
            b.push(&mk_chunk(i * 180_000, 180_000, CmafChunkKind::Segment)).unwrap();
        }
        b.close_pending_segment();

        let text = b.manifest().render();
        // First segment starts at the base.
        assert!(
            text.contains("#EXT-X-PROGRAM-DATE-TIME:2026-04-16T00:00:00.000Z"),
            "seg-0 PDT missing; got:\n{text}"
        );
        // Second segment starts at base + 2 s.
        assert!(
            text.contains("#EXT-X-PROGRAM-DATE-TIME:2026-04-16T00:00:02.000Z"),
            "seg-1 PDT missing; got:\n{text}"
        );
        // Third segment starts at base + 4 s.
        assert!(
            text.contains("#EXT-X-PROGRAM-DATE-TIME:2026-04-16T00:00:04.000Z"),
            "seg-2 PDT missing; got:\n{text}"
        );
        // PDT tags appear exactly 3 times (one per closed segment).
        assert_eq!(
            text.matches("#EXT-X-PROGRAM-DATE-TIME:").count(),
            3,
            "expected exactly 3 PDT tags; got:\n{text}"
        );
    }

    #[test]
    fn program_date_time_omitted_when_base_is_none() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 180_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(180_000, 180_000, CmafChunkKind::Segment)).unwrap();
        let text = b.manifest().render();
        assert!(
            !text.contains("#EXT-X-PROGRAM-DATE-TIME"),
            "PDT should not appear when base is None; got:\n{text}"
        );
    }

    #[test]
    fn format_program_date_time_known_epoch() {
        // 2026-04-16T01:30:45.123Z
        let millis = 1_776_303_045_123u64;
        let formatted = super::format_program_date_time(millis);
        assert_eq!(formatted, "2026-04-16T01:30:45.123Z");
    }

    #[test]
    fn format_program_date_time_unix_epoch() {
        assert_eq!(super::format_program_date_time(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn finalize_emits_endlist_and_suppresses_preload_hint() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 180_000, CmafChunkKind::Segment)).unwrap();
        b.push(&mk_chunk(180_000, 180_000, CmafChunkKind::Partial)).unwrap();
        // Before finalize: preload hint present, no ENDLIST.
        let pre = b.manifest().render();
        assert!(
            pre.contains("#EXT-X-PRELOAD-HINT:"),
            "preload hint must exist before finalize"
        );
        assert!(
            !pre.contains("#EXT-X-ENDLIST"),
            "ENDLIST must not appear before finalize"
        );

        b.finalize();
        let post = b.manifest().render();
        assert!(post.contains("#EXT-X-ENDLIST"), "ENDLIST must appear after finalize");
        assert!(
            !post.contains("#EXT-X-PRELOAD-HINT:"),
            "preload hint must disappear after finalize; got:\n{post}"
        );
        // The pending partials should have been closed into a segment.
        assert!(
            !b.manifest().segments.is_empty(),
            "finalize must close the pending segment"
        );
    }

    #[test]
    fn finalize_twice_is_harmless() {
        let mut b = PlaylistBuilder::new(PlaylistBuilderConfig::default());
        b.push(&mk_chunk(0, 180_000, CmafChunkKind::Segment)).unwrap();
        b.finalize();
        let text1 = b.manifest().render();
        b.finalize();
        let text2 = b.manifest().render();
        assert_eq!(text1, text2, "second finalize must not change the output");
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
