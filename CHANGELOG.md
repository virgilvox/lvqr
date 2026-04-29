# Changelog

All notable changes to LVQR are documented in this file. The
head of `main` is always the source of truth; this file
summarises user-visible surface changes between tagged
releases. For session-by-session engineering notes, see
`tracking/HANDOFF.md`.

## [1.0.0] - 2026-04-28

Stability commitment for the v0.4 surface. The 0.4.2 wave (sessions
156-162 + the 2026-04-28 audit cycle) closed the last open v1.1 plan
row (Phase A v1.1 #5, the pure-MoQ glass-to-glass SLO close-out via
the sidecar `0.timing` track), shipped four hardware encoder backends
instead of the planned one (VideoToolbox + NVENC + VA-API + QSV),
landed adversarial test coverage on every security-critical surface
(JWT expiration / wrong-secret / tampered-payload, JWKS expired /
wrong-keypair, stream-key TTL real-time expiry, SCTE-35 mutated-byte
proptest, C2PA tamper-detection round-trip, cross-agent panic
isolation), restructured CI to a Linux-required / macOS-informational
gate, and rewrote the README around the operator-facing capability
shape rather than competitive positioning. v1.0.0 publishes the same
artefact as v0.4.2 with a stability-commitment version label.

### Added

* **`@lvqr/admin-ui` 1.0.0** -- new sister npm package shipping the
  operator admin console. Vue 3 + Vite + TypeScript + Pinia + Vue
  Router; static SPA build; design tokens lifted from
  `mockups/tallyboard-storybook.html`; mobile-first responsive
  layout; multi-relay connection profiles via localStorage; plugin
  plumbing via `window.__LVQR_ADMIN_PLUGINS__`. Wires every shipped
  `/api/v1/*` route through `@lvqr/core 1.0.0`'s `LvqrAdminClient`;
  surfaces a clear placeholder + v1.x backlog hint for views the
  server does not currently expose (recordings list, transcode-edit,
  agent-edit, log tail, full config GET/PUT, WASM chain edit). See
  `bindings/js/packages/admin-ui/README.md` for deployment recipes
  (local dev, Digital Ocean App Platform, nginx static-host,
  multi-relay).

### Changed

* **Workspace renamed 0.4.2 -> 1.0.0.** No code change beyond the
  version label; every Rust crate published to crates.io as 1.0.0
  carries the same source as the 0.4.2 tag. v1.0.0 is a stability
  commitment, not a refactor.
* **JS SDK family renamed to 1.0.0.** `@lvqr/core` 0.3.3 -> 1.0.0,
  `@lvqr/dvr-player` 0.3.3 -> 1.0.0, `@lvqr/player` 0.3.2 -> 1.0.0
  with its `@lvqr/core` dep pinned exact at `1.0.0` (matches the
  pre-existing exact-pin pattern that prevents semver drift on the
  player surface).
* **Python `lvqr` renamed to 1.0.0.** No API change vs. 0.3.3.

## [0.4.2] - 2026-04-28

### Added

* **WHIP-ingest timing-track wiring** (session 161 follow-up).
  Mirrors session 159's RTMP-bridge wiring on `lvqr-whip`'s
  `WhipMoqBridge`: `BroadcastState` grows a
  `timing_sink: Option<MoqTimingTrackSink>` field, the broadcast
  initializer creates a sibling `<broadcast>/0.timing` track at
  the same lifecycle point as the existing `0.mp4` video track,
  and the keyframe dispatch in `push_sample` pushes a 16-byte LE
  `(group_id, ingest_time_ms)` anchor on every keyframe gated on
  `frag.ingest_time_ms != 0`. WHIP-ingested broadcasts now
  contribute to the pure-MoQ `lvqr_subscriber_glass_to_glass_ms`
  histogram exactly like RTMP-ingested broadcasts. SRT / RTSP /
  WS-fMP4 ingest bridges remain on the existing shape; mechanical
  mirror pending an operator ask. No public API change.

* **lvqr-srt test density brought to peer level** (session 160,
  audit recommendation #3). Three new proptest harnesses (256
  cases each) over `split_annex_b` / `annex_b_to_avcc` /
  `annex_b_to_hvcc` proving panic-freedom + length-prefix
  integrity on adversarial input. Three positive unit tests
  mirroring the existing h264 sibling at `:677`:
  `hevc_pes_publishes_init_and_keyframe_fragment_on_registry`,
  `aac_adts_publishes_init_and_audio_fragment_on_registry`,
  `scte35_section_with_valid_crc_publishes_event_on_registry`.
  One negative test (`scte35_section_with_invalid_crc_drops`)
  flips the trailing CRC byte and asserts no fragment publishes
  within 50 ms via `tokio::time::timeout`. Workspace lib tests
  go from 849 to 856; lvqr-srt's own count from 4 to 11.

