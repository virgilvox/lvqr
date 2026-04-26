# Session 152 Briefing -- SCTE-35 ad-marker passthrough

**Date kick-off**: 2026-04-25 (locked at end of session 151; actual
implementation session 152 picks up from here).
**Predecessor**: Session 151 (lvqr-agent runner-test polling fix --
pre-existing flake surfaced by but orthogonal to session 150's
wasmtime v25 -> v43 upgrade). Default-gate tests at **1111** / 0 / 0,
admin surface at **12 route trees**, origin/main head `073c311`. Hot
config reload is feature-complete after session 149; the post-150
README "Next up" list ranks SCTE-35 passthrough at #1.

## Goal

Today operators publishing live programming through LVQR have no
in-band path for ad-marker signaling. SCTE-35 splice events injected
on the publisher side (RTMP onCuePoint `scte35-bin64` payloads;
SRT/MPEG-TS streams carrying SCTE-35 tables on a dedicated PID) reach
the relay and are silently dropped: rtmp.rs's `ServerSessionEvent`
match has a `_ => debug!` fallthrough, and ts.rs's PMT parser
discards ES descriptors and never surfaces non-A/V PIDs. Downstream
HLS and DASH egresses render no `#EXT-X-DATERANGE` and no
`<EventStream>` regardless of what the publisher sent.

After this session, both ingest paths capture the operator-supplied
splice_info_section, route it through a parallel "scte35" track on
the existing `FragmentBroadcasterRegistry`, and surface it on the LL-
HLS variant media playlists (`#EXT-X-DATERANGE` per HLS spec section
4.4.5) and on the DASH MPD (`<EventStream
schemeIdUri="urn:scte:scte35:2014:xml+bin">` at the Period level per
ISO/IEC 23009-1 G.7). Passthrough only -- the splice_info_section
binary is preserved end-to-end; the relay never interprets, replaces,
blacks out, or schedules anything based on it.

## Decisions to confirm on read-back

### 1. Event surface: parallel "scte35" track on the registry

The `FragmentBroadcasterRegistry` is keyed `(broadcast_id, track_id)`
where `track_id` is an arbitrary string (today: `"0.mp4"`, `"1.mp4"`,
and `"captions"` from the whisper agent). Captions ship as a sibling
track end-to-end: `WhisperCaptionsAgent` emits `Fragment {
track_id: "captions", codec: "wvtt", timescale: 1000, payload: bytes
}`, and `MultiHlsServer::ensure_subtitles` lazily spawns a drain task
that pulls the captions track into `SubtitlesServer` for separate
playlist rendering.

SCTE-35 events follow the same pattern with track name `"scte35"`
(reserve via doc comment on `FragmentBroadcasterRegistry`):

```rust
Fragment {
    track_id: "scte35",
    codec: "scte35",
    timescale: 90_000,         // align with video PTS clock
    pts: <splice_time PTS>,    // from splice_info_section
    duration: <break_duration or 0 if undefined>,
    payload: <raw splice_info_section bytes>,
    // ... existing Fragment fields
}
```

This is the parallel-track shape, NOT a `FragmentMeta` extension.
Reasons (locked):
* Captions prove the pattern operates cleanly; no new registry
  primitive required.
* SCTE-35 events are sparse and discrete -- bolting them onto every
  video/audio `FragmentMeta` would bloat the per-fragment hot path
  for a once-per-break payload.
* Egresses (HLS variant playlist, DASH Period EventStream) consume
  the events independently of media; a separate track preserves that
  decoupling.
* DVR windowing on the scte35 track stays independent of the video
  segment window; egress pruning logic mirrors the captions
  `SubtitlesServer::max_cues` pattern.

### 2. Parser location: `lvqr-codec/src/scte35.rs`

The `lvqr-codec` crate already hosts the project's binary parsers
(`hevc.rs` for NAL unit + SPS extraction, `aac.rs` for
AudioSpecificConfig, `ts.rs` for MPEG-TS / PAT / PMT / PES). Each
module follows a "minimum viable extraction, opaque blob to muxer"
pattern. SCTE-35 fits cleanly: parse just `splice_command_type`,
`pts_time` (if present), `break_duration` (if present), and the
event ID; keep the raw `splice_info_section` byte slice for
egress-side base64 encoding.

Rejected alternatives:
* **Standalone `crates/lvqr-scte35`.** No cross-cutting concern (no
  config surface, no auth, no provider chain) justifying a new
  crate for v1. If post-passthrough work later adds semantic
  interpretation (descriptor decoding, ad ID extraction, etc.) the
  module promotes to a crate then.
