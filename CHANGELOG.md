# Changelog

All notable changes to LVQR are documented in this file. The
head of `main` is always the source of truth; this file
summarises user-visible surface changes between tagged
releases. For session-by-session engineering notes, see
`tracking/HANDOFF.md`.

## Unreleased (post-0.4.1)

### Added

* **SCTE-35 ad-break markers in `@lvqr/dvr-player` v0.3.3**
  (session 154). The DVR scrub component now paints session 152's
  `#EXT-X-DATERANGE` entries on its seek bar: vertical ticks for
  CMD / time-signal singletons, coloured break-range spans for
  paired SCTE35-OUT + SCTE35-IN entries (joined by their shared
  DATERANGE `ID`), and a faint in-flight overlay for an OUT whose
  IN has not yet landed. Hover tooltip shows kind, ID, time
  inside the seekable range, and duration when set. New
  `markers="visible"` (default) | `"hidden"` attribute toggles
  the visual layer without suppressing events. New events
  `lvqr-dvr-markers-changed` (fires on the diff vs the prior
  LEVEL_LOADED pass) and `lvqr-dvr-marker-crossed` (fires per
  ID when `currentTime` crosses a marker, debounced 100 ms per
  ID). New `getMarkers()` programmatic method returns the sorted
  store + pair groups. Reads markers from hls.js's
  `LevelDetails.dateRanges` (v1.5+) on `LEVEL_LOADED`; trusts
  `DateRange.startTime` for the PDT-anchored time mapping. **No
  Rust crate touched, no new server route, no new HLS tag.**
  CSS hooks: `--lvqr-marker-color`, `--lvqr-marker-tick-color`,
  `--lvqr-marker-in-flight`, `--lvqr-marker-tooltip-bg`. New
  shadow parts: `markers`, `marker-tooltip`. New helper
  `bindings/js/tests/helpers/rtmp-push.ts` (Node ffmpeg wrapper)
  closes session 153's deferred "live-stream-driven Playwright
  assertions" item via a real-publish LIVE-pill activation test.
  `@lvqr/player` and `@lvqr/core` stay at 0.3.2; workspace stays
  at 0.4.1.
* **`@lvqr/dvr-player` web component v0.3.2** (session 153). New
  npm package at `bindings/js/packages/dvr-player/`, sister to
  `@lvqr/player`, drops in as `<lvqr-dvr-player>` for HLS DVR
  scrub against the relay's existing `/hls/{broadcast}/master.m3u8`
  endpoint with the `--hls-dvr-window` sliding-window depth.
  Vanilla `class extends HTMLElement` (structured-vanilla pattern;
  template-literal HTML strings + small attribute helpers + shadow
  DOM + `attributeChangedCallback`-driven reactivity; no Lit, no
  Stencil). Wraps hls.js (`^1.5.0` direct dep). Custom seek bar
  with HH:MM:SS percentile labels (or MM:SS for sub-hour spans),
  LIVE pill toggling on `seekable.end - currentTime` crossing
  `max(6, 3 * #EXT-X-TARGETDURATION)` (configurable via
  `live-edge-threshold-secs`), explicit Go Live button, client-
  side hover thumbnails via canvas `drawImage` against a lazy
  second hls.js instance (LRU-capped at 60 entries; opt-out via
  `thumbnails="disabled"`). Bearer-token auth via hls.js
  `xhrSetup` with query-string fallback for native HLS in Safari
  MSE-less mode. Public events: `lvqr-dvr-seek`,
  `lvqr-dvr-live-edge-changed`, `lvqr-dvr-error`. Programmatic
  API: `play / pause / seek / goLive / getHlsInstance`. ESM-only
  via `tsc`, `MIT OR Apache-2.0`. **No new server route** -- the
  component consumes the existing `/hls/*` surface unchanged.
  32 Vitest unit tests (seekbar arithmetic + attrs helpers +
  typed dispatcher) + 15 Playwright e2e tests (mount + interaction
  flows including pointer drag, keyboard scrub, threshold
  customization, hover preview, programmatic seek + goLive +
  multi-seek event chaining, host-to-document event bubbling).
  New docs at `docs/dvr-scrub.md` covering the operator embedding
  recipe, signed-URL / bearer-token auth precedence, theming via
  CSS custom properties + `::part()` access. `@lvqr/core` and
  `@lvqr/player` stay at 0.3.2; workspace 0.4.1 unchanged; no
  Rust source touched.

