# Session 157 Briefing -- MoQ glass-to-glass SLO audit + scenario-(c) close-out

**Date kick-off**: 2026-04-27 (one day after session 156 + its four
2026-04-26 follow-ups landed). **Predecessor**: Session 156 follow-ups
(VideoToolbox CI lane + `POST /api/v1/slo/client-sample` route +
dual-auth + `@lvqr/dvr-player` SLO sampler). Origin/main head
`7964f11`. Workspace `0.4.1` unchanged. SDK packages
`@lvqr/core 0.3.2`, `@lvqr/player 0.3.2`, `@lvqr/dvr-player 0.3.3`.

The README "Next up" #5 (MoQ egress latency SLO, Phase A v1.1 #5)
remained the last open v1.1 checkbox after session 156 closed. The
session-156 follow-up shipped two of the three pieces the documented
close-out path called for: (1) the server-side
`POST /api/v1/slo/client-sample` route + `LatencyTracker` write +
dual-auth, and (2) the first real client (`@lvqr/dvr-player`'s opt-in
PDT-anchored sampler that pushes by default for HLS subscribers).
The third piece -- a Rust MoQ subscriber bin that pushes samples for
**pure-MoQ** subscribers -- is what session 157 was originally scoped
to ship.

Step 0 of the original session-157 brief was an **audit**: confirm
whether the MoQ wire actually carries the per-frame wall-clock that
the bin would need to compute glass-to-glass latency. The audit
fired first; the result locks the actual session-157 scope.

## Audit finding (2026-04-27)

**The MoQ wire does NOT carry `ingest_time_ms` or any per-frame
wall-clock anchor.** Cite-by-line:

* `Fragment::ingest_time_ms` exists as a `u64` on the in-memory
  pipeline struct (`crates/lvqr-fragment/src/fragment.rs:70`) and is
  set on the ingest path via `Fragment::with_ingest_time_ms`.
* `MoqTrackSink::push` is the only writer of the MoQ wire from the
  Fragment side. It writes `frag.payload.clone()` and nothing else
  (`crates/lvqr-fragment/src/moq_sink.rs:99-100`, `:104-105`). No
  serialization of `track_id`, `dts`, `pts`, `duration`, `priority`,
  `flags`, or `ingest_time_ms`.
* The inverse adapter explicitly documents the lossiness:
  "Timestamps (`dts`, `pts`, `duration`) and `priority` are **not**
  preserved across the MoQ projection because [`MoqTrackSink`] does
  not encode them onto the wire. The adapters emit zero for these
  fields." (`crates/lvqr-fragment/src/moq_stream.rs:35-41`).
  `MoqGroupStream::next_fragment` builds the receiving Fragment with
  hard-zero timestamps and never calls `with_ingest_time_ms`
  (`:142-152`).
* `Fragment` has no `Serialize` / `Deserialize` derives and no
  `to_bytes` / `from_bytes` constructors -- there is no "Fragment
  serialization" elsewhere either. It is purely an in-memory
  pipeline type.
* `lvqr-moq` is a thin re-export facade over `moq-lite 0.15`
  (`crates/lvqr-moq/src/lib.rs:40-43`). No LVQR-side metadata channel
  rides on top of frames.
* The fMP4 payload itself carries `tfdt` (decode-time relative to
  movie timeline), but no per-frame wall-clock box. The init segment
  `mvhd.creation_time` is per-stream, not per-frame, and would
  conflate stream-open clock drift with real network latency.

**Why the dvr-player works in spite of this**: HLS playlists carry
`#EXT-X-PROGRAM-DATE-TIME` text anchors per segment, and hls.js
surfaces them via the standard `HTMLMediaElement.getStartDate()`
extension. The dvr-player computes `latencyMs = Date.now() -
(startDate + currentTime * 1000)` per
`bindings/js/packages/dvr-player/src/slo-sampler.ts:67-78`. The HLS
manifest IS the wall-clock channel; MoQ has no manifest analog.