* **Inline in ingest path.** Parser is consumed by both RTMP and
  SRT paths; sharing in `lvqr-codec` avoids duplication.

Module shape:

```rust
// crates/lvqr-codec/src/scte35.rs
pub struct SpliceInfo {
    pub command_type: u8,           // splice_null=0, splice_insert=5, time_signal=6, ...
    pub pts: Option<u64>,           // 33-bit PTS at 90kHz, when present
    pub duration: Option<u64>,      // break_duration at 90kHz, when present
    pub event_id: Option<u32>,      // splice_event_id from splice_insert / splice_schedule
    pub raw: Bytes,                 // full splice_info_section (table_id .. CRC_32)
}

pub fn parse_splice_info_section(bytes: &[u8]) -> Result<SpliceInfo, CodecError>;
```

Implementation reads via the existing `bit_reader.rs` MSB-first
helper. CRC_32 is verified (a malformed event drops with a counted
warning -- see Risks). Unit tests against SCTE 35-2024 Annex A test
vectors (canonical splice_null, splice_insert, time_signal samples).

### 3. Ingest paths in scope for v1: RTMP + SRT only

**RTMP onCuePoint `scte35-bin64`.** The Adobe-spec convention used
by OBS, Wirecast, vMix, and most studio encoders: a ScriptData AMF0
packet named `onCuePoint` with an object containing `name:
"scte35-bin64"` and `data: <base64 splice_info_section>`. Today
`crates/lvqr-ingest/src/rtmp.rs` line ~360 catches the rml_rtmp
event for ScriptData under the `_ => debug!` fallthrough and drops
it. Session 152 adds an explicit arm.

* **Risk:** rml_rtmp v0.8's `ServerSessionEvent` enum may not expose
  `onCuePoint` ScriptData as a first-class variant; if it lands in
  a generic ScriptData/UnknownCommand variant, the arm still fires
  but parses the AMF0 object body itself. Step-1 of execution order
  is to confirm the rml_rtmp surface BEFORE writing the parser
  glue. If rml_rtmp does not expose it, we either bump the dep or
  parse the raw AMF0 frame from the connection layer (deferred
  decision -- driven by the surface check).

**SRT MPEG-TS PID-carried SCTE-35.** Per SCTE 35-2024 section 7,
broadcasters mux SCTE-35 on a dedicated PID with PMT stream_type
`0x86`. Today `crates/lvqr-codec/src/ts.rs:228` parses the PMT but
silently consumes ES descriptors, and `StreamType::Unknown(0x86)`
falls through `crates/lvqr-srt/src/ingest.rs:238` (drop). Session
152 adds:

* `StreamType::Scte35` variant + recognition of stream_type 0x86 in
  ts.rs.
* Public method on TsDemuxer to expose discovered scte35 PIDs (or,
  simpler, surface scte35 PES packets via the existing PesPacket
  flow with the new StreamType).
* New `process_scte35(pes)` arm in lvqr-srt's ingest dispatcher
  that decodes the splice_info_section from the PES payload and
  emits onto the registry's scte35 track.

**Deferred ingest paths (anti-scope):**
* WHIP / WebRTC -- no widely-adopted publisher convention for SCTE
  signaling over WebRTC data channels.
* RTSP -- no encoder convention; would require RDT or custom
  DESCRIBE handling.
* RTMP `onMetaData` legacy cuepoints (Adobe HDS) -- superseded by
  `scte35-bin64`, no live publisher demand.

### 4. HLS render shape: `#EXT-X-DATERANGE` per variant media playlist

Per HLS spec (draft-pantos-hls-rfc8216bis section 4.4.5), DATERANGE
tags belong in media playlists, NOT the master/multivariant
playlist. Each rendered variant media playlist (one per video
rendition; the captions playlist is unaffected) gains a
DATERANGE block at the playlist head, sourced from the active
window of scte35 events on that broadcast.

```
#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:1234
#EXT-X-MAP:URI="init.mp4"
#EXT-X-DATERANGE:ID="splice-1234567",START-DATE="2026-04-25T18:30:00.000Z",DURATION=30.0,SCTE35-OUT=0xFC301...
#EXT-X-DATERANGE:ID="splice-1234567",START-DATE="2026-04-25T18:30:30.000Z",SCTE35-IN=0xFC301...
#EXTINF:2.0,
segment-1234.m4s
...
```

Attribute mapping:
* `ID` -- stable per event_id (or PTS-derived if event_id absent).
* `START-DATE` -- PDT clock + (event PTS - playlist anchor PTS),
  rendered as RFC 3339.