* **Pure-MoQ glass-to-glass SLO sample pusher** (session 159,
  Phase A v1.1 #5 close-out). Sibling `<broadcast>/0.timing` MoQ
  track stamps one 16-byte LE
  `(group_id_u64_le || ingest_time_ms_u64_le)` anchor per video
  keyframe (additive; foreign MoQ clients ignore the unknown
  track name per the moq-lite contract). New
  `lvqr_fragment::MoqTimingTrackSink` + `TimingAnchor` value
  type with `encode` / `decode` round-trip helpers + new
  `TIMING_TRACK_NAME = "0.timing"` + `TIMING_ANCHOR_SIZE = 16`
  constants. `MoqTrackSink::push` return type widened from
  `Result<(), MoqSinkError>` to `Result<Option<u64>, MoqSinkError>`
  so the producer-side bridge knows the wire-side group sequence
  to encode (backward-compatible at every existing callsite).
  RTMP ingest bridge wires the timing track at broadcast-start
  and pushes one anchor per video keyframe. New
  `[[bin]] lvqr-moq-sample-pusher` on `lvqr-test-utils`
  subscribes to both `0.mp4` + `0.timing`, joins anchors against
  video frames by `group_id` in a 64-entry ring buffer
  (exact-match + largest-less-than fallback + skip-on-miss),
  throttles pushes to a configurable `--push-interval-secs`,
  and POSTs JSON samples to the existing dual-auth
  `POST /api/v1/slo/client-sample` route. New default-feature
  integration test (`crates/lvqr-test-utils/tests/moq_timing_e2e.rs`)
  drives the full RTMP -> relay -> bin -> SLO endpoint loop and
  asserts a non-empty `transport="moq"` entry on
  `GET /api/v1/slo`. **Phase A v1.1 #5 closed; the last open
  v1.1 roadmap row.** See
  [`tracking/SESSION_159_BRIEFING.md`](tracking/SESSION_159_BRIEFING.md).

* **`POST /api/v1/slo/client-sample` admin endpoint** (session
  156 follow-up). Accepts JSON
  `{broadcast, transport, ingest_ts_ms, render_ts_ms}` from any
  subscriber under dual-auth (admin OR per-broadcast subscribe
  token). Validates non-empty fields, render >= ingest, and
  latency <= 5 min clock-skew cap. Records into the existing
  `LatencyTracker` powering `GET /api/v1/slo` + the
  `lvqr_subscriber_glass_to_glass_ms` Prometheus histogram. New
  `lvqr_slo_client_samples_total{transport}` counter for
  sample-rate visibility.

* **`@lvqr/dvr-player` SLO sampler** (session 156 follow-up).
  Three new opt-in attributes (`slo-sampling="enabled"`,
  `slo-endpoint="<URL>"`, `slo-sample-interval-secs`) drive a
  client-side timer that lifts the publisher wall-clock via
  `HTMLMediaElement.getStartDate() + currentTime`, computes
  `latency_ms = Date.now() - that`, and POSTs to the new admin
  route. Best-effort: any failure is silently dropped to keep
  playback uninterrupted. Pure helpers in
  `bindings/js/packages/dvr-player/src/slo-sampler.ts`
  (`computeLatencyMs`, `broadcastFromHlsSrc`, `pushSample`)
  covered by 16 Vitest unit tests.

* **VideoToolbox hardware encoder backend on macOS** (session
  156). New `lvqr_transcode::VideoToolboxTranscoderFactory`
  behind a per-encoder `hw-videotoolbox` Cargo feature on
  `lvqr-transcode` + `lvqr-cli`. Mirrors
  `SoftwareTranscoderFactory` (105 B / 106 C) verbatim except
  for the encoder element (`vtenc_h264_hw` instead of
  `x264enc`) and its property mapping (`realtime=true` /
  `allow-frame-reordering=false` / `max-keyframe-interval=60`).
  HW-only path is intentional: the factory's `is_available()`
  probe at construction returns false when `vtenc_h264_hw` is
  missing and `build()` opts out of every stream with a warn
  log so a HW-pickable tier never silently falls back to CPU.
  CLI flag `--transcode-encoder software|videotoolbox`
  (default `software`); the `videotoolbox` value is rejected
  at parse time on builds without the feature. NVENC, VAAPI,
  QSV stay deferred to v1.2.

* **`videotoolbox-macos.yml` CI lane** (session 156 follow-up).
  Runs the new HW-encoder integration test on `macos-latest`
  for every PR touching the transcode crate, with
  Homebrew-installed GStreamer + plugins.
  `continue-on-error: true` initially while CI variance on the
  GitHub-hosted Apple Silicon runner stabilises.

* **lvqr-srt SCTE-35 PMT 0x86 reassembly + RTMP onCuePoint**
  (session 152). PMT stream_type 0x86 with private-section
  reassembly across TS packet boundaries; RTMP onCuePoint
  scte35-bin64 via the vendored `rml_rtmp` v0.8 fork at
  `vendor/rml_rtmp/`. Splice events flow through a reserved
  `"scte35"` parallel track on the existing
  `FragmentBroadcasterRegistry` and render as
  `#EXT-X-DATERANGE` on LL-HLS + Period-level `<EventStream>`
  on DASH. Splice_info_section bytes pass through verbatim
  with CRC verification but no semantic interpretation. New
  `lvqr-codec/src/scte35.rs` parser per ANSI/SCTE 35-2024
  section 8.1 with proptest harness + libfuzzer target. New
  metrics `lvqr_scte35_events_total{ingest, command}` +
  `lvqr_scte35_drops_total{ingest, reason}`.

### Changed

* **Workspace `Cargo.toml` version 0.4.1 -> 0.4.2.** All 26
  publishable Rust crates bump together. Run
  `cargo publish -p <crate>` per the tier order in CLAUDE.md
  (`lvqr-core` first, `lvqr-cli` last) to push to crates.io.

* **`crates/lvqr-fragment::MoqTrackSink::push` return type
  widens** from `Result<(), MoqSinkError>` to
  `Result<Option<u64>, MoqSinkError>` so callers can read the
  wire-side group sequence the call just opened (`Some(seq)` on
  the keyframe path; `None` on the delta-frame / pre-keyframe
  drop path). Backward-compatible at every existing in-tree
  callsite (each used `.expect()` discarding the value or
  `if let Err(...)`).

### Documentation

* **Codebase + roadmap audit** at
  `tracking/CODEBASE_AUDIT_2026_04_27.md` (session 158).
  12-section cite-by-line audit covering workspace shape,
  per-crate review, public API drift, test coverage, TODO
  markers, CI workflows, SDK packages, doc drift,
  roadmap-vs-implementation matrix, tech debt, and ranked next
  3-5 sessions. Recommendations 1-3 (DOC-DRIFT-A,
  PATH-X-MOQ-TIMING, SRT-TEST-GAP) all closed in subsequent
  sessions.

* **DOC-DRIFT-A doc sweep** (session 158 follow-up).
  `docs/architecture.md` + `docs/quickstart.md` flip 27-crate ->
  29-crate (architecture doc gains `lvqr-agent-whisper` +
  `lvqr-transcode`); seven Rust crate `lib.rs` doc-comments
  rewritten to drop scaffold-session framing
  (`lvqr-mesh`, `lvqr-whep`, `lvqr-hls`, `lvqr-cmaf`,
  `lvqr-transcode`); four crates that previously had zero
  module-level docstrings (`lvqr-relay`, `lvqr-rtsp`,
  `lvqr-admin`, `lvqr-signal`) gain one. Dead `@lvqr/core/wasm`
  SDK subpath dropped from `bindings/js/packages/core/package.json`
  (the pre-built artefacts under `wasm/` were built against
  the pre-0.4-session-44 browser-side `lvqr-wasm` crate that
  no longer exists).

* **`tracking/SESSION_159_BRIEFING.md`** (session 159 step 0).
  ~590 lines locking eight engineering decisions for the
  PATH-X-MOQ-TIMING close-out: sibling-track wire shape,
  producer wiring location, return-type widening on
  `MoqTrackSink::push`, subscriber-side bin shape, group-id
  matching strategy, anchor ring-buffer capacity, test scope,
  anti-scope.

* **`docs/slo.md` operator runbook refresh** (session 157
  follow-up). Documents the shipped
  `POST /api/v1/slo/client-sample` route, the `@lvqr/dvr-player`
  sampler as the reference HLS-side client, the new
  `lvqr_slo_client_samples_total{transport}` counter, the
  transport-specific recovery (HLS via PDT, MoQ via the
  session-159 sidecar track), and the
  Path X v1.2 sidecar-track plan with a forward-link.

* **`tracking/SESSION_157_BRIEFING.md`** (session 157, MoQ
  SLO audit). The audit confirmed the MoQ wire (then) carried
  no per-frame wall-clock anchor; the brief locked the Path Y
  / X / Z scoping decision (Y chosen at the time: document the
  gap; X became session 159's actual close-out).

### Removed

* Browser-side `LvqrSubscriber` artefacts at
  `bindings/js/packages/core/wasm/` (gitignored, never committed
  -- session 158 follow-up local cleanup) and the dead
  `./wasm` export + `build:wasm` script from `@lvqr/core`'s
  `package.json`. The pre-deletion artefacts dated to before
  the 0.4-session-44 refactor; the current `crates/lvqr-wasm`
  is the server-side wasmtime filter host with no
  `wasm-bindgen` surface.

## [0.4.1] - 2026-04-24

(See sessions 145 republish-only commit; no shape changes vs
0.4.0 surface.)

## Pre-0.4.2 unreleased entries (rolled into 0.4.2 above)

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