**Collateral defect**: the doc comment on
`ClientLatencySample::ingest_ts_ms`
(`crates/lvqr-admin/src/routes.rs:490-493`) currently reads:

> Wall-clock UNIX-ms timestamp the publisher stamped on the frame at
> ingest. The `lvqr_fragment::Fragment` already carries this via
> `Fragment::ingest_time_ms`; clients lift it from the frame's
> per-track metadata when they get one.

This is wrong. It implies a per-frame metadata channel exists on
some transports. None does today. The phrasing was apparently
written under the (incorrect) assumption that
`Fragment::ingest_time_ms` would later get serialized onto the wire;
the v1.1-B scoping rejection of that wire change leaves the
assumption hanging.

## Decision: Path Y -- document the gap, fix the comment, defer pure-MoQ to v1.2

Three paths were on the table. They are recapped here so a future
session can pick up the v1.2 close-out cleanly.

### Path Y (chosen): document the gap as v1.2; correct the misleading comment.

* Phase A v1.1 #5 stays unchecked. The README + Phase A v1.1 row
  reflect that the server-side endpoint + first HLS-side client
  shipped (session 156 follow-up), and that pure-MoQ subscribers
  remain open with a sketched v1.2 close-out path.
* Fix the `ClientLatencySample::ingest_ts_ms` doc comment in
  `crates/lvqr-admin/src/routes.rs` so the next reader does not
  repeat the audit. Replace the false "clients lift it from the
  frame's per-track metadata" claim with a transport-specific
  recovery table: HLS uses PDT via `getStartDate() + currentTime`;
  MoQ has no per-frame wall-clock channel, tracked as v1.2.
* Workspace stays at `0.4.1`. SDK packages stay at 0.3.2 / 0.3.2 /
  0.3.3. No relay-side wire change. No new crate. No new feature
  flag. Single commit, doc-only.

**Rationale**:

1. The original brief's scenario (c) explicitly framed the choice as
   "a strategy call, NOT engineering." The brief told this session:
   "If NO (scenarios (b) or (c)), report findings with a recommended
   path forward; don't ship the bin until the scoping is locked." The
   audit fired scenario (c); shipping anyway would violate the
   brief's own guard.

2. CLAUDE.md's "Don't add features, refactor, or introduce
   abstractions beyond what the task requires" + "Don't design for
   hypothetical future requirements" both cut toward Y. The Tier 5
   pure-MoQ client SDK that would consume a sidecar timing track does
   not exist yet. Building the producer side of an unconsumed
   channel is YAGNI.

3. The v1.1-B scoping decision is load-bearing. The doc-fix path
   respects it; reopening it would be a separate, larger session
   with its own design-decision read-back.

### Path X (deferred to v1.2): sibling `0.timing` MoQ track.

The right *eventual* close-out. Sketch:

* New track per broadcast: `<broadcast>/0.timing` published alongside
  `<broadcast>/0.mp4` (and `1.mp4` for audio). Foreign MoQ clients
  ignore unknown track names; LVQR-aware subscribers opt in by
  subscribing.
* Producer-side: tap the existing `Fragment::ingest_time_ms` stamp
  on the ingest path. On every keyframe (which already opens a new
  MoQ group on the video track) emit one timing object on the
  timing track with payload `(group_id_u64_le, ingest_time_ms_u64_le)`
  -- 16 bytes per anchor. Cadence ~1 / GoP (~2 s typical),
  configurable.
* New sink type in `lvqr-fragment`, e.g. `MoqTimingTrackSink`.
  Naming-wise the sibling might also live as a "catalog extension"
  per moq-lite conventions; the wire shape is the same.
* Subscriber-side: the future Rust MoQ sample-pusher bin subscribes
  to both `0.mp4` and `0.timing`. For each video frame received,
  look up the most-recent timing anchor whose `group_id` matches
  (or is the largest `group_id <= current`). Compute
  `latency_ms = now_unix_ms - timing.ingest_time_ms`. Push via the
  existing `POST /api/v1/slo/client-sample` route.