* `DURATION` -- only when splice_info_section sets break_duration.
* `SCTE35-OUT` / `SCTE35-IN` -- raw splice_info_section as `0x...`
  hex per HLS spec section 4.4.5.1.
* `SCTE35-CMD` -- for splice_null / time_signal / bandwidth_reservation
  / private_command (no out/in semantics).

Window pruning: when a segment ages out of the playlist window, any
DATERANGE whose START-DATE precedes the window's earliest
PROGRAM-DATE-TIME also drops.

### 5. DASH render shape: `<EventStream>` at Period level

Per ISO/IEC 23009-1 G.7 + SCTE 35-2024 section 12.2, the canonical
SCTE-35 DASH carriage is a Period-level `EventStream` element with
`schemeIdUri="urn:scte:scte35:2014:xml+bin"` and
`<Event>` children whose body carries the base64-encoded
splice_info_section.

```xml
<Period id="1" start="PT0S">
  <EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin" timescale="90000">
    <Event presentationTime="8100000" duration="2700000" id="1234567">
      /DAvAAAAAAAA///wBQb+...AAAAAAA=
    </Event>
  </EventStream>
  <AdaptationSet contentType="video" ...>
    ...
  </AdaptationSet>
</Period>
```

Rejected: AdaptationSet-level placement. Per-Representation events
fragment the signal across renditions; Period-level is the standard
SCTE-35 placement and matches the parallel-track decoupling.

`crates/lvqr-dash/src/mpd.rs` has a comment at line ~277 already
noting EventStream insertion as a future addition. Session 152
adds:
* New `event_stream` module sibling to `segment_template`.
* `Period` struct gains `Vec<EventStream>` field.
* `Period::write` renders the EventStream(s) BEFORE the
  AdaptationSets per spec ordering.

Window pruning matches HLS: events with presentationTime before the
Period's `availabilityStartTime + window_offset` drop.

### 6. Test scope: real wire, real splice_info_section

Per CLAUDE.md, integration tests use real network connections, not
mocks. Test plan:

* **Unit (lvqr-codec/src/scte35.rs):** ~6 tests against SCTE 35-2024
  Annex A vectors (splice_null, two splice_insert variants with +
  without break_duration, time_signal, malformed CRC drop, oversize
  drop).
* **Unit (lvqr-fragment, lvqr-hls, lvqr-dash):** scte35 track
  registration, DATERANGE rendering with synthetic events, Period-
  level EventStream rendering with synthetic events.
* **Integration (lvqr-ingest, RTMP):** TestServer + ffmpeg-style
  RTMP publish that injects an onCuePoint `scte35-bin64` frame
  carrying a known Annex-A splice_insert; assert the rendered
  HLS variant playlist contains the expected DATERANGE attributes
  and the rendered MPD contains the expected EventStream/Event.
* **Integration (lvqr-srt):** TestServer + a generated MPEG-TS
  stream with a SCTE-35 PID + a known splice_insert table; assert
  the same HLS + DASH render shape.

Mocks not acceptable for the integration tier per CLAUDE.md.

### 7. Anti-scope (explicit rejections)

* **No semantic interpretation.** The relay does not decode
  splice_command_type beyond what the parser surfaces (PTS,
  duration, event_id, command type tag); no auto-blackout, no
  auto-replace, no logo burn-in, no programmatic ad-break
  triggering, no SCTE-224 ESNI / Audience overlay.
* **No SCTE-104.** The studio ingest format that pre-dates SCTE-35
  on the wire; not in any LVQR ingest path's scope.
* **No mid-segment splice handling.** Events whose PTS falls inside
  an in-flight segment surface on the next segment boundary; the
  relay does not split segments to align splices.
* **No transcoder-level mid-stream IDR insertion.** Operators who
  need IDR-aligned splices configure their upstream transcoder
  ladder accordingly.
* **No CHANGELOG / SDK shape change beyond playlist render.** TS +
  Python clients consume HLS / DASH directly via hls.js / dash.js
  / Shaka; no new admin route, no new SDK type.
* **No version bump or publish.** Workspace stays at 0.4.1; SDK
  packages stay at 0.3.2.
* **No new admin route.** The scte35 track is observable via the
  existing fragment broadcaster metrics surface.

## Execution order

1. **Author this briefing.** Done (post-151 close).

2. **Confirm rml_rtmp ScriptData surface.** Read `rml_rtmp` v0.8
   `ServerSessionEvent` enum (vendored or via cargo doc). Decide
   whether onCuePoint lands in a typed variant or in a generic
   ScriptData arm. Lock the parser glue shape before opening
   rtmp.rs.

