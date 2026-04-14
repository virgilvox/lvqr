# LVQR Handoff Document

## Project Status: v0.4-dev -- Tier 2.3 TrackCoalescer lands ffprobe-validated

**Last Updated**: 2026-04-13 (session 9)
**Tests**: 58 test binaries across the workspace, 236 individual
tests (plus ~6300 generated proptest cases per run across the
ingest, fragment, hevc, aac, cmaf-policy, and hls-manifest
harnesses), all green. cargo clippy --workspace --all-targets --
-D warnings is clean. cargo fmt --check is clean.

## Session 9 additions (2026-04-13): raw-sample TrackCoalescer

Session 8 closed every item from the session-8 work list; session 9
inherits the remaining Tier 2.3 items. This session lands the
largest of those three: the raw-sample coalescer scoped by the
session-7 design note in `lvqr-cmaf::segmenter`.

1. **`RawSample` type** (`crates/lvqr-cmaf/src/sample.rs`). Minimal
   producer-side value type carrying `track_id`, `dts`, `cts_offset`,
   `duration`, `payload`, and `keyframe`. The payload layout is
   codec-defined: AVCC length-prefixed for AVC/HEVC, raw AU for
   AAC. The producer is authoritative for every field; the
   coalescer never re-parses the payload to infer keyframe status
   or re-derives DTS from PTS. `RawSample::keyframe` and
   `RawSample::delta` constructors cover the common AVC Baseline
   and audio cases without a struct literal.

2. **`TrackCoalescer` state machine**
   (`crates/lvqr-cmaf/src/coalescer.rs`). Per-track pure state
   machine that accumulates `RawSample` values and flushes them on
   partial / segment boundaries as `CmafChunk` values. State
   transitions mirror the session-7 design note exactly: on push,
   if the pending batch exists and the new sample crosses a
   partial boundary OR a segment-window keyframe, flush the batch
   and return the chunk; otherwise append. The returned chunk
   carries the `pending_kind` that was fixed when the batch was
   opened, so later samples inside the same partial window cannot
   change the chunk's kind retroactively. A trailing `flush` at
   end-of-stream drains whatever is still pending.

3. **`build_moof_mdat` writer**. The coalescer's `flush_pending`
   builds a wire-ready `moof + mdat` pair via `mp4-atom`'s
   `Moof` / `Mfhd` / `Traf` / `Tfhd` / `Tfdt` / `Trun` types. The
   `trun.data_offset` field is computed via a two-pass encode: the
   first pass populates `data_offset = 0` to measure the moof
   size; the second pass re-encodes with `data_offset = moof_size
   + 8`. Every field in the moof is fixed-width so the total size
   is stable across the two encodes. The mdat header is written
   by hand (4 bytes size + 4 bytes `"mdat"`) rather than through
   `mp4-atom::Mdat` so the per-sample payload `Bytes` blobs are
   extended into the buffer without an intermediate `Vec<u8>`
   copy. Sample flags use the same `0x02000000` sync / `0x01010000`
   non-sync layout the hand-rolled writer at
   `lvqr-ingest::remux::fmp4::video_segment` ships today, so
   byte-level diffs against the hand-rolled path see identical
   sample-flag fields.

4. **ffprobe-validated round trip**
   (`crates/lvqr-cmaf/tests/conformance_coalescer.rs`). New
   integration test that builds a real AVC init segment via
   `write_avc_init_segment`, pushes 10 AVCC-wrapped synthetic
   samples (one IDR + nine P-slices) through a `TrackCoalescer`,
   concatenates the init segment with every chunk's payload, and
   runs the whole thing through ffprobe 8.1 via the soft-skip
   helper. **ffprobe accepts the output.** This is the first
   real-encoder-validated proof that the mp4-atom-backed
   coalescer produces sound CMAF output and that the two-pass
   `data_offset` patch lands at the right byte offset.

5. **Lib-level unit tests**. Five new tests in
   `coalescer.rs::tests` cover: first sample does not flush,
   partial boundary flushes pending, segment boundary fires on
   keyframe past window, `flush` drains pending at end-of-stream,
   and the moof structure round-trips through mp4-atom's own
   decoder (asserting sequence number, track id, tfdt DTS, trun
   entry count and sizes, and the data_offset placeholder
   position).

### What the coalescer is NOT yet wired into

* **`CmafSegmenter::from_sample_stream`** constructor. The
   design note scheduled it as part of this session's deliverable.
   Deferred because the existing `CmafSegmenter::new` consumes a
   `FragmentStream` (pre-muxed) and the `TrackCoalescer` operates
   at the `RawSample` level; unifying the two under one segmenter
   type requires a `SampleStream` trait and the producer side
   does not yet emit raw samples. Session 10 wires it when the
   first producer migrates.
* **The RTMP bridge**. `lvqr-ingest::remux::fmp4::video_segment`
   still ships the media segments for the `rtmp_ws_e2e` path.
   Retirement behind a feature flag requires flipping the
   `lvqr-ingest` -> `lvqr-cmaf` dep direction (currently
   `lvqr-cmaf` dev-deps `lvqr-ingest` for the parity test); the
   cleanest migration is to move the parity test out of
   `lvqr-cmaf` and into a top-level workspace test, or to accept
   the test-only dev-dep cycle. Deferred to session 10.
* **Audio coalescing**. The state machine is codec-agnostic but
   the ffprobe round-trip test only exercises video. Audio works
   by construction (every sample is a keyframe, so every chunk
   fires a partial boundary cleanly), but no test covers it yet.

### Contract slot status as of session 9

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC init + coalescer) |
| lvqr-hls | y | open (no parser surface) | y | via router oneshot | soft-skip (mediastreamvalidator) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-cmaf`'s conformance slot grew from "AVC + HEVC + AAC init"
to "AVC + HEVC + AAC init + coalescer". The coalescer test is the
first one in the crate that exercises both the init writer and
the media writer through a single ffprobe check, which is the
minimal shape of a real segmenter->consumer handshake.

## Session 8 additions (2026-04-13)

## Session 8 additions (2026-04-13)

Session 8 took the `lvqr-hls` crate from 2-of-5 to 4-of-5 contract
slots in a single run. Two commits landed on main:

1. **`lvqr-hls` axum router with LL-HLS blocking reload**
   (`crates/lvqr-hls/src/server.rs`). Adds `HlsServer` on top of the
   session-7 `PlaylistBuilder` so real HLS clients can GET a
   playlist, the init segment, and every part / segment URI the
   manifest references. Four routes: `GET /playlist.m3u8`,
   `GET /init.mp4`, `GET /{uri}` catch-all. The playlist handler
   honors `_HLS_msn=N` and `_HLS_msn=N&_HLS_part=M` query parameters
   via `tokio::sync::Notify::notify_waiters()` on every push, with
   a three-target-duration hold-back ceiling so a stalled producer
   cannot hang subscribers indefinitely. Producer API:
   `HlsServer::push_init(bytes)` (idempotent), `push_chunk_bytes`
   (pushes into `PlaylistBuilder` and caches the payload under the
   URI the builder generated), `close_pending_segment`
   (end-of-stream hook). `HlsServer` wraps an `Arc<HlsState>` and
   is cheap to clone; the same handle lives on both the producer
   side and the router side. Shared state uses
   `tokio::sync::RwLock + HashMap` rather than dashmap so no new
   transitive dep lands for a footprint with zero lock contention
   worth tuning.

   Integration coverage in `tests/integration_server.rs`: four test
   cases driving real HTTP requests through the router via
   `tower::ServiceExt::oneshot + http-body-util` so the whole
   handler surface is exercised end-to-end, just over the axum
   service trait instead of a loopback TCP socket. Cases cover
   playlist + init + segment round trip, `/init.mp4` 404 before
   push, unknown URI 404, and `_HLS_msn=1` blocking reload with a
   real parked future that only resolves after a second publish
   wakes the `Notify`.

2. **`mediastreamvalidator` soft-skip helper and lvqr-hls
   conformance slot**
   (`crates/lvqr-test-utils/src/lib.rs`,
   `crates/lvqr-hls/tests/conformance_manifest.rs`). Adds
   `lvqr_test_utils::mediastreamvalidator_playlist`, a soft-skip
   wrapper around Apple's `mediastreamvalidator` tool following the
   same pattern as the existing `ffprobe_bytes` helper. The wrapper
   writes a rendered playlist plus its `(uri, bytes)` segment map
   into a tempdir and invokes the validator against the playlist
   path. When the tool is not on PATH the helper returns `Skipped`;
   when it is installed locally the caller gets real validator
   output and `assert_accepted()` panics on a non-zero exit with
   the validator's stdout attached.

   Apple's `mediastreamvalidator` is part of a free Developer
   download that is not on Homebrew, so it is not installed in CI
   either. The soft-skip path is the common case today; the test
   upgrades to a real validator run automatically the moment the
   binary appears on PATH. The helper itself builds unconditionally.

   The new `conformance_manifest.rs` test builds a minimal
   two-segment manifest via `HlsServer`, harvests the rendered
   playlist through a `tower::oneshot` call against the router
   (so the bytes exactly match what a real HTTP client would see),
   and hands the playlist plus stub segment bodies to the new
   helper. Stub bodies are intentional: the `TrackCoalescer`
   design note in `lvqr-cmaf::segmenter` schedules real producer
   bytes for a later session, and the soft-skip path keeps the
   test green until then.

### Contract slot status as of session 8

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer now covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-hls | y | open (no parser surface) | y | via router oneshot | soft-skip (mediastreamvalidator) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-hls` is now 4-of-5. The fuzz slot stays intentionally open
because the crate has no parser attack surface (the router only
reads structured input produced by the `PlaylistBuilder`). The E2E
slot is filled by the `router oneshot` path rather than a loopback
TCP socket; a real TCP E2E lands when `lvqr-cli` composes HLS into
its serve path and `lvqr-test-utils::TestServer` grows an HLS
address.

### What session 8 did NOT land

* **`TrackCoalescer` implementation**. Still the largest deferred
  Tier 2.3 item. Design note lives in
  `crates/lvqr-cmaf/src/segmenter.rs`; session 9 implements the
  `RawSample` / `SampleStream` trait pair, the `TrackCoalescer`
  state machine, `CmafSegmenter::from_sample_stream`, and the
  round-trip test against `lvqr-ingest::remux::fmp4::video_segment`
  output.
* **`write_avc_init_segment` feature-flag migration in
  `rtmp_ws_e2e`**. Deferred because the cleanest migration requires
  `lvqr-ingest` to normal-dep `lvqr-cmaf` (not the reverse, which
  is the current direction via the session-7 parity test dev-dep).
  Session 9 resolves the dep direction and flips the feature flag
  through the CI matrix.