* **SCTE-35 ad-marker passthrough v1** (session 152). Splice events
  injected on the publisher side flow ingest -> parser -> parallel
  `"scte35"` track on the existing `FragmentBroadcasterRegistry` ->
  per-broadcast bridge drain -> LL-HLS `#EXT-X-DATERANGE` + DASH
  Period-level `<EventStream>`. Splice_info_section bytes are
  preserved verbatim through both egress wire shapes (hex on HLS,
  base64 inside `<Signal><Binary>` on DASH). The relay never
  interprets splice semantics beyond what the egress wire shapes
  need; ad-decisioning is the operator's responsibility (typically
  via a downstream SSAI proxy that consumes the egress playlists).

  * **Ingest paths**:
    * **SRT MPEG-TS** -- PMT stream_type 0x86 on a dedicated PID
      (typically 0x1FFB by broadcast convention); private-section
      reassembly across TS packet boundaries.
    * **RTMP onCuePoint scte35-bin64** -- the Adobe AMF0 convention
      used by AWS Elemental, Wirecast, vMix, and ffmpeg's
      `-bsf:v scte35` pipeline. Required vendoring `rml_rtmp` v0.8.0
      at `vendor/rml_rtmp/` (MIT-licensed, license preserved) with a
      ~25-line patch that adds an `Amf0DataReceived` ServerSessionEvent
      variant: upstream's `handle_amf0_data` silently drops every AMF0
      Data message that is not `@setDataFrame`-wrapped onMetaData. The
      fork loads via `[patch.crates-io]` in the workspace `Cargo.toml`
      and passes 170 / 0 / 0 tests (168 upstream + 2 LVQR-side
      defensive tests).

  * **Egress wire shapes**:
    * **HLS** (per draft-pantos-hls-rfc8216bis section 4.4.5.1):
      `#EXT-X-DATERANGE` at the playlist head with
      `CLASS="urn:scte:scte35:2014:bin"` (industry convention),
      `START-DATE`, optional `DURATION`, and one of SCTE35-OUT /
      SCTE35-IN / SCTE35-CMD (driven by splice_command_type +
      out_of_network_indicator) carrying the raw splice_info_section
      as `0x...` hex.
    * **DASH** (per ISO/IEC 23009-1 G.7 + SCTE 214-1):
      Period-level `<EventStream
      schemeIdUri="urn:scte:scte35:2014:xml+bin" timescale="90000">`
      with `<Event>` children carrying base64-encoded
      splice_info_section inside a `<Signal><Binary>` body. Rendered
      BEFORE AdaptationSet siblings per spec ordering.

  * **Parser** (`lvqr-codec/src/scte35.rs`): minimum-viable
    splice_info_section decoder. Verifies CRC_32 (MPEG-2 polynomial
    0x04C11DB7); decodes splice_null / splice_insert / time_signal
    command bodies for the timing fields the egress renderers need
    (event_id, pts, break_duration, command_type, cancel,
    out_of_network_indicator); preserves the entire raw section in
    `SpliceInfo::raw` for downstream passthrough.

  * **Wiring**: new `lvqr_fragment::SCTE35_TRACK` reserved track
    name; new `publish_scte35` helper in `lvqr-ingest`; new
    `BroadcasterScte35Bridge` in `lvqr-cli` (mirror of the captions
    bridge); new `MultiHlsServer::push_date_range` and
    `MultiDashServer::push_event` methods.

  * **Counter metrics**:
    * `lvqr_scte35_events_total{ingest, command}` -- sections
      successfully parsed and emitted onto the scte35 track.
    * `lvqr_scte35_drops_total{ingest, reason}` -- sections dropped
      at the parser boundary (CRC mismatch, malformed, truncated).
    * `lvqr_scte35_bridge_drops_total{broadcast, reason}` -- sections
      that reached the cli-side bridge but failed parse on the
      second pass.

  * **Anti-scope**: no semantic interpretation, no SCTE-104, no
    mid-segment splice handling, no transcoder-level mid-stream IDR
    insertion. WHIP / RTSP ingest paths deferred (no widely-adopted
    publisher convention).

  See [`docs/scte35.md`](docs/scte35.md) for the full standards
  reference, ingest table, publisher quickstart, wire shape
  examples, internal architecture, and operator runbook.