3. **Land the parser.** New `crates/lvqr-codec/src/scte35.rs` with
   `SpliceInfo` + `parse_splice_info_section`; unit tests against
   Annex A vectors. Re-export from `lvqr-codec/src/lib.rs`.

4. **Land the registry surface.** Document the reserved `"scte35"`
   track name in `crates/lvqr-fragment/src/registry.rs`. No
   structural change required (string-keyed already).

5. **Land the SRT path.**
   * `crates/lvqr-codec/src/ts.rs`: add `StreamType::Scte35`,
     parse stream_type 0x86 in PMT, surface PesPacket through the
     existing flow.
   * `crates/lvqr-srt/src/ingest.rs`: add scte35 dispatch arm,
     decode splice_info_section, emit onto the registry's scte35
     track.

6. **Land the RTMP path.**
   * `crates/lvqr-ingest/src/rtmp.rs`: handle onCuePoint
     `scte35-bin64` ScriptData, base64-decode, call
     `parse_splice_info_section`, emit onto the registry's scte35
     track.

7. **Land the HLS render.**
   * `crates/lvqr-hls/src/manifest.rs`: PlaylistBuilder gains a
     parallel `Vec<DateRange>` window with the same eviction
     semantics as the segment window. `to_string` / render path
     emits DATERANGE entries at the playlist head.
   * Integration with the registry: an HLS-side drain task pulls
     the scte35 track into the playlist builder, mirroring the
     captions `MultiHlsServer::ensure_subtitles` shape.

8. **Land the DASH render.**
   * `crates/lvqr-dash/src/mpd.rs`: new `event_stream` module;
     `Period` gains `Vec<EventStream>`; `Period::write` renders
     EventStream(s) before AdaptationSets.
   * Integration: DASH-side drain task on the scte35 track,
     populating the Period's EventStream window.

9. **Land integration tests.**
   * `crates/lvqr-ingest/tests/scte35_rtmp_e2e.rs`: TestServer +
     RTMP publish + Annex-A splice_insert injection + HLS playlist
     + DASH MPD assertions.
   * `crates/lvqr-srt/tests/scte35_srt_e2e.rs`: TestServer + MPEG-
     TS publish (SCTE-35 on PID 0x1FFB) + same HLS + DASH
     assertions.

10. **Land docs.**
    * New `docs/scte35.md`: ingest formats accepted, render wire
      shape per egress, standards references (SCTE 35-2024,
      draft-pantos-hls-rfc8216bis section 4.4.5, ISO/IEC 23009-1
      G.7).
    * `docs/architecture.md`: update the ingest -> fragment ->
      egress diagram to show the scte35 track alongside captions.

11. **Land HANDOFF + README.**
    * README "Recently shipped" gains a session 152 entry;
      ranked Next-up #1 (SCTE-35 passthrough) flips to
      strikethrough.
    * HANDOFF session 152 close block.

12. **Push + verify CI green.**

## Risks + mitigations

* **rml_rtmp surface unknown.** ScriptData / onCuePoint exposure in
  rml_rtmp v0.8 is unverified. Mitigation: step 2 of execution
  order is the surface check; if rml_rtmp lacks the variant,
  defer RTMP to a follow-up session and ship SRT-only v1 (the
  brief's anti-scope tolerates ingest-by-ingest staging if the
  upstream library is the blocker).

* **PTS-to-PDT mapping.** SCTE-35 events arrive as 33-bit PTS at
  90 kHz; the HLS DATERANGE START-DATE attribute is wall-clock
  RFC 3339. The mapping is `pdt_anchor + (event_pts - playlist_anchor_pts)`;
  the playlist builder already maintains a PDT anchor for
  PROGRAM-DATE-TIME emission. Mitigation: reuse that anchor; unit
  test the mapping with synthetic PTS / PDT pairs.

* **CRC mismatch on inbound splice_info_section.** Malformed events
  from a buggy publisher would otherwise propagate downstream and
  break hls.js / dash.js parsers. Mitigation: parser verifies the
  CRC_32 trailer; on mismatch, drop and bump a counter
  (`lvqr_scte35_drops_total{reason="crc"}`). Integration test
  asserts the drop path.

* **Event-window pruning lag.** If the scte35 track's drain task
  falls behind the video segment eviction, stale DATERANGE / Event
  entries could linger in playlists. Mitigation: drain runs every
  segment-emit tick (driven by the same fragment-broadcaster
  signal as the segment build), bounded queue depth (drop oldest
  on overflow with a counter).