* **Real `TestServer` HLS address**. Blocked on `lvqr-cli` growing
  an HLS axum bind in the serve path. Session 9 or later.

## Session 7 additions (2026-04-13)

## Session 7 additions (2026-04-13)

Session 7 closed three of the four follow-up items from the session-6
HANDOFF work list in a single run. Only the `lvqr-hls` scaffold is
deferred; every other session-7 priority landed.

1. **AVC init parity gate** (`crates/lvqr-cmaf/tests/parity_avc_init.rs`).
   New test that runs both writers on the same SPS / PPS / dimensions
   triple and structurally compares the decoded Moov trees via
   `mp4_atom::Moov::decode`. The assertion set is the playback
   contract: ftyp brands, mvhd timescale + next_track_id, trak count
   and track_id, mdhd timescale, hdlr type, stsd codec kind, Avc1
   width / height / depth, avcC length_size, avcC SPS and PPS byte
   sequences, mvex.trex track_id + default_sample_description_index.
   Every one of those fields matches across writers. The total byte
   length differs (cmaf=698, ingest=662, delta=+36 bytes) because the
   two writers pick different defaults for fields that do not affect
   playback (creation timestamps, default volume, matrix values,
   stsz/stsc/stco table shapes, stts entry counts, hdlr name
   strings). A second test (`avc_init_parity_byte_equality_is_not_required`)
   pins the intentional-non-equality invariant so a future session
   cannot accidentally replace the structural-match test with a
   byte-equality assertion. Dev-dep cycle check: `lvqr-ingest` is now
   a dev-dep of `lvqr-cmaf` (test-only); no normal-dep cycle because
   `lvqr-ingest` does not depend on `lvqr-cmaf` in any direction.
   This is the first byte-level proof the mp4-atom writer is a
   drop-in replacement for the hand-rolled path. When the Tier 2.3
   migration retires the hand-rolled writer, the parity test becomes
   the migration gate.

2. **Real multi-sub-layer HEVC fixture via kvazaar**
   (`crates/lvqr-conformance/fixtures/codec/hevc-sps-kvazaar-main-320x240-gop8.{bin,toml}`).
   `brew install kvazaar` (kvazaar 2.3.2) plus a `ffmpeg 8.1 -f lavfi
   -i testsrc2=320x240:rate=30 -t 1 -f yuv4mpegpipe | kvazaar --input
   - --input-res 320x240 --input-fps 30 --gop 8` pipeline produces an
   HEVC Annex-B bytestream whose SPS has
   `sps_max_sub_layers_minus1 = 1`. x265 refused to emit this under
   every session-5 configuration tried; kvazaar's `--gop 8` flag
   flips it from the low-delay-P default into a real
   temporal-scalability GOP, which is what the multi-sub-layer SPS
   path is for.

   The SPS NAL payload (no 2-byte header) is pinned in the codec
   fixture corpus with a sidecar carrying every decoded field
   (profile_idc = 1 Main, level_idc = 186 i.e. HEVC level 6.2,
   compat flags 0x60000000, chroma 4:2:0, 320x240). The existing
   `lvqr-codec::tests::conformance_codec.rs` harness picked it up
   with zero code changes the first time it ran, validating the
   session-5 "drop a .bin + .toml in and coverage extends
   automatically" design choice. **The multi-sub-layer HEVC parser
   path now has real-encoder coverage** on top of the synthetic
   bit-writer fixtures that were the session-5 canonical truth. The
   session-5 HANDOFF note "If no maintained encoder on homebrew
   emits multi-sublayer SPSes, document that in HANDOFF and leave
   the synthetic coverage as the canonical truth" is now obsolete.

3. **CmafSegmenter raw-sample coalescer design note**
   (`crates/lvqr-cmaf/src/segmenter.rs` crate doc comment). Extended
   the existing segmenter-module doc with a nine-section design note
   covering: `RawSample` input shape, per-track state
   (`TrackCoalescer`), boundary decision flow (Append / FlushPartial
   / FlushSegment), `moof + mdat` construction via `mp4-atom`,
   init-segment lifecycle with a new `CmafSegmenter::init_segment`
   public method, interaction with the existing pass-through path
   during the transition, and a concrete session-7-or-7.5
   deliverable list. Treat the note as a living spec that the first
   implementation PR is allowed to rewrite. Pinning it in the
   segmenter source rather than a separate markdown file means the
   note travels with the code it describes and does not rot in
   isolation.

4. **`lvqr-hls` crate scaffold**. First egress protocol to land on
   top of `lvqr-cmaf::CmafChunk`. Pure-library day-one scope:
   * `Manifest` + `Segment` + `Part` + `ServerControl` types
     modelling an RFC 8216 media playlist plus the LL-HLS draft
     extensions.
   * `PlaylistBuilder` pure state machine that consumes `CmafChunk`
     values, enforces strict DTS monotonicity and non-zero
     duration, and produces an updated `Manifest` on every push.
   * `Manifest::render` text renderer emitting `#EXTM3U`,
     `#EXT-X-VERSION:9`, `#EXT-X-TARGETDURATION`,
     `#EXT-X-SERVER-CONTROL` (with `CAN-BLOCK-RELOAD=YES`,
     `PART-HOLD-BACK`, `HOLD-BACK`), `#EXT-X-PART-INF`,
     `#EXT-X-MAP`, `#EXT-X-MEDIA-SEQUENCE`, per-segment `#EXTINF`,
     and per-part `#EXT-X-PART` with `INDEPENDENT=YES` on
     keyframes.
   * Day-one 2-of-5 contract slots: 5 unit tests, 2 integration
     tests (`tests/integration_builder.rs`), 3 proptest properties
     (`tests/proptest_manifest.rs` -- never-panic, well-formed
     output, strictly monotonic media sequences). Fuzz, E2E, and
     conformance slots are open and land with the axum router +
     Apple `mediastreamvalidator` wrapper in a later session.
   * No axum router yet. The router lands when a real HTTP
     consumer (browser, `hls.js`, `mediastreamvalidator`) arrives.
     Day-one scope is the manifest library only.
   * Explicitly out of scope for now: multivariant master
     playlists, byte-range delivery, encryption, discontinuity
     handling, rendition groups, byte-level `mediastreamvalidator`
     conformance.

### What session 7 did NOT land

Nothing from the session 7 work list carried over. Session 8 picks
up the remaining Tier 2.3 items (raw-sample coalescer implementation,
lvqr-hls axum router, retiring the hand-rolled fmp4 writer behind a
feature flag).

### Contract slot status as of session 7

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer now covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-hls | y | open (no parser surface) | y | open (axum router pending) | open (mediastreamvalidator pending) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-hls` joins the table at 2-of-5 on day one. The multi-sub-layer
fixture strengthens the existing `lvqr-codec` conformance slot
without adding a new slot.

## Session 6 additions (2026-04-13): HEVC + AAC init segment writers

Session 6 tackled priority item 1 from the "Recommended Tier 2.3 entry
point" work list: grow `lvqr-cmaf` beyond AVC. The AVC-only
`write_avc_init_segment` from session 5 is now joined by HEVC and AAC
siblings, both built on `mp4-atom` and both covered by the same
ffprobe conformance harness.

1. **`write_hevc_init_segment` + `HevcInitParams`**. New public API in
   `crates/lvqr-cmaf/src/init.rs`. Takes VPS / SPS / PPS NAL unit byte
   blobs (each including the 2-byte HEVC NAL header so they can be
   written verbatim into the `hvcC` arrays) plus a decoded
   `lvqr_codec::hevc::HevcSps` view used to populate the `hvcC`
   header (profile, tier, level, chroma format) and the `tkhd` /
   `visual` dimensions. `general_constraint_indicator_flags` ships
   zeroed because the SPS parser does not surface them yet; that is
   fine for the 8-bit Main profile streams LVQR supports today but
   becomes a real gap the moment a Main10 or an HDR stream enters the
   picture. Comment at the call site flags the limitation.

2. **`write_aac_init_segment` + `AudioInitParams`**. Feeds raw
   `AudioSpecificConfig` bytes through `lvqr_codec::aac::parse_asc`
   and builds an `mp4a` sample entry plus an `esds` box using
   `mp4-atom`'s descriptor writer. mp4-atom's `DecoderSpecific` only
   supports the 4-bit `sampling_frequency_index` encoding and the
   compact (<32) AOT form, so the writer refuses:
   * sample rates that do not map to one of the 13 indexable
     frequencies in ISO/IEC 14496-3 Table 1.16
     (`InitSegmentError::UnsupportedAacSampleRate`),
   * AOT >= 32 / escape-encoded object types
     (`InitSegmentError::InvalidAsc` wrapping a `CodecError::MalformedAsc`
     with a descriptive message).

   Both errors are proptest-friendly and exercised by a new unit test
   using a hand-built explicit-frequency ASC. This is tighter than
   the existing hand-rolled `lvqr-ingest::remux::fmp4::esds` path,
   which silently produced malformed descriptors for ASCs longer than
   127 bytes pre-session-5.

3. **Real x265 HEVC NAL units captured**. Session 5 bootstrapped the
   fixture corpus with a real x265 SPS (post-NAL-header payload) but
   not VPS / PPS. Session 6 captured a full VPS + SPS + PPS triple
   from a `ffmpeg 8.1 -c:v libx265 -preset ultrafast` encode of a 1 s
   320x240 testsrc2 clip and pinned the bytes inline in the new
   lvqr-cmaf unit and conformance tests. The capture was a one-shot
   Python walker over the hvcC box; adding it to the corpus proper as
   a named fixture is deferred until the fixture loader grows a
   multi-NAL-per-fixture variant.

4. **ffprobe conformance expanded to HEVC and AAC**. New tests in
   `crates/lvqr-cmaf/tests/conformance_init.rs`:
   * `ffprobe_accepts_hevc_init_segment`: loads the
     `hevc-sps-x265-main-320x240` conformance-corpus fixture,
     constructs an `HevcSps` view from the sidecar metadata (so a
     drift between `parse_sps` output and the sidecar would fail
     `lvqr-codec`'s `conformance_codec.rs` harness first), builds an
     HEVC init segment using the captured x265 VPS / SPS NAL / PPS
     blobs, and feeds the result to ffprobe 8.1. **ffprobe accepts
     the output.** This is the first proof in the repo that the
     `mp4-atom`-backed HEVC writer produces bytes a real validator
     will take.
   * `ffprobe_accepts_aac_init_segment`: loads the
     `aac-asc-aaclc-44100hz-stereo` fixture, feeds the raw ASC
     through the new `write_aac_init_segment`, and asserts ffprobe
     accepts the resulting init segment. Same story for AAC.

5. **Unit-level round trips**. Three new lib-level tests cover the
   new writers without the conformance corpus dev-dep:
   * `hevc_init_segment_starts_with_ftyp_and_contains_moov`
   * `hevc_init_segment_round_trips_through_mp4_atom` (asserts the
     three `HvcCArray` entries match the input VPS / SPS / PPS
     byte-for-byte after mp4-atom decode)
   * `aac_init_segment_round_trips_through_mp4_atom` (asserts
     channel_count, sample_size, AOT, freq_index, chan_conf)
   * `aac_init_rejects_non_indexable_sample_rate` (explicit-frequency
     11468 Hz ASC must be refused with the typed error variant)

6. **Public API surface grew**. `lvqr_cmaf::{HevcInitParams,
   AudioInitParams, write_hevc_init_segment, write_aac_init_segment}`
   are now re-exported from the crate root. `InitSegmentError` gained
   two variants (`InvalidAsc`, `UnsupportedAacSampleRate`) so callers
   can distinguish a parse failure from a sample-rate-out-of-table
   rejection. Existing `VideoInitParams` / `write_avc_init_segment`
   signatures are unchanged; this is purely additive.

### What session 6 did NOT land (deferred to session 7)

The "Recommended Tier 2.3 entry point" work list named four follow-ups
after the HEVC / AAC writers. Only item 1 closed this session. The
remaining three are still open:

1. **`rtmp_ws_e2e` migration and AVC byte-diff** (priority 2 in the
   prior handoff). Wiring `lvqr-cmaf::write_avc_init_segment` into
   `rtmp_ws_e2e` alongside the hand-rolled
   `lvqr-ingest::remux::fmp4::video_init_segment` and diffing the
   byte outputs is the first real drop-in-replacement proof. Not
   landed this session; the HEVC / AAC writers were the higher
   leverage item because they unblock every future egress crate that
   needs non-AVC codec support.
2. **Multi-sub-layer HEVC fixture capture** (priority 3). Still not
   attempted; x265 will not produce `max_sub_layers_minus1 > 0` in
   any configuration tried so far, and kvazaar has not been
   installed. Synthetic-only coverage remains the canonical truth.
3. **CmafSegmenter raw-sample coalescer** (priority 4). Not started.
   The segmenter remains a pass-through that annotates pre-muxed
   fragments; the load-bearing raw-sample coalescer is a design-note
   item for the next session, not an implementation item.

### Contract slot status as of session 6

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-cmaf`'s conformance coverage is now AVC + HEVC + AAC against
ffprobe 8.1. Fuzz remains intentionally open for the same reason as
the prior sessions: the crate consumes `Bytes` from trusted producers
and writes mp4-atom structures, so there is no parser attack surface.