* **Hot config reload** (sessions 147 + 148 + 149). New
  `lvqr serve --config <path.toml>` flag points at a TOML file;
  SIGHUP (Unix) and `POST /api/v1/config-reload` (cross-platform)
  re-apply the file atomically without bouncing the relay. The
  five hot-reloadable categories are honored end-to-end:
  - `[auth]` section -- Static + HS256 JWT (147).
  - `mesh_ice_servers` -- the operator's STUN / TURN list pushed
    via `/signal` `AssignParent` (148).
  - `hmac_playback_secret` -- the HMAC-SHA256 key used by live
    HLS / DASH and DVR `/playback/*` `?sig=...&exp=...` (148).
  - `jwks_url` -- JWKS discovery endpoint URL (149, requires
    `--features jwks`).
  - `webhook_auth_url` -- decision-webhook URL (149, requires
    `--features webhook`).

  Implemented via a new `lvqr_auth::HotReloadAuthProvider`
  (always-on `arc_swap::ArcSwap` wrap; single-digit-ns reads on
  the auth-check fast path) plus per-category `ArcSwap` handles
  threaded through the signal callback and live-playback
  middleware. The reload pipeline is `async` so it can call
  `JwksAuthProvider::new` / `WebhookAuthProvider::new`
  mid-process and atomically swap the resulting provider into
  the chain; old provider's `Drop` aborts its spawned refresh /
  fetcher task. Build failure (malformed TOML, JWT init reject,
  JWKS initial fetch failure) leaves all prior state intact (no
  partial swap). Stream-key store handle (146) preserved across
  reloads. The route's wire shape (`ConfigReloadStatus`) carries
  `applied_keys` (categories that effectively changed against
  prior snapshot) and `warnings` (e.g. file naming a feature-
  gated URL with that feature disabled at build). SDK clients
  gain `LvqrAdminClient.configReload()` / `triggerConfigReload()`
  in TS and `config_reload_status()` / `trigger_config_reload()`
  in Python. See
  [`docs/config-reload.md`](docs/config-reload.md).