* **MPD render ordering.** ISO/IEC 23009-1 requires EventStream
  before AdaptationSet within a Period. Mitigation: explicit
  ordering in `Period::write`; unit test asserts the rendered XML
  ordering.

* **Big binary blobs in MPD `<Event>` body.** Some splice_info_
  sections approach the SCTE-35 max (4093 bytes per spec). Base64-
  encoded and embedded in the MPD inflates the manifest. Mitigation:
  no truncation (passthrough is the contract); document the upper
  bound and the expected per-event manifest delta.

* **Concurrent splices on a single broadcast.** Overlapping events
  (e.g., a long break with nested cue-out / cue-in pairs) are valid
  per SCTE 35; the ID-keyed pairing in HLS DATERANGE handles this.
  Mitigation: the playlist's DateRange Vec stores events
  independently; rendering is order-by-PTS with no merging.

* **Registry track-name collisions.** A misconfigured ingest could
  collide with the reserved `"scte35"` track. Mitigation: doc
  comment on `FragmentBroadcasterRegistry`; runtime collision drops
  the second producer with a counted warning (matches the captions
  posture).

## Ground truth (session 152 brief-write)

* **Head**: `073c311` on `main` (post-151). v0.4.1 unchanged.
  Workspace at **1111** tests / 0 / 0 (131 test binaries).
* **lvqr-fragment shape**: `FragmentBroadcasterRegistry` is
  `(String, String)`-keyed (broadcast_id, track_id); track names
  are arbitrary opaque strings.
* **lvqr-codec shape**: `hevc.rs`, `aac.rs`, `ts.rs`, `bit_reader.rs`,
  `error.rs`. ts.rs line 11 explicitly defers SCTE-35 to a future
  cut.
* **lvqr-ingest/rtmp.rs shape**: rml_rtmp `ServerSessionEvent` match
  with `_ => debug!("unhandled RTMP event")` fallthrough at line
  ~360. ScriptData (onMetaData / onCuePoint) currently dropped.
* **lvqr-srt shape**: TsDemuxer-driven dispatcher in
  `ingest.rs` line ~209; `StreamType::Unknown(_)` drops at line
  ~238.
* **lvqr-hls shape**: PlaylistBuilder in `manifest.rs` lines 344-572
  with sliding-window eviction; SubtitlesServer in `subtitles.rs`
  is the parallel-track captions reference. Master playlist
  rendered in `server.rs` lines 1170-1184.
* **lvqr-dash shape**: Single Period, no EventStream support;
  `mpd.rs` line ~277 has a deferred-EventStream comment.
* **lvqr-agent-whisper shape**: `agent.rs` lines 86-108 +
  `worker.rs` lines 305-317; emits Fragment with `track_id =
  "captions"`, `codec = "wvtt"`, `timescale = 1000`. The reference
  pattern session 152 mirrors.
* **Existing SCTE footprint**: zero. Two deferral comments only
  (ts.rs line 11, mpd.rs line 277).
* **CI**: 8 GitHub Actions workflows GREEN on session 151's
  substantive head (`073c311`).
* **Tests**: Net additions expected from this session: roughly +6
  lvqr-codec unit (Annex A vectors + CRC drop), +3 lvqr-fragment
  unit (registry track reservation, drain shape), +4 lvqr-hls unit
  (DateRange render, eviction, PDT mapping), +3 lvqr-dash unit
  (EventStream render, Period ordering, window prune), +1 lvqr-
  ingest integration (RTMP onCuePoint e2e), +1 lvqr-srt integration
  (SRT MPEG-TS scte35 PID e2e). Workspace target ~1129 / 0 / 0.
  pytest 38 (unchanged), Vitest 13 (unchanged). SDK packages
  unchanged at 0.3.2.

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_152_BRIEFING.md`. Read sections 1
through 7 in order; the actual implementation order is in section
"Execution order". The author of session 152 should re-read
`crates/lvqr-fragment/src/registry.rs` first (the parallel-track
surface session 152 reuses), then `crates/lvqr-agent-whisper/src/`
(the captions reference pattern), then `crates/lvqr-codec/src/ts.rs`
(the binary-parser conventions and the deferred-SCTE comment),
then `crates/lvqr-hls/src/manifest.rs` + `subtitles.rs` (the HLS
render + parallel-playlist model), then `crates/lvqr-dash/src/mpd.rs`
(the MPD render + the deferred-EventStream comment). Step 2 of the
execution order -- the rml_rtmp `ServerSessionEvent` surface check
-- is the gating decision that determines whether RTMP ships in v1
or staggers behind SRT.