## Session 5 part 2 additions (2026-04-13)

Directly after the part-1 commit landed and pushed, two follow-ups
from the "Recommended Tier 2.3 entry point" work list in this file
closed in the same session:

1. **`lvqr-conformance` fixture corpus bootstrapped**. The session-3
   Tier 1 item that had been BLOCKED since session 3 on "no ffmpeg
   in the dev env" unblocked as soon as ffmpeg 8.1 was installed.
   Captured:
   - `fixtures/codec/hevc-sps-x265-main-320x240.{bin,toml}` -- the
     real x265 SPS already pinned in the parser's unit test, now
     sitting in the corpus with a sidecar naming every expected
     decoded field including `general_level_idc = 60` (HEVC level
     2.0, which x265 picks for 320x240 at 30 fps).
   - `fixtures/codec/aac-asc-aaclc-{44100,48}khz-stereo.{bin,toml}`
     -- the two canonical AAC-LC ASC byte blobs LVQR already relies
     on elsewhere, pinned with their decoded values.
   - `fixtures/fmp4/cmaf-h264-baseline-360p-1s.{mp4,toml}` -- a 1 s
     fragmented CMAF H.264 Baseline 3.1 capture from ffmpeg, seed
     for future lvqr-ingest and lvqr-cmaf consumer tests.
   - `fixtures/rtmp/h264_aac_1s.{flv,toml}` -- a 1 s H.264 + AAC-LC
     FLV, first real RTMP test vector in the repo.

   `lvqr-conformance::codec::{list, load, CodecFixture,
   CodecFixtureMeta, HevcSpsExpected, AacAscExpected}` exposes a
   typed loader: consumers call `list()` to iterate every fixture
   on disk, and each fixture comes with parsed sidecar metadata so
   adding a new byte blob + TOML pair automatically extends
   coverage without touching test code. Sidecar parsing runs
   through `toml` + `serde`, already in the workspace dep set.

2. **Conformance slot closed for `lvqr-codec`**. New
   `crates/lvqr-codec/tests/conformance_codec.rs` iterates the
   codec corpus via `lvqr_conformance::codec::list()` and asserts
   `parse_sps` / `parse_asc` decode every blob to the expected
   values from the sidecar. **The contract mechanism paid for
   itself on its first run**: my initial hand-computed sidecar
   guessed `general_level_idc = 93` for the 320x240 x265 SPS
   (copying the value from the synthetic `codec_string_format` unit
   test), and the conformance test failed loudly on the first run
   because the real encoder output is level 60. The fixture sidecar
   and the hand-rolled x265 unit test are now both pinned to the
   real value. This is exactly the "catches silent drift between
   hand-written synthetic tests and real encoder output" story the
   5-artifact contract exists for.

   `lvqr-codec` is now the second crate (after `lvqr-ingest`) to
   hit **5/5 contract slots**. The only remaining open slots
   workspace-wide are the fuzz slots on `lvqr-record`, `lvqr-moq`,
   `lvqr-fragment`, `lvqr-cmaf` (all low-marginal-value per prior
   session decisions) and the conformance slots on `lvqr-moq` and
   `lvqr-fragment` (pure value types with no external validator
   target).

## Session 5 additions (2026-04-13): Tier 2.2 closure + Tier 2.3 scaffold

Five work items landed in a single session, closing Tier 2.2 and
opening Tier 2.3 on top of the `mp4-atom` box writer.

1. **HEVC SPS parser now handles multi-sub-layer streams**. Replaced
   the session-4 `Unsupported` bail at `sps_max_sub_layers_minus1 > 0`
   with a real `parse_ptl_sublayers` helper that walks the sub-layer
   profile/level present flag loop (2 bits per sub-layer), the
   reserved-zero-2-bits padding for layers in `max_sub_layers_minus1..8`,
   and the per-sub-layer 88-bit PTL body plus optional 8-bit level_idc.
   LVQR does not surface per-sub-layer data; the bits are consumed so
   the reader ends up at the right position for the SPS fields that
   follow. Three positive decode tests land alongside: synthetic
   single-sub-layer, synthetic two-sub-layer, and synthetic
   max-sub-layer (`max_sub_layers_minus1 = 6`), all built via a tiny
   test-only bit writer.

   Plus a **real encoder fixture**: an SPS captured from
   `ffmpeg -c:v libx265` encoding a 320x240 testsrc2 clip, pinned in
   `parse_sps_decodes_real_x265_single_sublayer`. This is the first
   time the parser is pinned against an independent encoder's bit
   layout rather than the LVQR test writer. Multi-sub-layer *real*
   fixtures are deferred: neither x265's `--temporal-layers` nor
   b-pyramid modes produced a `max_sub_layers_minus1 > 0` SPS in any
   configuration tried, so the multi-sub-layer path is currently
   synthetic-only. Not ideal; honest.

2. **`lvqr-ingest::remux::fmp4::esds` migrated to
   `lvqr_codec::aac::parse_asc`**. Closes the internal audit finding
   "fMP4 esds descriptor uses single-byte length encoding". The
   hand-rolled `parse_audio_specific_config` in `flv.rs` now
   delegates to the hardened parser, so every FLV AAC sequence
   header benefits from the 5-bit + 6-bit object-type escape, the
   15-index explicit-frequency escape, and HE-AAC SBR/PS signalling
   that the v0.3 writer silently truncated. The descriptor length
   encoding in the `esds` box is now a new `write_mpeg4_descriptor`
   helper that always emits the 4-byte MPEG-4 variable-length form
   (tag byte + 4 length bytes, MSB continuation), replacing the
   previous single-byte prefix that would malform on any
   DecoderSpecificInfo larger than 127 bytes. The hardened path is
   exercised by a new conformance test
   `ffprobe_accepts_audio_init_and_frame` in `golden_fmp4.rs` which
   feeds the AAC init segment plus a one-frame media segment to
   ffprobe 8.1, and by a new unit test
   `mpeg4_descriptor_length_encoding_round_trips_large_payloads` that
   writes a 200-byte payload through `write_mpeg4_descriptor` and
   asserts every byte of the emitted length field.

3. **`lvqr-cmaf` crate scaffolded, built on `mp4-atom` 0.10.1**. New
   workspace member opening Tier 2.3. Four modules:

   * `chunk.rs`: `CmafChunk` (wire-ready `moof+mdat` bytes, DTS,
     duration, track id) plus `CmafChunkKind`
     (`Partial` / `PartialIndependent` / `Segment`) so egress crates
     get HLS/DASH/MoQ boundary classification in one enum.
   * `policy.rs`: `CmafPolicy` tuning (partial + segment durations)
     and `CmafPolicyState`, a pure state machine that classifies
     each fragment by keyframe flag + DTS. Defaults land for 90-kHz
     video (200 ms partial, 2 s segment) and 48-kHz audio. Pure, no
     I/O, no async, trivially proptest-able.
   * `init.rs`: working `write_avc_init_segment` using `mp4-atom`'s
     `Ftyp`, `Moov`, `Mvhd`, `Trak`, `Tkhd`, `Mdia`, `Mdhd`, `Hdlr`,
     `Minf`, `Vmhd`, `Dinf`, `Dref`, `Stbl`, `Stsd`, `Codec::Avc1`,
     `Avcc`, `Visual`, `Mvex`, `Trex`. Encodes directly into a
     `BytesMut` via the crate's `bytes` feature. Round-trips through
     `mp4-atom` decode and is accepted by ffprobe 8.1.
   * `segmenter.rs`: `CmafSegmenter<S: FragmentStream>` with pull-
     based `next_chunk()`. Thin today because every `Fragment` from
     the RTMP bridge is already a pre-muxed `moof+mdat`; the
     segmenter annotates with boundary info and passes through. The
     real sample-coalescer grows additively when ingest begins
     emitting raw samples instead of pre-muxed fragments.

   4-of-5 contract slots on day one: proptest (`tests/proptest_policy.rs`,
   4 properties x 200 cases), integration (`tests/integration_segmenter.rs`,
   3 scenarios driving a scripted `FragmentStream`), conformance
   (`tests/conformance_init.rs`, ffprobe accepting the mp4-atom init
   segment), e2e via the workspace `rtmp_ws_e2e` path. Fuzz slot
   intentionally open: the segmenter has no parser attack surface.