* **Runtime stream-key CRUD admin API** (session 146). New routes
  `GET /api/v1/streamkeys`, `POST /api/v1/streamkeys`,
  `DELETE /api/v1/streamkeys/{id}`, and
  `POST /api/v1/streamkeys/{id}/rotate` let admin clients mint, list,
  revoke, and rotate ingest stream keys at runtime. Backed by a new
  `lvqr_auth::MultiKeyAuthProvider` that wraps the existing auth
  chain (Noop / Static / Jwt / Jwks / Webhook) additively: store-first
  on Publish; Subscribe + Admin always delegate to the wrapped
  provider so a misconfigured store cannot lock the operator out of
  their own admin API. Tokens are
  `lvqr_sk_<43-char base64url-no-pad>` (32 bytes OsRng + typed prefix
  per industry convention -- Stripe `sk_live_`, GitHub `ghp_`, AWS
  IVS `sk_<region>_`). In-memory only in v1; restart loses every
  minted key (operators needing durable single-key publish auth keep
  using `LVQR_PUBLISH_KEY` which becomes the wrapped fallback). New
  `--no-streamkeys` (env `LVQR_NO_STREAMKEYS`) flag opts out for
  pre-146 behavior verbatim. Counter
  `lvqr_streamkeys_changed_total{op="mint"|"revoke"|"rotate"}`
  increments once per successful API call. SDK clients
  (`@lvqr/core` and `lvqr` python package on `main`) gain matching
  `StreamKey` / `StreamKeySpec` types + four methods each. Default
  on. See [`docs/auth.md#stream-key-crud-admin-api`](docs/auth.md#stream-key-crud-admin-api).

## [0.4.1] - 2026-04-24

Workspace republish so the source on `origin/main` becomes
reachable from `cargo install`. Sessions 83 through 144 landed
between the 0.4.0 release (2026-04-16) and today but never
reached crates.io; this release closes that gap. See
`tracking/HANDOFF.md` for the session-by-session narrative.

The 0.4.0 -> 0.4.1 commit itself is a workspace version bump
with zero source changes; the published artifact carries the
full `origin/main` tree at the time of publish. The release
notes below for the 45-82 window are accurate as written; the
post-82 narrative through 144 lives only in HANDOFF.md and may
be folded back into this changelog in a future docs sweep.

## Unreleased-pre-0.4.1 (post-0.4.0, through session 82 -- 2026-04-17)

Sessions 45 through 82 expanded the protocol surface well
beyond the 0.4.0 release cut, then added a cluster plane and
the first two observability-plane sessions. Net result: 25
crates, 711 workspace tests, and the single-binary
`lvqr serve` now covers every protocol in the v1 scope plus
multi-node operation and OTLP telemetry.

### Added

- **RTSP/1.0 server.** `lvqr-rtsp` accepts ANNOUNCE / SETUP /
  RECORD / TEARDOWN over TCP with interleaved RTP; depacketized
  H.264 / HEVC flow through the unified `Fragment` stream to
  every existing egress. Enabled via `--rtsp-port` (env
  `LVQR_RTSP_PORT`). 44 unit tests plus a full
  `rtsp_hls_e2e` integration test. Session-80 audit fixed the
  `rtsp_play_emits_rtcp_sender_report_after_interval` flake
  (root-caused to `start_paused` + tokio auto-advance firing
  timeouts inside the shared read helper); the test is now
  deterministic at ~6 s runtime.

- **SRT ingest.** `lvqr-srt` accepts SRT-over-UDP MPEG-TS
  streams from broadcast encoders (OBS, vMix, Larix, ffmpeg),
  demuxes them, and feeds the unified fragment pipeline.
  Enabled via `--srt-port` (env `LVQR_SRT_PORT`).

- **Cluster plane (chitchat).** `lvqr-cluster` gives `lvqr
  serve --cluster-listen=... --cluster-seeds=...` a two-node
  cluster out of the box.
    - Membership + failure detection via chitchat (session 72).
    - Broadcast ownership KV with lease renewal and release on
      broadcaster close (session 73).
    - Per-node capacity advertisement -- CPU %, memory RSS,
      outbound bandwidth utilization (session 74).
    - Cluster-wide config with last-write-wins semantics and
      read-only `/api/v1/cluster/{nodes,broadcasts,config}`
      admin routes (session 75).
    - Per-node endpoints KV + HLS redirect-to-owner (session
      76-77). A subscriber hitting a non-owner receives a 302
      to the owner's advertised base URL.
    - DASH + RTSP redirect-to-owner (session 78).
    - Ingest auto-claim on first broadcast -- publishers no
      longer need a manual `claim_broadcast` call; the CLI
      wires a callback on the
      `FragmentBroadcasterRegistry::on_entry_created` hook
      that auto-claims every new broadcast for the life of its
      broadcaster (session 79).
    - Configurable via `--cluster-listen`, `--cluster-seeds`,
      `--cluster-node-id`, `--cluster-id`, and
      `--cluster-advertise-{hls,dash,rtsp}`.

- **Observability plane (OTLP + Prometheus fanout).**
  `lvqr-observability` gates every OTLP surface behind
  `LVQR_OTLP_ENDPOINT`.
    - Session G (80): scaffold crate, `ObservabilityConfig::
      from_env` parsing five env vars, stdout fmt subscriber.
    - Session H (81): OTLP gRPC span export.
      `tracing_opentelemetry` layer composed with the fmt
      layer through `tracing_subscriber::registry()`;
      `Sampler::TraceIdRatioBased` honours
      `LVQR_TRACE_SAMPLE_RATIO`; `BatchSpanProcessor` flushes
      and shuts down on `ObservabilityHandle::drop`.
    - Session I (82): OTLP gRPC metric export + a
      `metrics::Recorder` bridge (`OtelMetricsRecorder`) that
      forwards every existing `metrics::counter!` /
      `gauge!` / `histogram!` call site to an OTel
      `SdkMeterProvider`. `lvqr-cli::start` composes the
      bridge with the Prometheus scrape recorder via
      `metrics_util::layers::FanoutBuilder` when both paths
      are enabled.
    - Resource attribution via `service.name` (from
      `LVQR_SERVICE_NAME`) plus arbitrary `k=v` pairs from
      `LVQR_OTLP_RESOURCE`.

- **LL-HLS always-on in the zero-config default.**
  `--hls-port` default is now `8888`; a fresh
  `lvqr serve` exposes `/hls/{broadcast}/playlist.m3u8`
  without any extra flags.

- **Workspace-level deps pinned.** `opentelemetry = "0.27"`,
  `opentelemetry_sdk = "0.27"` (`rt-tokio` + `trace` +
  `metrics`), `opentelemetry-otlp = "0.27"` (`grpc-tonic` +
  `trace` + `metrics`), `tracing-opentelemetry = "0.28"`,
  `metrics-util = "0.19"`.

- **5-artifact test contract enforcement.** Every crate under
  `crates/lvqr-{ingest,whip,whep,hls,dash,srt,rtsp,codec,cmaf,
  archive,moq,fragment,record}` now ships proptest + fuzz +
  integration + E2E + conformance (some conformance slots are
  still soft-skips until external validators are in CI).
  `scripts/check_test_contract.sh` drives
  `.github/workflows/contract.yml`.

- **Criterion benches.** 15 benches across `lvqr-rtsp` (session
  68), `lvqr-cmaf` (session 69), and `lvqr-hls`
  (`PlaylistBuilder`).

### Changed

- **`lvqr-cli::start` recorder install.** The Prometheus
  recorder install path is now a four-arm match over
  `(install_prometheus, otel_metrics_recorder)`. Both set →
  `FanoutBuilder`; Prom only → legacy install; OTel only →
  `set_global_recorder(otel)`; neither → no-op. The
  Prometheus scrape handle is always captured before the
  recorder is handed to the fanout, so `/metrics` works in
  every permutation.

- **`lvqr-cli::main` lifetime.** The observability handle is
  held for the full `main` scope so the OTLP background
  flushers get a clean force_flush + shutdown on process
  exit. `take_metrics_recorder()` runs once, immediately
  after `init`, and threads the recorder through
  `ServeConfig`.

### Removed

- **`lvqr-wasm` deleted.** Browser clients should use
  `@lvqr/core` (MoQ client + admin client) and
  `@lvqr/player` (`<lvqr-player>` web component) instead.

### Fixed

- **`rtsp_play_emits_rtcp_sender_report_after_interval` flake.**
  Session-80 audit removed `start_paused=true` + auto-advance
  from the test; uses a real-time `sleep(6s)` past the
  default SR interval. Deterministic 5/5 green.

- **Honest test count.** The session-30 README claimed "84 test
  binaries, 379 tests" under the default feature set. Tier
  1 + Tier 2 progress replaced roughly a third of the Tier-0
  theatrical tests with real integration tests (publisher
  RTMP, subscriber HLS, end-to-end ffprobe validation) and
  added the 5-artifact contract harness; current count is
  711 / 0 failed / 1 ignored across the workspace.

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
