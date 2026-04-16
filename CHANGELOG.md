# Changelog

All notable changes to LVQR are documented in this file.

## [0.4.0] - 2026-04-16

M1 milestone: single-binary live video server with RTMP + WHIP
ingest and LL-HLS + DASH + WHEP + MoQ egress. 420 tests, all
CI green.

### Added

- **LL-HLS sliding-window eviction.** `PlaylistBuilderConfig`
  gains `max_segments: Option<usize>` that caps the rendered
  playlist and purges evicted segment/partial bytes from the
  server cache. Production default is 60 segments (~120 s).

- **`#EXT-X-PROGRAM-DATE-TIME` per segment.** RFC 8216bis
  requires this tag when `CAN-SKIP-UNTIL` is advertised. The
  builder computes each segment's wall-clock time from a
  configurable base timestamp. Per-broadcast anchoring in
  `MultiHlsServer` via `SystemTime::now()` at creation time.

- **`#EXT-X-ENDLIST` and `PlaylistBuilder::finalize`.** When a
  broadcaster disconnects, the playlist gains `#EXT-X-ENDLIST`
  and the retained window becomes a VOD surface. The preload
  hint is suppressed. Idempotent.

- **DASH finalize on disconnect.** `DashServer::finalize()`
  switches the MPD from `type="dynamic"` to `type="static"` and
  omits `minimumUpdatePeriod`. DASH clients stop polling.

- **Broadcaster disconnect wiring.** Both RTMP (`on_unpublish`)
  and WHIP (`on_disconnect`) emit `BroadcastStopped` on the
  event bus. Subscribers finalize both HLS and DASH per-broadcast
  servers. E2E tests verify the full path for both protocols.

- **`--hls-dvr-window <secs>`.** Operator-tunable DVR depth.
  Default 120 s. Set to 0 for unbounded retention. Env:
  `LVQR_HLS_DVR_WINDOW`.

- **`--hls-target-duration <secs>` and `--hls-part-target <ms>`.**
  Configurable segment and partial timing. Flows end-to-end
  through `CmafPolicy`, `PlaylistBuilderConfig`, and
  `ServerControl` (HOLD-BACK, PART-HOLD-BACK, CAN-SKIP-UNTIL
  auto-derived). Env: `LVQR_HLS_TARGET_DURATION`,
  `LVQR_HLS_PART_TARGET`.

- **`--whip-port` and `--dash-port` CLI flags.** Enable WHIP
  ingest and DASH egress on dedicated ports.

- **CORS headers on HLS and DASH routers.** Browser-hosted
  hls.js and dash.js players can fetch playlists and segments
  cross-origin out of the box.

- **`CmafPolicy::with_durations`.** Configurable segment and
  partial duration in milliseconds, converted to timescale ticks
  at construction.

- **`IngestSampleSink::on_disconnect`.** Trait method (default
  no-op) called when a WHIP session ends, enabling cleanup and
  event emission.

- **Criterion bench for `PlaylistBuilder`.** Three bench groups:
  `push_partial` (~630 ns), `push_segment_boundary` (~1 us),
  `render` (~43 us at 60 segments).

- **cargo-fuzz skeletons.** `lvqr-hls` (PlaylistBuilder driver)
  and `lvqr-cmaf` (codec string detector driver).

### Changed

- `collect_coalesce_work` in `HlsServer` switched from
  index-based to sequence-based detection so the closed-segment
  coalesce path stays correct when eviction shrinks
  `manifest.segments` from the front inside the same push.

- `ServerControl` timing parameters auto-derive from the
  configured `target_duration_secs` and `part_target_secs`
  instead of being hardcoded. HOLD-BACK = 3 * target,
  PART-HOLD-BACK = 3 * part, CAN-SKIP-UNTIL = 6 * target.

- `MultiHlsServer::ensure_video` and `ensure_audio` stamp
  `program_date_time_base = SystemTime::now()` per-broadcast
  so every broadcast anchors its PDT independently.

- `run_session_loop` in `lvqr-whip` split into outer + inner
  so `on_disconnect` fires unconditionally on every exit path.

- DASH MPD `minimumUpdatePeriod` attribute is now conditional:
  omitted when the value is empty (finalized broadcasts).

### Fixed

- HLS and DASH HTTP routers now serve CORS headers. Previously
  only the admin router had `CorsLayer::permissive()`.

## [0.3.1] - 2026-04-15

LL-HLS closed-segment cache coalesce fix, lvqr-dash end-to-end,
hls-conformance CI workflow flipped to required.

## [0.3.0] - 2026-04-14

WHIP H.264 + HEVC + Opus end-to-end, WHEP video egress, LL-HLS
master playlist with dynamic codec strings, DVR archive with
redb index, delta playlists, blocking reload.

## [0.2.0] - 2026-04-10

Initial maturity audit. RTMP ingest, MoQ relay, WebSocket
fallback, mesh topology planner, admin API, disk recording.

## [0.1.0] - 2026-04-08

Project scaffold. MoQ relay with QUIC/WebTransport, RTMP ingest
with FLV-to-fMP4 remux.