4. **cargo-fuzz targets for `lvqr-codec`**. New `crates/lvqr-codec/fuzz/`
   with three targets: `parse_hevc_sps`, `parse_aac_asc`, and
   `read_ue_v` (which uses the input's first byte as a bit offset so
   the exp-Golomb decoder is fuzzed across every starting alignment,
   bounded to 64 iterations per input so libfuzzer terminates).
   Excluded from the workspace members list because `libfuzzer-sys`
   needs nightly. `.github/workflows/fuzz.yml` migrated from a single
   `target` matrix axis to an `include`-style matrix carrying
   `(target, fuzz_dir)` pairs so the ingest and codec fuzz crates
   share one job definition. Closes the fuzz slot for `lvqr-codec`.

5. **Conformance slot for `lvqr-record`**. New
   `tests/record_conformance.rs` builds a real AVC init segment via
   `lvqr_cmaf::write_avc_init_segment`, drives it through a MoQ
   origin + broadcast + track + group publisher, records it with
   `BroadcastRecorder::record_broadcast`, reads the init file back
   from disk, runs it through `ffprobe_bytes`, and asserts
   byte-for-byte equality with the bytes fed to the publisher. This
   is the first test in the repo that exercises `lvqr-cmaf` from a
   different crate, and the first that chains mp4-atom -> MoQ ->
   recorder -> disk -> ffprobe end-to-end. Closes the last open
   contract slot on `lvqr-record` (fuzz stays open per the session-3
   decision that pure helpers are already proptest-covered and fuzz
   is low-marginal-value).

### Library research decision (session 5)

Before writing any new codec parser code this session, verified that
the Rust ecosystem still has no maintained, pure-Rust, MIT/Apache
alternative for the narrow "codec string + sample-entry fields"
niche that `lvqr-codec` owns:

* No `h265-reader` / `h26x-reader` crates exist.
* `hevc-parser` (quietvoid) is a Dolby-Vision-focused tool,
  self-described "incomplete", pulls `nom 8` + `bitvec_helpers` +
  `matroska-demuxer` + `regex-lite`. Not a drop-in.
* Mozilla `mp4parse` is MPL-2.0, read-only, last release May 2023.
* `symphonia`'s AAC ASC parser is private behind MPL-2.0 and not
  exposed as a standalone API.
* `bitstream-io` (Matt Brubeck) is actively maintained but does not
  ship exp-Golomb, so replacing LVQR's ~250-line BitReader would
  save <200 lines and still require Golomb on top.
* `mp4-atom` 0.10.1 (kixelated, MIT/Apache, pure Rust, actively
  maintained) is the right call for `lvqr-cmaf` and already wired
  in.

Decision: keep `lvqr-codec` hand-rolled, build `lvqr-cmaf` on
`mp4-atom`. Revisit when a maintained pure-Rust HEVC/ASC parser
appears or symphonia factors its ASC code out.

## Session 4 part 2 additions (2026-04-13): Tier 2.2 `lvqr-codec` scaffold

The first Tier 2.2 deliverable landed directly after Tier 2.1 was
committed and pushed: a `lvqr-codec` crate with a shared MSB-first
forward bit reader (including H.26x exp-Golomb decoders and
EBSP->RBSP emulation-prevention byte stripping), an HEVC NAL unit
type classifier + minimal SPS parser (profile / tier / level /
chroma-format / resolution, enough to build an `hev1` sample entry
and emit a codec string), and a hardened AAC `AudioSpecificConfig`
parser that correctly handles the 5-bit + 6-bit escape encoding for
object types in the 32..=63 range, the 15-index explicit-frequency
escape, and HE-AAC (SBR) / HE-AAC v2 (PS) signalling.

4-of-5 artifact coverage on day one: proptest never-panic harnesses
for HEVC and AAC, an integration test that wires the parsers to
expected codec-string outputs, 19 unit tests covering the bit
reader + both codec modules. Fuzz is deferred because cargo-fuzz
harnesses want their own nightly-only crate, and conformance is
deferred until real encoder fixtures are captured and checked in.

The HEVC SPS parser intentionally only supports
`sps_max_sub_layers_minus1 == 0` (every consumer HEVC stream LVQR
has encountered in practice). Multi-sublayer streams return
`CodecError::Unsupported` so callers know to plug in a more complete
parser. Full scaling-list / VUI / HRD parsing is explicitly out of
scope: LVQR does not decode HEVC, it only needs enough metadata to
build an fMP4 init segment.

The AAC parser is ready to replace the 2-byte ASC assumption baked
into `lvqr-ingest::remux::fmp4::esds`. That migration will land
alongside the HEVC RTMP support in a follow-up commit.

## What a new session must read first

1. `CLAUDE.md` (project rules, hard hard rules)
2. `tracking/ROADMAP.md` (authoritative 18-24 month plan, 10 load-bearing decisions)
3. `tracking/AUDIT-2026-04-13.md` (competitive audit, 5 strategic bets, what NOT to ship)
4. `tracking/AUDIT-INTERNAL-2026-04-13.md` (dead-code, bug, hardening inventory + Fix Plan)
5. `tracking/AUDIT-READINESS-2026-04-13.md` (CI + supply chain + doc drift + Tier 1 progress)
6. `tracking/HANDOFF.md` (this file)
7. `tests/CONTRACT.md` (5-artifact test contract)

The single most important architectural decision in the entire roadmap
is the Unified Fragment Model (`lvqr-fragment`) plus the `lvqr-moq`
facade crate, Tier 2.1. As of session 4 both have landed, the RTMP
bridge has migrated to produce Fragments through `MoqTrackSink`, and
the dead code in `lvqr-core` (Registry, RingBuffer, GopCache, Gop)
has been deleted in the same commit. Tier 2.2 (lvqr-codec, HEVC
scaffold) is the next target.

## Session 4 (2026-04-13) additions -- Tier 2.1 landing

Seven bullets. All of Tier 2.1 as scoped in the roadmap plus one
follow-up fix for a Tier 1 latent issue that surfaced under ffprobe
8.1.

1. **`crates/lvqr-moq/` facade crate**. Re-exports the moq-lite types
   every LVQR crate uses (`Track`, `Origin`, `OriginProducer`,
   `BroadcastProducer`, `BroadcastConsumer`, `TrackProducer`,
   `TrackConsumer`, `GroupProducer`, `GroupConsumer`) under one module
   so upstream churn has a single point of impact. `MOQ_LITE_VERSION`
   const pins the version the facade was built against. The lib.rs
   doc is explicit that this is a re-export layer today and that
   newtypes will be introduced at the facade when downstream crates
   need behavioral hooks -- honest scoping instead of 500 lines of
   mechanical wrappers with no current value.

2. **`crates/lvqr-fragment/` Unified Fragment Model**. Core types
   (`Fragment { track_id, group_id, object_id, priority, dts, pts,
   duration, flags: FragmentFlags, payload: Bytes }`, `FragmentFlags`
   with `KEYFRAME` / `AUDIO` / `DELTA` / `DELTA_DISCARDABLE` presets,
   `FragmentMeta` with lazy `set_init_segment` for the late-binding
   RTMP sequence-header case) plus the `FragmentStream` trait (an
   async `next_fragment() -> Option<Fragment>` + a `meta()` accessor,
   intentionally without `async_trait` since the future is always
   borrowed from `self`).

3. **`MoqTrackSink` adapter** inside `lvqr-fragment`. The first
   concrete projection from Fragment into a wire format: holds a
   `TrackProducer` plus an optional current `GroupProducer`, opens a
   new MoQ group on every keyframe push (closing the prior group
   first), prepends `FragmentMeta::init_segment` as frame 0 of every
   new group so late-joining subscribers can always decode, writes
   delta fragments into the current group, and silently drops deltas
   that arrive before any keyframe. `Drop` finishes the current
   group. This is the load-bearing shape change: every future ingest
   crate produces Fragments, calls `sink.push(..)`, and never touches
   MoQ directly.

4. **Facade migration across every downstream crate**. `lvqr-relay`,
   `lvqr-ingest`, `lvqr-record`, `lvqr-cli`, plus their tests, now
   import MoQ types from `lvqr_moq::` rather than `moq_lite::`.
   `lvqr-record` dropped its direct `moq-lite` dep entirely. `lvqr-relay`
   and `lvqr-cli` kept their direct `moq-lite` deps because they still
   interoperate with `moq-native` at the transport layer, but every
   *type reference* in those crates now goes through the facade.

5. **`RtmpMoqBridge` migrated to produce Fragments**. The video and
   audio RTMP callbacks no longer manipulate MoQ `GroupProducer`s
   directly. Instead each stream holds a `MoqTrackSink` for video and
   another for audio; the callbacks build a `Fragment` (with the
   appropriate `FragmentFlags::KEYFRAME` or `FragmentFlags::DELTA`)
   and call `sink.push(&frag)`. FLV sequence headers call
   `sink.set_init_segment(init)`. The audio path finishes its group
   after every frame so every AAC frame is its own independently-
   decodable MoQ group (the existing behavior, preserved). Every
   existing `rtmp_bridge_integration` and `rtmp_ws_e2e` test passes
   unchanged, which is the real proof the migration is behavior-
   preserving.

6. **Dead code deletion in `lvqr-core`**. Per the internal audit
   recommendation at `tracking/AUDIT-INTERNAL-2026-04-13.md`, deleted
   `Registry`, `RingBuffer`, `GopCache`, and the `Gop` struct in the
   same commit that lands their replacement. Removed both benches
   (`fanout.rs` and `ringbuffer.rs`), their `criterion` dev-dep, and
   the `TestPublisher` + `synthetic_gop` helpers in `lvqr-test-utils`
   that only existed to exercise `Registry`. `Frame`, `TrackName`,
   `StreamId`, `SubscriberId`, `RelayStats`, `EventBus`, and
   `RelayEvent` survive as shared value types. `lvqr-core` is now
   roughly 40% smaller and every remaining type has at least one
   production consumer.

7. **5-artifact contract closed for the new crates (4 of 5 slots)**.
   `lvqr-moq` and `lvqr-fragment` both ship proptest, integration,
   and e2e coverage on day one; conformance and fuzz slots are still
   open by design (both require additional infrastructure and belong
   to their own follow-up work). `scripts/check_test_contract.sh`
   was updated to include the two new crates in its in-scope list,
   the contract runs green in educational mode, and the only
   remaining warnings are the four still-open fuzz/conformance slots
   across `lvqr-record`, `lvqr-moq`, and `lvqr-fragment`.

### Bonus fix: ffprobe 8.1 false negative in the golden fMP4 conformance slot