* Tests: producer-side stamping accuracy, subscriber-side track-join
  correctness, an integration test that drives the full RTMP ->
  relay -> sample-pusher -> SLO endpoint loop and asserts a
  non-empty entry under `transport="moq"`.
* Anti-scope (still): no per-frame wall-clock field on `0.mp4` (the
  v1.1-B rejection stays); no break to the existing 0.mp4 wire
  shape.

Estimated size: ~800-1200 LOC (sink + stream adapters + ingest
wiring + bin + integration test + unit coverage). A standalone
session, not a session-157 follow-on.

### Path Z (rejected outright): mvhd.creation_time anchor.

A pure-MoQ subscriber could parse the init segment's `mvhd.creation_time`
box (seconds since 1904-01-01 UTC) and combine it with `tfdt` to
recover a wall-clock-ish timestamp per frame. Rejected because:

* `mvhd.creation_time` is set once at moov emission (stream open),
  not per-frame. Encoder buffering + clock drift accumulate into the
  computed latency, conflating those with the metric we actually
  care about.
* The per-`transport` percentile bins on
  `lvqr_subscriber_glass_to_glass_ms` would not be apples-to-apples:
  HLS samples are PDT-anchored (publisher-side wall-clock); MoQ
  samples would be moov-creation-anchored (encoder-side, with drift).
  Mixing them in a single histogram would corrupt the SLO surface.
* It also depends on `lvqr-ingest::remux` actually setting
  `mvhd.creation_time` on emitted moovs, which is unaudited (and the
  default mp4 box value is "epoch", which would silently produce
  60-year-latency samples).

## What lands this session

* **`crates/lvqr-admin/src/routes.rs`** -- rewrite the
  `ClientLatencySample::ingest_ts_ms` doc comment to reflect the
  transport-specific reality: HLS lifts from PDT, MoQ has no
  per-frame wall-clock channel today. Adds an explicit forward-link
  to the v1.2 sidecar-track design.
* **`README.md`** -- update the "Next up" #5 row + the Phase A v1.1
  row to record that the server endpoint + first HLS client shipped
  (session 156 follow-up), and that pure-MoQ subscriber measurement
  is open / tracked as v1.2 with the sidecar-track sketch. New top
  bullet under "Recently shipped" for session 157 (the audit + doc
  fix).
* **`tracking/HANDOFF.md`** -- new `## Session 157 close (2026-04-27)`
  block above the existing session-156 close block. Lead paragraph
  + "Last Updated" line updated.
* **`tracking/SESSION_157_BRIEFING.md`** (this file) -- the audit
  finding + decision record + Path X v1.2 design sketch.

## What is NOT touched

* Any `crates/*` source code other than the
  `crates/lvqr-admin/src/routes.rs` doc comment.
* No new crate, no new bin, no new test, no new module.
* No MoQ wire-format change (the v1.1-B rejection stays).
* No new feature flag.
* No SDK package version bump.
* No npm publish, no cargo publish.
* Workspace `Cargo.toml` version (`0.4.1`).
* CI workflows.
* The `Fragment` struct, the `MoqTrackSink` / `MoqGroupStream`
  contracts, and the `lvqr-moq` facade all stay byte-identical.

## Verification

* `cargo check -p lvqr-admin` -- doc-comment-only edit, no
  compile-relevant change.
* `cargo test -p lvqr-admin --lib` -- unchanged baseline (54 / 0 / 0
  per session 156 follow-up; the doc-comment edit cannot move tests).
* `cargo fmt --all -- --check` -- clean.

## Pending follow-ups (NOT in this session)

* **Path X v1.2 close-out**: ship the sibling `0.timing` MoQ track +
  the Rust MoQ sample-pusher bin per the sketch above. This is the
  eventual close-out for Phase A v1.1 #5; until it lands, the
  checkbox stays unchecked.
* **NVENC / VAAPI / QSV transcode backends** (still v1.2; unchanged
  from session 156's pending list).
* **Per-rendition encoder selection** (still v1.2; unchanged).