`ffprobe_bytes` in `lvqr-test-utils` treated any non-empty stderr on
an exit-zero ffprobe run as a failure. ffprobe 8.1 (the current
Homebrew version) emits decoder-level warnings
(`deblocking_filter_idc 32 out of range`, `no frame!`) on the
synthetic H.264 NAL payloads the golden tests feed it, even though
the container parses cleanly. Under older ffprobe builds those
warnings were silent and the test passed; under 8.1 they broke CI
the moment ffmpeg got installed locally. Fix: trust the exit code
as the authoritative verdict (non-zero = rejected, zero = accepted)
and surface stderr on exit-zero runs via `eprintln!` as diagnostics
rather than failing on them. This closes the last pre-existing test
failure that was latent before session 4 and unrelated to Tier 2.1.

## Session 3 (2026-04-13) additions

Seven Tier 1 items landed, one bonus security fix caught by a new
proptest, one bonus integration harness closing an audit gap. The
single Tier 1 item still blocked is the conformance fixture corpus
bootstrap, which requires `ffmpeg` in the dev environment.

1. **`lvqr_cli::start` library target** (`crates/lvqr-cli/src/lib.rs`).
   Extracted the full server wiring from `main.rs` into a public lib:
   `ServeConfig`, `ServerHandle`, `async fn start(config) -> Result<ServerHandle>`.
   All listeners bind before `start` returns so callers that pass
   `port: 0` get real addresses back off the handle. `main.rs` shrinks
   to ~150 lines (parse args, build auth, call `start`, wait on
   ctrl-c, `handle.shutdown().await`). `RtmpServer::run_with_listener`
   added in `lvqr-ingest` so the pre-bind pattern works without a
   find-available-port race.

2. **`lvqr_test_utils::TestServer`** (`crates/lvqr-test-utils/src/test_server.rs`).
   Thin wrapper over `lvqr_cli::start` that binds on `127.0.0.1:0`,
   disables Prometheus (process-wide, panics on second install), and
   returns a handle with `rtmp_url()`, `ws_url()`, `ws_ingest_url()`,
   `http_base()`, `relay_addr()`, etc. Config builder supports
   `with_mesh(max_peers)`, `with_auth(SharedAuth)`, `with_record_dir`.
   Dev-dep cycle `lvqr-cli -> lvqr-ingest -> [dev] lvqr-test-utils -> lvqr-cli`
   is allowed by cargo and works correctly.
   Smoke tests at `crates/lvqr-test-utils/tests/test_server_smoke.rs`
   prove every listener binds and every URL helper formats against the
   bound address.

3. **`lvqr-signal` input validation** (`crates/lvqr-signal/src/signaling.rs`).
   Closes the internal-audit finding. New `is_valid_peer_id` (enforces
   `[A-Za-z0-9_-]{1,64}`) and `is_valid_track` (wider alphabet plus
   explicit rejection of `..`, `//`, leading/trailing slash,
   backslashes). New `SignalMessage::Error { code, reason }` variant.
   `wait_for_register` sends a structured error frame on every reject
   path (`invalid_json`, `invalid_peer_id`, `invalid_track`,
   `expected_register`) and closes the session. The main loop rejects
   a second Register on an already-registered connection with
   `duplicate_register`, enforcing the audit's "cap registrations per
   connection at 1" explicitly. Peer-id log fields on reject paths
   record only `len`, never the attacker-controlled bytes.
   Integration tests at `crates/lvqr-signal/tests/signal_integration.rs`
   drive the validators through the real `/signal` endpoint on a
   `TestServer::with_mesh(3)` instance using `tokio-tungstenite`.
   Five tests: malformed peer_id, traversal track, non-Register first
   message, duplicate Register, happy path (receives AssignParent).

4. **Proptest extensions for `lvqr-ingest`**
   (`crates/lvqr-ingest/tests/proptest_parsers.rs`). Four new
   properties, roughly 4100 generated cases per run (up from 2560):
   `extract_resolution_never_panics`,
   `extract_resolution_never_panics_on_sps_prefix`,
   `generate_catalog_always_parses_as_json` (parses output with
   `serde_json::from_str`, asserts track count and required fields),
   `generate_catalog_places_video_before_audio` (ordering invariant
   the browser MSE player depends on). Added `serde_json` as dev-dep.

5. **Proptest for `lvqr-record` pure helpers**
   (`crates/lvqr-record/tests/proptest_recorder.rs`). Five
   properties targeting the internal helpers exposed via a new
   `#[doc(hidden)] pub mod internals` re-export. **Proptest caught a
   real path-traversal bypass in `sanitize_name`**: input `".\0."`
   sanitized to `".."` because the old ordering stripped control
   chars *after* the `..` replacement pass, so deleting `\0`
   regenerated a traversal sequence. Fixed by stripping controls
   first, then replacing `/`, `\`, and `..`. Regression seed pinned
   in `tests/proptest_recorder.proptest-regressions`.

6. **Nightly cargo-fuzz CI** (`.github/workflows/fuzz.yml`). 60s per
   target on PR (path-filtered so unrelated PRs don't compile the
   fuzz harness), 15 min per target on daily 07:00 UTC cron, manual
   dispatch supported. Matrix over `parse_video_tag` and
   `parse_audio_tag`. `continue-on-error: true` during Tier 1.
   Crash artifacts and corpora upload unconditionally with 30-day
   retention.

7. **cargo-audit CI job** (`.github/workflows/ci.yml`).
   `continue-on-error: true`, separate `audit-v1` cache key. Step
   failures surface honestly in the Checks tab without blocking
   PRs. Promote to required once the baseline is clean.

8. **5-artifact contract enforcement** (`scripts/check_test_contract.sh`
   plus `.github/workflows/contract.yml`). Portable bash script
   (no `globstar` / bash 4+ features; runs on macOS bash 3.2). Walks
   the in-scope crate list, checks each of the five slots, emits
   GitHub Actions warning annotations on missing slots. Soft-fail
   during Tier 1; flipped to strict via `LVQR_CONTRACT_STRICT=1` in
   Tier 2. Per-crate E2E exemption via
   `CONTRACT_E2E_EXEMPT_<crate_with_underscores>=1`. Current state:
   `lvqr-ingest` satisfies all 5 slots; `lvqr-record` satisfies 3/5
   (missing fuzz and conformance slots).

9. **Playwright E2E scaffold** (`tests/e2e/`,
   `.github/workflows/e2e.yml`). Shell-level specs over the test-app
   rendered through `python3 -m http.server`. Three specs covering
   the three-tab navigation, the Watch-tab video element and
   broadcast input, and the Stream-tab form reachability. Tier 1
   scope: no live LVQR binary. Tier 2 extends the
   `playwright.config.ts` webServer array with a `cargo run` entry
   and specs assert on buffered media.

10. **Admin HTTP + JWT integration tests**
    (`crates/lvqr-cli/tests/auth_integration.rs`). Six tests driving
    `TestServer` with three auth providers (Noop, StaticAuthProvider,
    JwtAuthProvider) over a hand-rolled HTTP/1.1 client on raw
    `tokio::net::TcpStream`. Closes the
    `tracking/AUDIT-READINESS-2026-04-13.md` gap: "JWT provider is
    wired into the CLI but has no integration test ... no test
    verifies that `lvqr-cli serve --jwt-secret foo` actually
    validates a real JWT end-to-end". Covers: open access happy
    path, static token missing/wrong/correct, JWT good token, JWT
    wrong secret, JWT insufficient scope, JWT expired. Mints tokens
    via `jsonwebtoken::encode` using `lvqr_auth::JwtClaims` directly
    so the test cannot drift from the production claim schema. First
    integration-level coverage of the admin HTTP layer at all.

## Bonus security fix: `sanitize_name` path-traversal bypass

The `lvqr-record` proptest for `sanitize_name` (added in session 3 as
part of item #5 above) failed on its first run with minimal repro
`".\0."`. The old ordering stripped control characters *after* the
`..` replacement pass, so deleting `\0` regenerated the traversal
sequence `..` from `.\0.`. An attacker-supplied broadcast name like
`"..\0.."` would sanitize to `"...."`, and `"..\0..\0etc\0passwd"`
would sanitize to `"....etc..passwd"` — both still containing `..`.

**Fix**: reorder so control-char stripping runs first, then `/`, `\`,
and `..` replacement. The prior ordering's unit test
(`sanitize_strips_path_traversal` in `recorder.rs`) was not wrong,
just incomplete: it only exercised a literal `"../etc/passwd"` which
the old code did catch. The proptest found the class of input the
unit test missed in under a second. Minimal repro pinned in
`crates/lvqr-record/tests/proptest_recorder.proptest-regressions`
for replay on every future run.

This is the clearest Tier 1 validation that the 5-artifact contract
pays for itself: adding one proptest to a crate that already had a
passing unit test suite surfaced a real security bypass that had
been latent across multiple releases.

## Tier 1 work list status (end of session 3)

| Item | Status |
|---|---|
| 1. TestServer in `lvqr-test-utils` | DONE |
| 2. `lvqr-signal` validators + integration test | DONE |
| 3. Proptest for `extract_resolution` and catalog JSON | DONE |
| 4. Nightly cargo-fuzz CI | DONE |
| 5. `cargo audit` in CI | DONE (soft-fail) |
| 6. `lvqr-conformance` fixture corpus bootstrap | BLOCKED (ffmpeg missing locally) |
| 7. 5-artifact CI enforcement script | DONE (educational mode) |
| 8. Playwright `tests/e2e/` scaffolding | DONE (shell-only) |
| bonus: `lvqr-record` proptest + `sanitize_name` fix | DONE |
| bonus: JWT + static admin auth integration tests | DONE |
| bonus: first integration coverage of admin HTTP layer | DONE |

The load-bearing Tier 2 architectural call
(`lvqr-fragment` + `lvqr-moq` facade, roadmap decisions 1 and 2)
remains explicitly the next target now that Tier 1 is substantially
closed. Item 6 is the only remaining Tier 1 blocker and needs an
ffmpeg-equipped host for one session to capture fixture bytes.

## Known debt and honest limitations after session 3

These are not bugs; they are tracked follow-ups a future session
should be aware of so nothing is discovered twice.

- **`start()` fire-and-forget tasks**: the optional recorder task
  and the mesh reaper task are spawned outside the outer
  `tokio::join!` in `lvqr_cli::start`. Both respect the shared
  shutdown token and exit cleanly, but `ServerHandle::shutdown().await`
  does not block on them. In practice fine (they are short-lived
  after cancellation), but tests that inspect recorder output after
  shutdown must drive the recorder directly rather than through
  `TestServer`. See `crates/lvqr-record/tests/record_integration.rs`
  for the direct-drive pattern.
- **`lvqr-record` contract slots**: after session 3, lvqr-record
  satisfies proptest, integration, and (via the workspace E2E)
  the e2e slot of the 5-artifact contract. The fuzz and conformance
  slots are still open. Fuzz is low-marginal-value (the helpers are
  already proptest-covered); conformance requires ffprobe against
  recorded segments and is a natural follow-up once a session has
  ffmpeg available.
- **`scripts/check_test_contract.sh` cross-crate E2E attribution**:
  the script accepts workspace-level `tests/e2e/**/*.spec.ts` as
  satisfying the e2e slot for any in-scope crate. This is over-
  permissive during Tier 1 and should be tightened in Tier 2 via
  the `CONTRACT_E2E_EXEMPT_<crate>` knob plus a per-crate e2e
  convention (e.g. `tests/e2e/<crate-name>/*.spec.ts`).
- **`docs/architecture.md` and `docs/quickstart.md` are stale**
  per `tracking/AUDIT-READINESS-2026-04-13.md`. Architecture still
  says `tokio::select!` for the CLI server composition; the Tier 0
  fix was `tokio::join!`. Quickstart references a `/watch/*` admin
  endpoint that does not exist. `CONTRIBUTING.md` crate list is
  missing `lvqr-auth`, `lvqr-record`, `lvqr-conformance`. None of
  this affects CI; it is a dedicated docs pass for Tier 5.
- **`lvqr-cli` stale deps**: `rcgen`, `rustls`, `serde`,
  `serde_json`, `futures`, and `toml` are declared in
  `crates/lvqr-cli/Cargo.toml` as normal deps but the new
  `lib.rs` + `main.rs` don't use them directly (they were
  dependencies of the old 930-line `main.rs`). Harmless but
  worth a cleanup pass once the Tier 2 rewrite of the CLI
  composition root settles.
- **Admin-level hardening (Tier 3)**: `/metrics` is intentionally
  unauthenticated for Prometheus scraping; `CorsLayer::permissive()`
  is applied workspace-wide; admin auth middleware does not emit
  `lvqr_auth_failures_total{entry="admin"}`; no rate limiting
  anywhere. All four are already tracked in
  `tracking/AUDIT-INTERNAL-2026-04-13.md` as Tier 3 work.
- **Dead code in lvqr-core: DELETED in session 4** alongside the
  Tier 2.1 landing. `Registry`, `RingBuffer`, `GopCache`, and the
  `Gop` struct are gone. The remaining surface is `Frame`,
  `TrackName`, `StreamId`, `SubscriberId`, `RelayStats`, `EventBus`,
  `RelayEvent`. `StreamId`/`SubscriberId` are still dead (no
  external consumers) but were deliberately kept to avoid scope
  creep in this commit; they should be deleted in a later cleanup
  pass if they remain unused.
- **`lvqr-wasm`**: entire crate is self-deprecated. Scheduled for
  removal in v0.5. CI still builds it.
- **Still-open 5-artifact slots after session 5 (educational mode,
  not blocking)**: fuzz for `lvqr-record`, `lvqr-moq`,
  `lvqr-fragment`, `lvqr-cmaf`; conformance for `lvqr-moq`,
  `lvqr-fragment`, `lvqr-codec`. `lvqr-ingest` is 5/5; `lvqr-record`,
  `lvqr-codec`, and `lvqr-cmaf` are 4/5. Fuzz is low-marginal-value
  for the facade + fragment + cmaf types (they are pure value
  types or stateful shims with no parser attack surface).
  Conformance for `lvqr-codec` is the single most obvious next
  slot to close: pin a handful of real encoder-captured HEVC SPS
  and AAC ASC byte blobs plus their expected decoded values,
  reusing the x265 fixture already in
  `parse_sps_decodes_real_x265_single_sublayer` as the seed.

## Recommended Tier 2.3 entry point (session 10)

Session 9 closed the big-ticket item from the session-9 list (the
raw-sample `TrackCoalescer`). Session 10 inherits the two
remaining items plus two follow-ups the coalescer surfaced.

1. **Add `CmafSegmenter::from_sample_stream`** plus a `SampleStream`
   trait. Scaffold:
   ```text
   pub trait SampleStream: Send {
       fn next_sample<'a>(&'a mut self)
           -> Pin<Box<dyn Future<Output = Option<RawSample>> + Send + 'a>>;
       fn meta(&self) -> &FragmentMeta;
   }
   ```
   The `CmafSegmenter::from_sample_stream` constructor owns a
   `HashMap<u32, TrackCoalescer>` keyed by track id and pulls
   samples via the trait, routing each into its track's
   coalescer. `next_chunk` returns the next flushed chunk across
   any track. This lets the lvqr-hls router consume a real
   producer (once one emits `RawSample` values) without an extra
   adapter layer.

2. **Wire `lvqr-cli serve` to compose HLS**. Add an `--hls-addr`
   flag to `ServeConfig`, have `lvqr_cli::start` spin up an axum
   binding on the address with `HlsServer::router()`, and teach
   the RTMP bridge to push CmafChunks into the `HlsServer`. The
   RTMP bridge today emits pre-muxed `Fragment` values; session
   10 can route those through the pass-through `CmafSegmenter`
   (no coalescer needed yet) and then into `HlsServer::push_chunk_bytes`.
   Day-one E2E: `lvqr-test-utils::TestServer::hls_url()` plus a
   real tokio HTTP client that publishes RTMP and GETs
   `/playlist.m3u8`, asserting the returned playlist contains
   the RTMP-ingested broadcast's segments.

3. **Retire the hand-rolled
   `lvqr-ingest::remux::fmp4::video_segment` writer behind a
   feature flag**. The session-7 parity gate proves the cmaf
   path is structurally equivalent for the AVC init segment; the
   session-9 coalescer now has ffprobe-validated media segment
   output too. The migration is a feature flag on `lvqr-cli` (or
   `lvqr-ingest`) that switches between the two writers. CI
   matrix runs with both settings; when both are green on main,
   the hand-rolled path moves to a `legacy-fmp4` feature gate
   and eventually to deletion.

4. **Audio coalescing round trip**. Extend
   `conformance_coalescer.rs` with an AAC variant so the
   audio-side coalescer state is covered by a real ffprobe run,
   not just the shared unit tests. Blocker today is that the AAC
   init writer refuses non-indexable sample rates; the test can
   pick 44.1 kHz or 48 kHz to stay on the happy path.

5. **Session 7 byte-diff followups against the hand-rolled
   writer**. Expand `parity_avc_init.rs` into
   `parity_avc_segment.rs` that compares coalescer output against
   `lvqr-ingest::remux::fmp4::video_segment` for the same sample
   sequence. If the bytes match structurally, the feature flag
   migration in item 3 is low-risk; if they differ, document
   the harmless differences and pin the structural assertions.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-whep`, `lvqr-srt`,
`lvqr-rtsp`, or `lvqr-archive` until the `CmafSegmenter::from_sample_stream`
constructor and the `lvqr-cli` HLS composition above are in place.
Every egress beyond LL-HLS needs the coalescer to produce chunks
from arbitrary sample sources, not just the RTMP pre-muxed path.

## Recommended Tier 2.3 entry point (session 6, closed)

Session 6 closed item 3 from this list (grow `lvqr-cmaf` beyond AVC,
with the same ffprobe conformance harness extended to HEVC and AAC).
Items 1, 2, and 4 are now deferred to session 7 per the list above.
The original item text is preserved here for historical reference:

1. **Bootstrap the `lvqr-conformance` fixture corpus** now that
   ffmpeg is available locally. This has been BLOCKED since
   session 3 and unblocks codec conformance, HLS comparison
   harnesses, and the DASH path when those land. Capture a small
   matrix of FLV, fMP4, H.264, HEVC, and AAC bytes under
   `crates/lvqr-conformance/fixtures/` via `ffmpeg -f lavfi` and
   pin them into the corpus with a per-fixture `metadata.toml`
   stating the expected parser outputs.
2. **Add a codec conformance slot to `lvqr-codec`** using the new
   fixture corpus. Parser outputs (profile, level, resolution,
   sample rate, channel count) should match the corpus metadata
   exactly. This closes the last educational warning on
   `lvqr-codec` per `scripts/check_test_contract.sh`.
3. **Grow `lvqr-cmaf` beyond AVC**: add `write_hevc_init_segment`
   and `write_aac_init_segment` using `mp4-atom`'s `Hev1` / `Hvcc`
   / `Mp4a` / `Esds` types plus the new `lvqr_codec::hevc` and
   `lvqr_codec::aac` decoded values. Wire the cmaf init writer
   into `rtmp_ws_e2e` in parallel with the hand-rolled writer
   and diff the bytes as the first byte-level proof that
   `mp4-atom` output is a drop-in replacement for the current
   writer.
4. **Multi-sub-layer HEVC fixture capture**: try `kvazaar` or an
   nvenc-based HEVC encoder rather than x265 to get a real
   `sps_max_sub_layers_minus1 > 0` SPS on disk. If none of the
   available encoders produce one, capture an HEVC SPS from a
   publicly licensed sample (Apple bipbop? Big Buck Bunny HEVC
   rendition?) and pin the bytes.

Do NOT start any Tier 2 egress protocol crate (`lvqr-whip`,
`lvqr-whep`, `lvqr-hls`, `lvqr-dash`, `lvqr-srt`, `lvqr-rtsp`,
`lvqr-archive`) until `lvqr-cmaf` has a working segmenter that
emits real `moof + mdat` bytes from raw samples. The scaffold
landed in session 5 is a pass-through that annotates pre-muxed
fragments; the sample-coalescer is the actual Tier 2.3 load-bearing
piece.

---
**E2E Verified**: real RTMP publish -> RtmpMoqBridge -> MoQ origin -> axum WS
relay -> tungstenite WebSocket client, with fMP4 init (ftyp) and media (moof)
segments verified byte-by-byte. See `crates/lvqr-cli/tests/rtmp_ws_e2e.rs`.

The roadmap at `tracking/ROADMAP.md` is the authoritative plan for the next
18-24 months of work; read it alongside CLAUDE.md before starting anything.
Three audits sit next to it:

- `tracking/AUDIT-2026-04-13.md` (external) compares LVQR's current
  surface area against MediaMTX, LiveKit, OvenMediaEngine, SRS, Ant
  Media, AWS Kinesis Video Streams, Janus, and Jitsi, and calibrates
  the five strategic bets.
- `tracking/AUDIT-INTERNAL-2026-04-13.md` (internal) is the dead-code,
  latent-bug, and security-hardening audit of LVQR itself. Every
  critical claim was manually verified before landing. Five fixes
  shipped the same session.
- `tracking/AUDIT-READINESS-2026-04-13.md` (readiness) audits CI
  wiring, supply-chain, documentation drift, unwired CLI surface,
  and Tier 0/1 progress against the roadmap. Five fixes landed:
  README refresh, ffmpeg installed in CI, `--config` dead flag
  removed, plus this document.

## What Tier 0 Closed (2026-04-13)

The v0.4 audit found real bugs hiding behind v0.3.1's "green CI" claim. Tier 0
addressed each of them; the state today is:

1. **Graceful shutdown race fixed.** `crates/lvqr-cli/src/main.rs` now runs
   the relay, RTMP, and admin subsystems via `tokio::join!` with per-subsystem
   wrappers that cancel the shared token on exit. The outer `select!` arm
   that pre-empted draining subsystems on ctrl-c is gone.
2. **EventBus wired end-to-end.** `lvqr_core::EventBus` is created once in
   the CLI and handed to `RtmpMoqBridge::with_events`. The RTMP bridge emits
   `BroadcastStarted/Stopped` on publish/unpublish; the WS ingest handler
   emits the same events around its session; `spawn_recordings` subscribes
   to the bus instead of polling `bridge.stream_names()`, so WS-ingested
   broadcasts are recorded identically to RTMP-ingested ones.
3. **Player audio SourceBuffer fix.** Both `@lvqr/player` and the test app's
   watch tab now set `sb.mode = 'sequence'` only for video (`0.mp4`); audio
   stays in the default `segments` mode so fMP4 `baseMediaDecodeTime` is
   honored and A/V stays in lock.
4. **Tokens out of query strings.** WebSocket auth now travels in
   `Sec-WebSocket-Protocol: lvqr.bearer.<token>`. The new `resolve_ws_token`
   helper in `lvqr-cli` parses the header and echoes the exact subprotocol
   back so axum's upgrade handshake completes. `?token=` is still accepted
   during the transition but logs a deprecation warning per upgrade. The
   JS client (`bindings/js/packages/core/src/client.ts`) and the test app
   construct their WebSockets with the subprotocol array when a token is
   set, and the test app grew token inputs on both Watch and Stream tabs.
5. **Pluggable protocol scaffolding and auth crate.** `lvqr-auth` is a new
   crate with `AuthProvider`, `StaticAuthProvider`, `NoopAuthProvider`, and
   an optional `JwtAuthProvider` behind the `jwt` feature. `lvqr-ingest`
   gained `IngestProtocol` + an `RtmpIngest` adapter. `lvqr-relay` gained
   a mirror `RelayProtocol` trait. The object-safety mock test that the
   audit flagged as theatrical is gone.
6. **Real RTMP to WS E2E test.** `crates/lvqr-cli/tests/rtmp_ws_e2e.rs`
   drives a real rml_rtmp publisher through the bridge, subscribes via a
   real tokio-tungstenite WebSocket client, and asserts both an init
   segment (`ftyp`) and a media segment (`moof`) arrive over the wire.
   Zero mocks, zero helper-in-isolation assertions.

## Breaking Changes vs 0.3.1

- **WS auth transport**: prefer `Sec-WebSocket-Protocol: lvqr.bearer.<token>`
  over `?token=`. The query-string form still works but logs a deprecation
  warning and is scheduled for removal in a future release.
- **Recorder eligibility**: anything ingested over WebSocket is now recorded
  when `--record-dir` is set; previously only RTMP-ingested streams were.

## Next Up: Tier 1 (Test Infrastructure)

Per the roadmap, Tier 0 unblocks Tier 1: build the reference fixture corpus,
proptest harnesses, cargo-fuzz targets, testcontainers fixtures, playwright
E2E, ffprobe validation in CI, and the MediaMTX comparison harness. The
load-bearing architectural call after that (Tier 2) is the Unified Fragment
Model in `crates/lvqr-fragment/` and the `lvqr-moq` facade crate -- do NOT
add new protocol code before those two land.

The audit reorders two Tier 1 items:

1. The MediaMTX cross-implementation comparison harness graduates to a
   first-day CI requirement for Tier 2.5 (LL-HLS) rather than a late
   Tier 1 add-on. Bake it into `lvqr-conformance` during Tier 1 so
   Tier 2.5 does not have to build it later.
2. `lvqr-conformance` and the proptest/cargo-fuzz harnesses ship before
   `lvqr-chaos`. Chaos testing is valuable but does not block Tier 2
   the way the conformance corpus does.

## Tier 1 Progress as of 2026-04-13

Landed in this session:

1. **`lvqr-conformance` crate skeleton** (`publish = false`). Directory
   layout for fixtures under `fixtures/{rtmp,fmp4,hls,dash,moq,edge-cases}/`,
   `ValidatorResult` type with soft-skip semantics, README documenting
   the provenance metadata every fixture must ship with.
2. **Proptest harness** for `lvqr-ingest` parsers and fMP4 writer at
   `crates/lvqr-ingest/tests/proptest_parsers.rs`. `parse_video_tag` and
   `parse_audio_tag` tested to never panic across 1024 cases each;
   `video_init_segment_with_size` and `video_segment` tested to produce
   structurally well-formed ISO BMFF buffers across 256 cases each
   (2560 generated cases total, all green).
3. **Golden-file regression** for the fMP4 writer at
   `crates/lvqr-ingest/tests/golden_fmp4.rs` with two fixtures under
   `crates/lvqr-ingest/tests/fixtures/golden/`. `BLESS=1` regenerates
   both after intentional format changes.
4. **`ffprobe_bytes` helper** in `lvqr-test-utils` with
   `FfprobeResult::{Ok, Skipped, Failed}`. Tests soft-skip when ffprobe
   is not on PATH so contributor laptops without ffmpeg do not break CI.
5. **ffprobe wired into the golden fMP4 test** via a new
   `ffprobe_accepts_concatenated_cmaf` case that feeds the init segment
   plus a keyframe media segment into ffprobe and asserts acceptance.
6. **cargo-fuzz scaffold** for the FLV parsers at
   `crates/lvqr-ingest/fuzz/`, excluded from the main workspace so
   stable builds do not pull libfuzzer-sys. Two targets:
   `parse_video_tag` and `parse_audio_tag`. Nightly-only; runs via
   `cargo +nightly fuzz run <target>`.
7. **5-artifact test contract** documented at `tests/CONTRACT.md` with
   a table tracking each in-scope crate's current coverage of the
   five required artifacts (proptest, fuzz, integration, E2E,
   conformance). Educational during Tier 1; hard CI gate from Tier 2.

After this session, `lvqr-ingest` has four of the five artifacts for
its parsers and writers: proptest (new), cargo-fuzz (new, nightly),
integration (existing RTMP bridge test), and conformance (new golden
plus ffprobe). The fifth slot (browser E2E) is covered transitively
by the `lvqr-cli` rtmp_ws_e2e test. No other crate has full coverage yet.

## Internal Audit Fixes (2026-04-13)

The internal audit identified confirmed bugs, dead code, and hardening
targets. Five items landed in the same commit as the audit document:

1. **Broadcast path traversal hardening** in `lvqr-relay::parse_url_token`.
   A new `is_valid_broadcast_name` validator rejects names containing
   `..`, backslash, control characters, leading/trailing slashes, or
   anything outside `[A-Za-z0-9._/-]`. Empty names remain permitted
   because MoQ sessions legitimately connect to the relay root and
   select broadcasts via SUBSCRIBE. Six new unit tests, plus the five
   existing relay integration tests continuing to pass.
2. **Stale child reference fix** in `lvqr-mesh::reassign_peer`. The
   function overwrote the peer's parent field but never removed the
   stale child reference from the old parent's children list. Latent
   bug that only triggers on live rebalance (the orphan path calls
   `remove_peer` first which deletes the old parent entirely). Defensive
   fix plus a new regression test for the live-rebalance path.
3. **Theatrical heartbeat test replaced** in `lvqr-mesh`. The prior
   version set `heartbeat_timeout_secs = 0` and asserted nothing
   meaningful. New version exercises the full lifecycle: fresh peer
   alive, stale after 1.1s sleep, alive again after heartbeat.
4. **JWT provider wired into the CLI**. `JwtAuthProvider` was
   feature-complete but had zero consumers outside its own unit tests.
   `lvqr-cli` now pulls `lvqr-auth` with the `jwt` feature on and
   exposes `--jwt-secret` / `--jwt-issuer` / `--jwt-audience` plus
   matching `LVQR_JWT_*` env vars, taking precedence over static
   tokens.
5. **lvqr-mesh scaffolding comment** at the top of `crates/lvqr-mesh/src/lib.rs`
   making it explicit that the crate is a topology planner and no
   code in the repo yet drives real WebRTC DataChannel peer forwarding.
   The offload percentage exposed via the admin API is intended
   offload, not actual. Documentation change only.

Plus a new Tier 1 test that closes one of the audit's deferred items:

6. **lvqr-record integration test** at
   `crates/lvqr-record/tests/record_integration.rs`. Drives a
   synthesized MoQ broadcast through a real `record_broadcast` call
   in a tempdir and asserts the on-disk layout matches the documented
   structure. Also verifies that cancellation returns Ok cleanly within
   a timeout. Before this test, `record_track` had zero integration
   coverage; only the pure helpers (`looks_like_init`, `track_prefix`,
   `sanitize_name`) were tested.

## Readiness Audit Fixes (2026-04-13)

A third audit pass focused on readiness: what a new contributor or
future session encounters when they sit down to work. Five fixes
landed in the same commit as `tracking/AUDIT-READINESS-2026-04-13.md`:

1. **README refreshed** to v0.4-dev. Removed the stale "83 Rust
   tests, no auth, no recording" claims. Added current crate list
   including `lvqr-auth`, `lvqr-record`, `lvqr-conformance`. Added
   a pointer at the three audit documents and the roadmap.
2. **ffmpeg installed in CI** on both the Linux and macOS legs of
   the test matrix via apt and brew respectively. Before this
   change, the `ffprobe_accepts_concatenated_cmaf` test landed in
   Tier 1 kickoff silently soft-skipped on every CI run because
   ffprobe was not on PATH.
3. **`cargo test --workspace`** used on both matrix legs (previously
   split into `--lib` and `--test '*'` which skipped doc tests).
   Doctests in `lvqr-auth` and `lvqr-ingest::protocol` now run.
4. **Verify-ffprobe step** added to CI so if the ffmpeg install
   silently succeeds but ffprobe is not on PATH we fail fast with
   a loud error instead of silently skipping the conformance
   check.
5. **Dead `--config` CLI flag removed** from `lvqr-cli::ServeArgs`.
   The flag was declared but never read, leaking into `--help`,
   the README, the quickstart, and CONTRIBUTING as a capability
   lie. Will be re-added with a real loader alongside the Tier 3
   hot config reload work.

Tracked by the audit for later (not fixed this commit):

- `docs/architecture.md` still says `tokio::select!` for the CLI
  server composition. The Tier 0 fix was `tokio::join!`. Dedicated
  docs pass in Tier 5.
- `docs/quickstart.md` references a `/watch/my-stream` endpoint
  that does not exist.
- `CONTRIBUTING.md` crate list missing `lvqr-auth`, `lvqr-record`,
  `lvqr-conformance`, and references a `docker/docker-compose.test.yml`
  that does not exist.
- No cargo-audit job in CI. Supply-chain CVE scan deferred.
- No nightly cargo-fuzz runner wired up. The fuzz targets exist and
  compile under nightly but nothing runs them on a schedule.
- No playwright E2E suite. No 5-artifact CI enforcement script.

## Tier 1 Remaining Work

Big-ticket items still to build:

- `TestServer` in `lvqr-test-utils` that spawns a full LVQR binary (or
  calls `lvqr_cli::serve` directly once the CLI crate exposes a lib)
  with ephemeral ports and cleanup. Replaces ad-hoc server setup in
  every test file.
- testcontainers fixtures for MinIO (S3-compatible object storage),
  needed for the Tier 2.4 archive crate.
- `tests/e2e/` playwright suite that drives a real Chrome against the
  test app to exercise ingest plus playback. Trace recording on
  failure. Gating for the audio A/V drift soak test the audit calls
  out.
- ffprobe-backed validation of every fMP4 output in every test,
  swapping hand-rolled structural assertions for the external
  validator where practical.
- `mediastreamvalidator` wrapper in `lvqr-conformance` that runs Apple's
  HLS validator against generated playlists. Blocks on Tier 2.5 existing.
- Cross-implementation comparison harness: same RTMP into LVQR and
  MediaMTX, structural diff of HLS playlists. Blocks on Tier 2.5.
- 24-hour soak rig that runs synthetic publisher plus subscribers and
  asserts no memory growth, no FD leaks, no gauge drift.
- `lvqr-loadgen` crate for Rust-native data-plane load generation.
- `lvqr-chaos` crate for fault injection. Lowest priority per the audit.
- CI enforcement script for the 5-artifact contract. Educational PR
  comments in Tier 1, hard fail in Tier 2.

The `lvqr-fragment` and `lvqr-moq` crates from Tier 2.1 remain the
load-bearing architectural call. Do not ship new protocol code before
those two land. Read `tracking/AUDIT-2026-04-13.md` for the full
competitor comparison and the five strategic bets before arguing about
priority.

## End-to-End Pipeline (Proven Working)

## End-to-End Pipeline (Proven Working)

## End-to-End Pipeline (Proven Working)

```
Browser Webcam (getUserMedia)
    |
    v
VideoEncoder (H.264 Baseline, WebCodecs API)
    |
    v
WebSocket (/ingest/{broadcast}) -- [type][timestamp][AVCC NALUs]
    |
    v
LVQR Server (Rust)
  - Parses AVCC config (SPS/PPS, width/height)
  - Generates fMP4 init segment (ftyp+moov with avcC, correct dimensions)
  - Remuxes H.264 NALUs to fMP4 media segments (moof+mdat)
  - Publishes to MoQ tracks via OriginProducer
    |
    v
WebSocket (/ws/{broadcast}) -- forwards fMP4 binary frames
    |
    v
Browser Viewer (MSE)
  - Auto-detects codec from avcC box in init segment
  - Creates SourceBuffer in sequence mode
  - Chases live edge (seeks when >500ms behind)
  - Plays video
```

Also works with RTMP ingest (OBS/ffmpeg) via the same fMP4 remux pipeline.

## All Packages Published

### crates.io (8 crates)
lvqr-core, lvqr-signal, lvqr-relay, lvqr-ingest, lvqr-mesh, lvqr-admin, lvqr-wasm, lvqr-cli @ 0.3.1

### npm (2 packages)
@lvqr/core, @lvqr/player @ 0.3.1

### PyPI
lvqr @ 0.3.1

## Repository Structure

```
lvqr/
  crates/
    lvqr-core/          Shared types, ring buffer, GOP cache (25 tests)
    lvqr-relay/         MoQ relay on moq-lite, connection callbacks (4 integration tests)
    lvqr-ingest/        RTMP server + FLV-to-CMAF remuxer (26 lib + 2 integration tests)
    lvqr-mesh/          Peer tree coordinator (13 tests)
    lvqr-signal/        WebRTC signaling + mesh push (7 tests)
    lvqr-admin/         HTTP API: stats, streams, mesh (6 tests)
    lvqr-wasm/          WebTransport browser bindings
    lvqr-cli/           Single binary: relay + RTMP + admin + WS relay/ingest + mesh
    lvqr-test-utils/    Test helpers (publish = false)
  bindings/
    js/packages/
      core/             MoQ client, admin client, mesh peer (@lvqr/core)
      player/           <lvqr-player> web component (@lvqr/player)
    python/             Admin API client (lvqr on PyPI)
  test-app/             Brutalist test UI: stream, watch, admin
  tracking/             Handoff, audit, session notes
```

## Test App

`test-app/index.html` -- single-page brutalist test app with three tabs:

- **Stream**: Webcam capture via WebCodecs VideoEncoder (H.264 Baseline, level 4.0), streams over WebSocket to `/ingest/{broadcast}`. No ffmpeg or OBS needed.
- **Watch**: WebSocket fMP4 viewer with MSE SourceBuffer. Auto-detects codec from avcC box. Chases live edge to keep latency low.
- **Admin**: Real-time dashboard polling `/healthz`, `/api/v1/stats`, `/api/v1/streams`, `/api/v1/mesh`. Auto-refresh toggle.

Run:
```bash
lvqr serve                              # terminal 1
cd test-app && python3 -m http.server 9000  # terminal 2
# open http://localhost:9000
```

## Key Endpoints

| Endpoint | Protocol | Description |
|----------|----------|-------------|
| `:4443` | QUIC/UDP | MoQ relay (WebTransport/QUIC subscribers) |
| `:1935` | TCP | RTMP ingest (OBS/ffmpeg) |
| `:8080/healthz` | HTTP GET | Health check |
| `:8080/api/v1/stats` | HTTP GET | Publisher/subscriber/track counts |
| `:8080/api/v1/streams` | HTTP GET | Active stream list |
| `:8080/api/v1/mesh` | HTTP GET | Mesh peer count, offload % |
| `:8080/ws/{broadcast}` | WebSocket | fMP4 viewer relay (binary frames) |
| `:8080/ingest/{broadcast}` | WebSocket | Browser H.264 ingest |
| `:8080/signal` | WebSocket | WebRTC signaling for mesh peers |

## WS Ingest Wire Format

Binary WebSocket messages: `[u8 type][u32 BE timestamp_ms][payload]`

| Type | Payload |
|------|---------|
| 0 | Config: `[u16 BE width][u16 BE height][AVCDecoderConfigurationRecord]` |
| 1 | Keyframe: AVCC-format NALUs (length-prefixed) |
| 2 | Delta frame: AVCC-format NALUs |

The AVCC record comes from `VideoEncoder.output()` metadata's `decoderConfig.description`. NALUs use `avc: { format: 'avc' }` (length-prefixed, not Annex B).

## All Bugs Found and Fixed (12 total)

### Protocol bugs (found by reading moq-lite source, no browser needed)

| # | Bug | Impact |
|---|-----|--------|
| 1 | CLIENT_SETUP size: varint instead of u16 BE | Every MoQ connection fails |
| 2 | Path encoding: segmented array instead of plain string | Every subscribe returns NotFound |
| 3 | AnnouncePlease: 1 empty segment instead of 0 | Discovery returns nothing |
| 4 | Subscribe priority: varint instead of u8 | Misparse for priority > 63 |
| 5 | trun box version 0 for signed CTS | B-frame timestamps wrong |
| 6 | Player hardcoded codec string | Non-High-profile H.264 fails |
| 7 | Video+audio to single MSE SourceBuffer | MSE crash on first audio frame |

### E2E bugs (found during live browser testing)

| # | Bug | Impact |
|---|-----|--------|
| 8 | Init segment width=0 height=0 in avc1/tkhd | Chrome MSE rejects init |
| 9 | Duplicate init segments (stored + group frame 0) | MSE error after first append |
| 10 | No live-edge seeking | Unbounded latency growth |
| 11 | H.264 level 3.0 for 720p capture | VideoEncoder refuses to encode |
| 12 | No CORS headers on admin HTTP | Admin tab fetch() blocked |

## What Works (verified)

| Feature | How Verified |
|---------|-------------|
| Webcam -> browser -> LVQR -> browser viewer | E2E in Chrome |
| RTMP ingest (OBS/ffmpeg) -> fMP4 -> MoQ | 2 integration tests |
| FLV parsing (SPS/PPS, AAC config) | 12 unit tests |
| fMP4 generation (init + media segments) | 10 unit tests |
| MoQ QUIC fan-out (1 pub, 3 subs) | 3 integration tests |
| Relay connection callback | 1 integration test |
| Mesh tree assignment + orphan reassignment | 13 unit tests |
| Signal server message routing + push | 7 unit tests |
| Admin API (stats, streams, mesh) | 6 unit tests |
| Admin dashboard in browser | Manual test |
| Registry fanout benchmark | ~230ns to 500 subscribers |

## What's Not Done

| Feature | Status |
|---------|--------|
| MoQ/WebTransport browser path | Code written (SETUP, Subscribe, Group/Frame), WS fallback works, WebTransport path untested |
| WebRTC mesh media relay | Coordination works (tree + signal push), DataChannel code written, relay untested |
| Audio playback | Separate MSE SourceBuffer needed, not wired in player |
| Stream authentication | Not started |
| Recording | Not started |
| Multi-server federation | Not started |

## Key Technical Decisions

- **FLV-to-CMAF remux** in Rust (manual fMP4 box writer, no external crate). AVCC NALUs pass through unchanged since both FLV and fMP4 use length-prefixed format.
- **WebSocket for browser ingest** because browsers can't do RTMP (no TCP sockets). WebCodecs VideoEncoder provides hardware H.264 encoding.
- **WebSocket for browser playback** as fallback because MoQ/WebTransport version negotiation hasn't been E2E tested. The WS path is proven working.
- **Init segment per group** in MoQ tracks so late-joining subscribers always get codec config. The WS relay skips duplicate init segments to avoid MSE errors.
- **Live-edge chasing** in the viewer (seek forward when >500ms behind) because MSE in sequence mode accumulates buffer without bound.
- **CORS permissive** on the admin/WS server for development. Should be restricted in production.
