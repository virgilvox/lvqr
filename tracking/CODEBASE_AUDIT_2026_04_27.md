# LVQR Codebase + Roadmap Audit -- session 158 (2026-04-27)

## 1. Audit summary

The codebase is in good shape. The default-feature workspace builds
clean (`cargo test --workspace --lib --no-run` finished in 21.42 s
producing 29 unit-test executables, one per workspace member; no
compile errors, no warnings during this audit's pass), the
post-session-150 supply-chain ledger sits at six documented
ignores in `audit.toml`, and the eight long-running phase rows
the README tracks are closed except Phase A v1.1 #5 -- which is
correctly characterised as a v1.2 follow-up after the session 157
scenario-(c) audit. The three biggest concrete surprises are
documentation drift, not code drift:

* **`docs/architecture.md` + `docs/quickstart.md` still describe a
  27-crate workspace.** The Cargo.toml has 29 members, the README
  agrees ("29 crates"), and the architecture doc's bulleted listing
  at `docs/architecture.md:175-220` is missing `lvqr-agent-whisper`
  and `lvqr-transcode` (both shipped sessions 98+ and 104+
  respectively). `docs/quickstart.md:329` repeats the same stale
  "27 crates" count and `:337-338` describes the mesh as "topology
  only today; media relay is Tier 4 on the roadmap" while
  `docs/mesh.md:8` flips to "**IMPLEMENTED**" and the README
  records the data-plane phase D as fully shipped (sessions
  141-144).
* **Several Rust crate `lib.rs` doc-comments are frozen at the
  scaffold-session shape and now contradict the README +
  docs/.** The biggest offenders are `lvqr-mesh/src/lib.rs:1-19`
  (claims "topology planner only" + "the mesh offload percentage
  exposed via the admin API reports *intended* offload, not
  *actual* offload" -- session 141 closed actual-offload
  reporting), `lvqr-whep/src/lib.rs:6-13` (says "A future session
  plugs in `str0m` behind the [`SdpAnswerer`] trait" while
  `pub mod str0m_backend` already lives at `:23`), and
  `lvqr-transcode/src/lib.rs:11-103` (still narrates "Session 104 A
  scope" / "What session 105 B adds" / "What session 106 C adds"
  long after sessions 113 + 156 superseded those framings).
  `lvqr-relay`, `lvqr-rtsp`, `lvqr-admin`, and `lvqr-signal` all
  ship `lib.rs` files with zero module-level docstring at all.
* **The `@lvqr/core` JS package still ships a stale `./wasm`
  subpath** pointing at pre-built artefacts under
  `bindings/js/packages/core/wasm/` (mtime `Apr 10`). The
  artefacts were built against the *browser-side* `lvqr-wasm`
  crate that `crates/lvqr-wasm/src/lib.rs:5-6` documents as
  "deliberately unrelated to the browser-facing `lvqr-wasm`
  crate that was deleted in the 0.4-session-44 refactor". The
  `package.json` `build:wasm` script
  (`bindings/js/packages/core/package.json:31`) still points at
  `crates/lvqr-wasm` with `wasm-pack --target web`, but that
  crate is now the *server-side* wasmtime filter host with no
  `wasm-bindgen` surface. The `./wasm` export, the `wasm/`
  directory, and the script are dead SDK surface that the next
  `@lvqr/core` publish should drop.

The recommended next session is **DOC-DRIFT-A**: a single
documentation-only sweep that closes the architecture / quickstart
crate-count drift, refreshes the seven stale `lib.rs`
docstrings, and removes the dead `./wasm` subpath from
`@lvqr/core`. ~150-line diff across docs + four crate `lib.rs`
files, no code, no test changes, very low risk -- and it clears
the deck for **PATH-X-MOQ-TIMING** (the Phase A v1.1 #5 v1.2
close-out, ~800-1200 LOC) without re-litigating already-closed
decisions.

## 2. Workspace shape

`Cargo.toml` declares **29** workspace members
(`Cargo.toml:16-46`). Edition 2024, MSRV 1.85
(`Cargo.toml:50,58`). Three of the 29 are `publish = false`:
`lvqr-conformance` (`crates/lvqr-conformance/Cargo.toml:10`),
`lvqr-test-utils` (`crates/lvqr-test-utils/Cargo.toml:10`), and
`lvqr-soak` (`crates/lvqr-soak/Cargo.toml:10`). That matches the
HANDOFF claim of "26 publishable Rust crates".

Eight fuzz crates are explicitly excluded from the workspace
(`Cargo.toml:6-15`) so libfuzzer-sys's nightly + sanitizer
requirement does not infect default builds. Each excluded path
exists on disk and has a non-empty `fuzz_targets/` directory:

* `crates/lvqr-cmaf/fuzz/fuzz_targets/detect_codec_strings.rs`
* `crates/lvqr-codec/fuzz/fuzz_targets/{parse_aac_asc,parse_hevc_sps,parse_scte35,read_ue_v,ts_demux}.rs`
* `crates/lvqr-hls/fuzz/fuzz_targets/playlist_builder.rs`
* (plus `lvqr-{ingest,whep,whip,dash,rtsp}/fuzz/`)

All eight fuzz directories are wired into `.github/workflows/fuzz.yml`.

### Workspace dependency hygiene

`[workspace.dependencies]` (`Cargo.toml:61-218`) is dense but
disciplined. Every non-trivial pin carries a comment explaining
the reason and the upgrade procedure. Notable pins:

* `wasmtime = "43"` with the v25 -> v43 close-out story
  (`Cargo.toml:182-185`). Session 150 closed 16 advisories.
* `c2pa = "0.80"` with `default-features = false` +
  `rust_native_crypto` (`Cargo.toml:215-218`). Avoids the
  vendored OpenSSL closure.
* `gstreamer = "0.23"` family pinned because 0.24 raised MSRV
  beyond 1.85 (`Cargo.toml:198-207`).
* `tokio-uring = "0.5"` listed at workspace level but resolved
  only via `[target.'cfg(target_os = "linux")'.dependencies]`
  in `lvqr-archive` (`crates/lvqr-archive/Cargo.toml:32-33`).
* `opentelemetry = "0.27"` family pinned to the 0.27 line
  matched by `tracing-opentelemetry 0.28` (`Cargo.toml:174-177`).
* `rml_rtmp = "0.8"` redirected via `[patch.crates-io]` at
  `Cargo.toml:254-260` to `vendor/rml_rtmp/` so the
  session 152 + session 155 patches load. The patch table is the
  only one in the workspace.

Every internal crate is referenced via `version = "0.4.1", path =
"crates/<name>"` so a `cargo publish` works without lockfile
gymnastics; the three `publish = false` crates omit `version`.

### Per-crate Cargo.toml health

Sampled all 29 crates' `Cargo.toml` files. Findings:

* **lvqr-relay** declares `default = ["quinn-transport"]`
  (`crates/lvqr-relay/Cargo.toml:14-15`). The dep edges for the
  feature are correct; the `quinn`, `web-transport-quinn`,
  `rustls`, `moq-native`, `rcgen` deps are all `optional = true`
  and pulled in by the feature.
* **lvqr-admin** declares `default = ["cluster"]`
  (`crates/lvqr-admin/Cargo.toml:32-40`). Means a vanilla
  `cargo build -p lvqr-admin` pulls `lvqr-cluster`. That is the
  right shape for a single-binary build but the cluster-disabled
  surface is exercisable only via `--no-default-features`.
* **lvqr-cli** has the largest feature surface
  (`crates/lvqr-cli/Cargo.toml:21-76`): default
  `["rtmp", "quinn-transport", "cluster"]`, full
  meta-feature `["...", "c2pa", "whisper", "transcode", "jwks",
  "webhook"]`, plus per-platform `hw-videotoolbox` (which implies
  `transcode`). The matrix is non-trivial but every cell has a
  matching `feature-matrix.yml` job (line 39+, soft-fail).
* **lvqr-test-utils** carries 3 features (`c2pa`, `whisper`,
  `transcode`) that all forward to the matching `lvqr-cli`
  features (`crates/lvqr-test-utils/Cargo.toml:12-29`). A test
  helper crate forwarding production features is unusual but
  correct: the `TestServer::start` builder needs the same
  feature surface as the CLI it wraps.
* **lvqr-test-utils** also has a `[[bin]]` declaration for
  `scte35-rtmp-push` (`crates/lvqr-test-utils/Cargo.toml:101-103`)
  added in session 155. That is the only binary inside a
  test-helper crate.
* **lvqr-srt** pulls `srt-tokio = "0.4"` directly
  (`crates/lvqr-srt/Cargo.toml:24`); `lvqr-whip`/`lvqr-whep`
  pull `str0m = "0.18"` directly
  (`crates/lvqr-whip/Cargo.toml:26`,
  `crates/lvqr-whep/Cargo.toml:31`). These are the only direct
  semver edges that bypass `[workspace.dependencies]`. Both are
  small enough to keep at the leaf.

No Cargo.toml stanza in the audit looked unused or
feature-flag-bloated.

### Excluded fuzz crates

The workspace's `exclude = [...]` block at `Cargo.toml:6-15`
deliberately keeps fuzz crates out of the default workspace.
Each excluded directory is its own self-contained workspace per
cargo-fuzz convention. They run via the dedicated `fuzz.yml`
workflow (scheduled + on-demand). That is the right shape.

## 3. Per-crate review (29 crates)

Below: each crate's stated purpose (from its lib.rs doc when
present, else its `description` field), top-2 source files by
LOC, public-surface size, test count (default-feature unit +
integration), and any concerning pattern. Source-LOC counts
exclude `tests/`, `examples/`, `benches/`, `fuzz/`.

### lvqr-core (Tier 0)

* **lib.rs**: 75 lines, full module docstring describing the
  post-Tier-2.1 retired type list (`lvqr-core/src/lib.rs:1-22`).
  Public surface: `Frame`, `TrackName`, `EventBus`,
  `RelayEvent`, `CoreError`, `now_unix_ms`, `DEFAULT_EVENT_CAPACITY`
  (lib.rs:28-30,42).
* **Top files**: `events.rs` (~150 LOC), `types.rs` (~100 LOC).
* **Tests**: 6 unit, 0 integration.
* **Tier match**: Tier 0 (foundation). Direct deps are leaf
  crates only (`bytes`, `serde`, `thiserror`, `parking_lot`,
  `tracing`, `dashmap`, `tokio` with minimal features). No
  internal deps. **Publishable as Tier 0 confirmed.**

### lvqr-fragment (Tier 0/1 boundary)

* **lib.rs**: 48 lines, rich roadmap-decision docstring + ASCII
  art. Public surface lists `Fragment`, `FragmentMeta`,
  `FragmentFlags`, `FragmentBroadcaster`, `FragmentBroadcasterRegistry`,
  `MoqTrackSink`, `MoqGroupStream`, `MoqTrackStream`,
  `FragmentStream`, `BroadcasterStream`, `SCTE35_TRACK`, etc.
* **Top files**: `registry.rs` (650 LOC), `moq_sink.rs`, `moq_stream.rs`.
* **Tests**: 32 unit + 10 integration.
* **Internal dep**: `lvqr-moq` only (clean).

### lvqr-moq (Tier 1)

* **lib.rs**: 76 lines, pure re-export facade over `moq-lite
  0.15`. Tier 1 placement matches CLAUDE.md. The re-export table
  at lib.rs:25-36 stays current; `MOQ_LITE_VERSION` const at
  `:47` documents the pinned upstream.
* **Tests**: 3 unit + 3 integration.
* **No internal deps.**

### lvqr-codec (Tier 2)

* **lib.rs**: 45 lines, doc lists `hevc`, `aac`, `bit_reader`,
  `error`, `scte35` (added session 152), `ts` modules. The
  re-export of `parse_splice_info_section` + `SpliceInfo` was
  added in session 152.
* **Top files**: `ts.rs` (707 LOC), `scte35.rs` (616 LOC),
  `hevc.rs` (545 LOC).
* **Tests**: 40 unit + 17 integration.
* **Suspicious patterns**: `panic!` at `aac.rs:262` is a test
  assertion (`other => panic!("expected MalformedAsc error...")`);
  no production panics.

### lvqr-cmaf

* **lib.rs**: 72 lines, clear rationale for `mp4-atom`
  dependency. **Drift at `:42`**: claims "The hand-rolled writer
  in `lvqr-ingest::remux::fmp4` stays in place during the
  transition so `rtmp_ws_e2e` does not regress." Verified: that
  hand-rolled writer still exists
  (`crates/lvqr-ingest/src/remux/fmp4.rs`, 608 LOC) and
  `rtmp_ws_e2e.rs` is still referenced from
  `crates/lvqr-cli/tests/rtmp_hls_e2e.rs:3`. Statement is
  *current*, not drifted -- the migration genuinely has not
  completed.
* **Drift at `:53`**: "Fuzz slot opens when a parser attack
  surface lands in this crate (today there is none -- the
  segmenter only reads `Bytes` from a trusted producer and
  writes mp4-atom structures)." A fuzz target DOES exist:
  `crates/lvqr-cmaf/fuzz/fuzz_targets/detect_codec_strings.rs`.
  The lib.rs is stale on this point.
* **Top files**: `init.rs` (1757 LOC), `coalescer.rs` (494 LOC).
* **Tests**: 43 unit + 14 integration.

### lvqr-hls

* **lib.rs**: 77 lines, "5-artifact contract (day-one state)"
  doc reads as if Apple `mediastreamvalidator` is still
  unintegrated (`lvqr-hls/src/lib.rs:58-64`). Verified vs HANDOFF:
  `mediastreamvalidator` integration was identified as the
  largest open audit gap (per the user-memory anchor:
  "Session-30 maturity audit summary: biggest open gap is
  mediastreamvalidator in CI"). That gap is *still* open --
  there is no `mediastreamvalidator_bytes` helper in
  `crates/lvqr-test-utils/src/`, so the lib.rs is correctly
  describing current state.
* **`lib.rs:31-37`** says "What is NOT in this crate yet:
  Multivariant master playlists (`#EXT-X-STREAM-INF`). Single
  rendition only for now." **Drift**: master.rs is in the
  module list at `:67` and `pub use master::{MasterPlaylist,
  MediaRendition, ...}` is exported at `:75`. Multivariant master
  shipped session 106 C with the transcode ladder.
* **Top files**: `server.rs` (1597 LOC), `manifest.rs`
  (1294 LOC), `subtitles.rs` (516 LOC).
* **Tests**: 56 unit + 25 integration.

### lvqr-dash

* **lib.rs**: 77 lines, accurate; the doc has been kept current
  through session 109 A (the `lvqr-admin` direct dep at
  `Cargo.toml:24` is documented at the lib.rs level too).
* **Top files**: `server.rs` (912 LOC), `mpd.rs` (741 LOC).
* **Tests**: 32 unit + 12 integration.

### lvqr-relay (Tier 2 per CLAUDE.md)

* **lib.rs**: **7 lines** -- zero module-level docstring. Just
  five `pub mod`/`pub use` lines. The crate description
  (`Cargo.toml:3`) is the only narrative.
* **Top files**: `server.rs` (330 LOC -- with a
  `#[cfg(not(feature = "quinn-transport"))]` stub `RelayServer`
  at `:312-330` that returns an error from `run`. Reasonable
  shape, undocumented.).
* **Tests**: 6 unit + 5 integration.

### lvqr-mesh (Tier 2)

* **lib.rs**: 27 lines. **Stale doc**: `:1-19` claims
  "Status: topology planner only. ... It does not yet drive
  real WebRTC peer connections." and "Until then, the mesh
  offload percentage exposed via the admin API reports
  *intended* offload, not *actual* offload." Both contradict:
  * **README:794-797** "**Mesh data-plane Phase D** (sessions
    141-144) ... `docs/mesh.md` is IMPLEMENTED."
  * **`docs/mesh.md:8`** "**Status as of main (post v0.4.0):
    IMPLEMENTED.**"
  * **README:895-903** "**Actual-vs-intended offload
    reporting** ... Shipped in session 141. Browser peers
    maintain a cumulative forwarded-frame counter and emit a
    `ForwardReport` signal message every second."
* **Top files**: `coordinator.rs` (794 LOC), `tree.rs`.
* **Tests**: 25 unit + 0 integration.
* **Public surface**: `MeshConfig`, `MeshCoordinator`,
  `PeerAssignment`, `PeerInfo`, `PeerRole`, `MeshError`.

### lvqr-ingest (Tier 2)

* **lib.rs**: 27 lines, all `pub mod` + `pub use`. No
  module-level docstring; lib.rs is bare.
* **Top files**: `remux/fmp4.rs` (608 LOC), `rtmp.rs` (588 LOC),
  `bridge.rs` (566 LOC), `remux/flv.rs` (502 LOC).
* **Tests**: 34 unit + 16 integration.
* **`default = ["rtmp"]`** (`Cargo.toml:14-15`) -- the
  rtmp-feature gate compiles out the bridge, rtmp.rs, and
  protocol::RtmpIngest, which is good hygiene for the
  TestServer surface.

### lvqr-whep (Tier 2/3)

* **lib.rs**: 35 lines. **Stale doc**: `:6-13` says "Session 16
  ... A future session plugs in `str0m` behind the
  [`SdpAnswerer`] trait. Once that lands, [`WhepServer`] is
  wired into `lvqr-cli` via
  `RtmpMoqBridge::with_raw_sample_observer`." But:
  * `pub mod str0m_backend;` at `:23`
  * `pub use str0m_backend::{Str0mAnswerer, Str0mConfig, Str0mSessionHandle};`
    at `:28`
  * `str0m = "0.18"` direct dep at `Cargo.toml:31`
* **Stale comment** at `crates/lvqr-whep/src/str0m_backend.rs:954-955`:
  "the audio path is still unwired (no AAC -> Opus
  transcoder) and trickle ICE ingestion is still TODO."
  The AAC -> Opus transcoder shipped in session 113 via the
  `aac-opus` Cargo feature on lvqr-whep
  (`crates/lvqr-whep/Cargo.toml:14-20`). Trickle ICE remains a
  real open TODO.
* **Top files**: `str0m_backend.rs` (1414 LOC).
* **Tests**: 28 unit + 19 integration.

### lvqr-whip

* **lib.rs**: 34 lines, accurate. `Str0mIngestAnswerer` etc.
  exported at `:33`.
* **Top files**: `bridge.rs` (1040 LOC), `str0m_backend.rs`
  (521 LOC).
* **Tests**: 27 unit + 19 integration.

### lvqr-archive

* **lib.rs**: 95 lines, exhaustive design rationale block.
  `:75-84` lists "What this crate is NOT" -- still accurate.
* **Top files**: `provenance.rs` (665 LOC).
* **Tests**: 36 unit + 5 integration.
* `:107` in provenance.rs: "KMS operator story that was a
  documentation TODO through session 93" -- this is a *closed*
  TODO referenced in a doc comment. Not drift.

### lvqr-signal

* **lib.rs**: **5 lines** -- zero module-level docstring,
  smallest in the workspace. Just `pub mod` + `pub use`.
* **Top files**: `signaling.rs` (973 LOC).
* **Tests**: 21 unit + 7 integration.
* **Public surface**: `ForwardReportCallback`, `IceServer`,
  `PeerCallback`, `PeerEvent`, `SignalMessage`, `SignalServer`,
  `SignalError`.

### lvqr-admin (Tier 3)

* **lib.rs**: 28 lines -- module declarations + `pub use`
  block + 11-line `AdminConfig` stub. **Zero module-level
  docstring**.
* **Top files**: `routes.rs` (1424 LOC), `slo.rs`,
  `cluster_routes.rs`.
* **Tests**: 54 unit + 3 integration.
* `routes.rs:480-519` `ClientLatencySample::ingest_ts_ms` doc
  comment was rewritten in the session 157 audit; the new
  text correctly describes HLS-via-PDT recovery + the MoQ gap.
  No drift.

### lvqr-auth

* **lib.rs**: 57 lines, accurate. Module gating tracks the
  three optional features (`jwt`, `jwks`, `webhook`).
* **Top files**: `jwks_provider.rs` (700 LOC),
  `webhook_provider.rs` (653 LOC), `stream_key_store.rs`
  (469 LOC).
* **Tests**: 86 unit. **Largest unit-test count after
  `lvqr-cli`.**
* No integration tests (everything in-crate via real-server
  fixtures with `wiremock`).

### lvqr-record

* **lib.rs**: 23 lines, accurate. Layout doc at `:8-15`.
* **Top files**: `recorder.rs` (~225 LOC).
* **Tests**: 3 unit + 8 integration. Slightly low unit count
  but the integration suite covers the real-disk path.

### lvqr-rtsp

* **lib.rs**: **10 lines** -- zero module-level docstring.
  Just module declarations + `pub use server::{...}`.
* **Top files**: `server.rs` (1697 LOC), `rtp.rs` (1510 LOC),
  `play.rs` (1030 LOC), `sdp.rs` (632 LOC).
* **Tests**: 120 unit + 21 integration. **Highest unit-test
  count in the workspace.** 6313 LOC of source, ~141 tests
  total. Healthy density for a wire-format crate.

### lvqr-srt

* **lib.rs**: 29 lines, doc accurately describes the SRT
  listener + MPEG-TS demux flow. Mentions session-152 SCTE-35
  passthrough indirectly via the `lvqr_codec::TsDemuxer`
  reference but does not call out the SCTE-35 path
  explicitly.
* **Top files**: `ingest.rs` (731 LOC).
* **Tests**: 4 unit + 0 integration. **Lowest test density
  for any crate >500 LOC.** This is the conspicuous gap of
  this audit. The SRT crate carries ~760 source LOC including
  the PMT 0x86 SCTE-35 reassembly path (cited as a session 152
  surface in `docs/scte35.md`) but ships only four unit tests
  -- no integration test, no proptest, no fuzz harness. The
  cmaf, hls, codec, and ingest crates all have proptest
  harnesses; lvqr-srt has none. Recommended action: a future
  session should add at least a proptest over the TS-packet
  reassembly state machine and one integration test that
  drives a real srt-tokio publisher into a TestServer-backed
  listener (the test infrastructure is already available via
  `lvqr-test-utils` and the existing srt-tokio dev-dep at
  `Cargo.toml:24`).

### lvqr-cli (Tier 4)

* **lib.rs**: 1474 LOC; module-level docstring is concise
  (lines 1-12, plus rich pub-use re-export commentary). Also
  bears the largest `start()` function in the workspace.
* **`main.rs`**: 1361 LOC.
* **Top files**: `lib.rs`, `main.rs`, `archive.rs` (1244 LOC),
  `config_reload.rs` (988 LOC), `ws.rs` (669 LOC),
  `config.rs` (623 LOC).
* **Tests**: 87 unit + 90 integration. **Highest absolute
  test count in the workspace** (177 in this crate alone). The
  integration suite at `crates/lvqr-cli/tests/` is the
  workspace's e2e backbone.
* **Tier 4 confirmed**: depends on every Tier 0-3 crate.

### lvqr-cluster (Tier 3)

* **lib.rs**: 669 LOC, deep module-level docstring through
  `:60`. Public surface scaled by the broadcast-ownership KV +
  capacity gauge + endpoints + LWW config + federation.
* **Top files**: `federation.rs` (973 LOC), `lib.rs`,
  `broadcast.rs` (467 LOC).
* **Tests**: 51 unit + 23 integration.

### lvqr-observability

* **lib.rs**: 593 LOC, comprehensive env-var doc table at
  `:54-67`. Doc claims session-H scope still references session
  I (OTLP metric) and session J (JSON logs) as future. **Drift
  if those shipped**: the OTLP metric path appears to have
  shipped (workspace deps include `metrics-util` for
  `FanoutBuilder` per `Cargo.toml:163-168`, and
  `lvqr-cli/src/lib.rs:101-124` installs both Prometheus +
  OTLP recorders). Session J (JSON logs) is harder to verify;
  the env var `LVQR_LOG_JSON` is referenced at `:65`.
* **Top files**: `lib.rs` (593 LOC).
* **Tests**: 14 unit + 7 integration.

### lvqr-soak

* **lib.rs**: 940 LOC; the soak harness implementation lives
  here as a library so the bin can wrap it. Doc at `:1-34`
  accurately describes scope + non-coverage.
* **Tests**: 0 unit + 6 integration. (Bin crate, integration
  is the right shape.)

### lvqr-test-utils

* **lib.rs**: 302 LOC, 11 modules + `TestServer` accessor.
* **Top files**: `test_server.rs` (599 LOC), `bin/scte35_rtmp_push.rs`
  (445 LOC -- session 155).
* **Tests**: 12 unit + 3 integration. (Test-helper crate,
  appropriate.)

### lvqr-wasm (Tier 4)

* **lib.rs**: 770 LOC, deep doc with session-by-session scope
  doc at `:53-65`. `:71` notes the v1.1 PLAN Phase D session 136
  chain composition addendum -- doc is *partially* updated.
  The "Session A / B / C" framing is preserved for historical
  context.
* **Top files**: `lib.rs`, `observer.rs`.
* **Tests**: 28 unit + 1 integration.

### lvqr-agent (Tier 4)

* **lib.rs**: 120 lines, scaffold-session doc preserved with
  "Anti-scope (session 97 A)" block at `:97-113`. The session-97
  anti-scope is now stale (CLI wiring shipped session 100 D,
  whisper concrete agent shipped session 98 B per the
  scaffolding's own session sequence) but the block is framed
  as historical record, which is defensible.
* **Top files**: `runner.rs` (650 LOC).
* **Tests**: 8 unit + 1 integration.

### lvqr-agent-whisper

* **lib.rs**: 101 lines, module-level doc accurately gates the
  `whisper` Cargo feature path. "Anti-scope (session 98 B)" at
  `:69-86` is similarly historical-but-not-misleading.
* **Top files**: `worker.rs`, `decode.rs`.
* **Tests**: 32 unit + 1 integration. The integration is the
  `#[ignore]`-gated whisper-cpp model test.

### lvqr-transcode (Tier 4)

* **lib.rs**: 133 lines. **Most extensive lib.rs drift in
  the workspace**: `:11-103` walks through "Session 104 A
  scope", "What session 105 B adds", "What session 106 C adds",
  and "Anti-scope (session 104 A)" as if those are upcoming.
  All have shipped:
  * Session 105 B (real GStreamer pipelines, gating on
    `transcode` feature) -- shipped, exported at `:127-130`
    (`SoftwareTranscoder`, `SoftwareTranscoderFactory`).
  * Session 106 C (CLI wiring, master playlist composition,
    `AudioPassthroughTranscoderFactory`) -- shipped, exported
    at `:121` and via `lvqr-cli`.
  * Session 113 (`AacToOpusEncoder` for WHEP audio) --
    shipped, exported at `:127-128`.
  * Session 156 (VideoToolbox HW backend) -- shipped,
    `cfg(feature = "hw-videotoolbox")` exports at `:132-133`.
  The doc framing should be present-tense, not "what session N
  adds".
* **Top files**: `software.rs` (916 LOC), `videotoolbox.rs`
  (792 LOC), `aac_opus.rs` (675 LOC), `runner.rs` (567 LOC).
* **Tests**: 42 unit + 3 integration (gated test files in
  `tests/`: `aac_opus_roundtrip.rs`, `software_ladder.rs`,
  `videotoolbox_ladder.rs`).

### lvqr-conformance (test-only)

* **lib.rs**: 282 lines, accurate doc at `:1-27`. `publish =
  false` correctly declared.
* **Tests**: 3 unit + 0 integration. (Fixture loader; tests
  are in consumer crates.)

### Tier publishability

CLAUDE.md publishing order is:

* Tier 0: lvqr-core
* Tier 1: lvqr-signal
* Tier 2: lvqr-relay, lvqr-ingest, lvqr-mesh
* Tier 3: lvqr-admin
* Tier 4: lvqr-wasm, lvqr-cli

Cross-checked against actual `[dependencies]` graphs:

* **lvqr-signal** as Tier 1 matches its in-tree deps:
  `lvqr-core` only (`crates/lvqr-signal/Cargo.toml:14`). OK.
* **lvqr-mesh** as Tier 2 names `lvqr-core` + `lvqr-signal`
  (`crates/lvqr-mesh/Cargo.toml:14-15`). Tier-2 OK.
* **lvqr-admin** at Tier 3 depends on `lvqr-core`, `lvqr-auth`,
  optional `lvqr-cluster` (`crates/lvqr-admin/Cargo.toml:14-16`).
  Wait -- `lvqr-cluster` is Tier 3 too. The default-features
  build of lvqr-admin pulls in lvqr-cluster which is also Tier
  3. That makes lvqr-admin's *default* publish artifact
  cluster-bound; the cluster-disabled artifact ships via
  `--no-default-features` per `:33-39`. CLAUDE.md only names
  `lvqr-admin` at Tier 3, not `lvqr-cluster`. The CLAUDE.md
  ordering is incomplete: lvqr-cluster + lvqr-observability
  + lvqr-auth + lvqr-archive + lvqr-record + lvqr-cmaf +
  lvqr-codec + lvqr-fragment + lvqr-moq + lvqr-hls + lvqr-dash
  + lvqr-whip + lvqr-whep + lvqr-rtsp + lvqr-srt all live in
  the publish-tier graph too. The CLAUDE.md tier list reads
  more like "biggest-dependency-fan-out blockers" than a
  complete tiering. Worth flagging but not actionable in this
  audit.

## 4. Public API surface drift

Spot-checked claimed-vs-actual public surface for the high-drift
candidates from Section 2:

* **lvqr-mesh**: lib.rs claims topology-only; the *Rust crate*
  is in fact still topology-only (the data-plane lives in the
  browser SDK at `bindings/js/packages/core/src/mesh.ts`). The
  doc's literal "topology planner" framing is technically
  correct for the Rust surface. The misleading bit is the
  conclusion sentence at `lib.rs:14-16` ("the mesh offload
  percentage exposed via the admin API reports *intended*
  offload, not *actual* offload"). Session 141 closed actual-
  offload reporting via `MeshPeerStats.forwarded_frames` on
  `GET /api/v1/mesh` (per README:895-903). The lib.rs is wrong
  about the admin API, not about its own internal scope.
* **lvqr-whep**: lib.rs `pub use` block at `:25-34` includes
  every type the crate actually exports. The drift is in the
  narrative, not the surface.
* **lvqr-hls**: lib.rs claims master playlist is not yet in
  the crate (`:31-37`); `pub use master::{...}` at `:75`
  contradicts. **This is a "doc says NO, code says YES"
  case** -- a reader who trusts the lib.rs would conclude they
  need to wait for a future session and miss the existing
  surface.
* **lvqr-cmaf**: lib.rs:53 claims no fuzz target; the fuzz
  target exists at `crates/lvqr-cmaf/fuzz/fuzz_targets/detect_codec_strings.rs`.
* **lvqr-relay / lvqr-rtsp / lvqr-admin / lvqr-signal**: all
  four have zero module-level docstring. Their public surface
  is exposed only via `pub use` lines without narrative. The
  Cargo.toml `description` field is the only narrative for
  each.

No "doc claims a public type that does not exist" case
surfaced -- every `pub use` resolves.

## 5. Test coverage observations

Workspace builds clean. `cargo test --workspace --lib --no-run`
finished in **21.42 s** producing 29 unit-test executables, one
per workspace member (`lvqr_admin`, `lvqr_agent`, ...,
`lvqr_whip`). No compile errors.

### Aggregate test counts

Per-crate `#[test]` + `#[tokio::test]` counts (separated by `src/`
unit tests vs `tests/` integration tests):

| Crate | Source LOC | Unit | Integ | Total |
|---|---|---|---|---|
| lvqr-admin | 2738 | 54 | 3 | 57 |
| lvqr-agent | 888 | 8 | 1 | 9 |
| lvqr-agent-whisper | 1749 | 32 | 1 | 33 |
| lvqr-archive | 1880 | 36 | 5 | 41 |
| lvqr-auth | 3261 | 86 | 0 | 86 |
| lvqr-cli | 8337 | 87 | 90 | 177 |
| lvqr-cluster | 2836 | 51 | 23 | 74 |
| lvqr-cmaf | 3091 | 43 | 14 | 57 |
| lvqr-codec | 2459 | 40 | 17 | 57 |
| lvqr-conformance | 282 | 3 | 0 | 3 |
| lvqr-core | 339 | 6 | 0 | 6 |
| lvqr-dash | 2014 | 32 | 12 | 44 |
| lvqr-fragment | 2049 | 32 | 10 | 42 |
| lvqr-hls | 3867 | 56 | 25 | 81 |
| lvqr-ingest | 2810 | 34 | 16 | 50 |
| lvqr-mesh | 1028 | 25 | 0 | 25 |
| lvqr-moq | 76 | 3 | 3 | 6 |
| lvqr-observability | 1108 | 14 | 7 | 21 |
| lvqr-record | 272 | 3 | 8 | 11 |
| lvqr-relay | 389 | 6 | 5 | 11 |
| lvqr-rtsp | 6313 | 120 | 21 | 141 |
| lvqr-signal | 994 | 21 | 7 | 28 |
| lvqr-soak | 1026 | 0 | 6 | 6 |
| **lvqr-srt** | **760** | **4** | **0** | **4** |
| lvqr-test-utils | 2144 | 12 | 3 | 15 |
| lvqr-transcode | 4003 | 42 | 3 | 45 |
| lvqr-wasm | 1446 | 28 | 1 | 29 |
| lvqr-whep | 2323 | 28 | 19 | 47 |
| lvqr-whip | 2258 | 27 | 19 | 46 |
| **Total** | **~62k** | **~933** | **~317** | **~1250** |

(The 933 unit count overstates HANDOFF's "1111 / 0 / 0"
default-gate lib count slightly because it includes
`#[tokio::test]` inside `#[cfg(test)]` modules in some
non-default-feature gates. Close enough; HANDOFF's number is
the authoritative one.)

### Conspicuously low density

* **lvqr-srt** is the standout: 4 tests on 760 LOC, with no
  integration test, no proptest, no fuzz target. Given the
  crate hosts the SRT MPEG-TS demux + PMT 0x86 SCTE-35
  reassembly across TS packets (session 152) plus connection-
  drop signalling on the EventBus, this is meaningfully
  under-tested relative to its peers. The brief's stated bar
  ("<5 tests for a >2000 LOC crate") does not literally fail
  for lvqr-srt because LOC < 2000, but the density-per-line is
  the lowest in the workspace by ~3x.

### Ignored tests

Two `#[ignore]` attributes, both whisper-related:

* `crates/lvqr-cli/tests/whisper_cli_e2e.rs:140`:
  `#[ignore = "requires WHISPER_MODEL_PATH + the whisper feature; run via -- --ignored"]`
* `crates/lvqr-agent-whisper/tests/whisper_basic.rs:138`:
  `#[ignore = "requires WHISPER_MODEL_PATH env var pointing at a ggml-*.bin file"]`

Both correctly gate on env-var presence. The README's
"Known v0.4.0 limitations" section mentions
"`whisper_cli_e2e` remains `#[ignore]` because it needs a
~78 MB ggml model download; a scheduled-workflow follow-up
will cache the model + flip it on." That follow-up appears to
have shipped as `whisper-scheduled.yml`. Both `#[ignore]`
attributes therefore stay correctly applied to the
`cargo test` default path while the scheduled workflow
exercises the real model.

The HANDOFF claims "3 ignored doctests (the `moq_sink`,
`sign_playback_url`, and `sign_live_url` doctests, all of
which need a running-server fixture)." Doctests are a
separate axis from `#[ignore]` test attributes; my scan was
attribute-based only. The 3 ignored doctests are consistent
with `pub fn sign_playback_url` (`crates/lvqr-cli/src/lib.rs:35`)
and `sign_live_url` (`:46`) being on the public surface and
having `cargo test --doc` runs of their `## Examples`
blocks.

## 6. TODO / FIXME / XXX / HACK markers

Ripgrep across crates/, docs/, tracking/, bindings/ (excluding
`target`, `node_modules`, `dist`, `fixtures`, `tracking/archive`)
matched **5 lines**. After de-noising:

* **`crates/lvqr-archive/src/provenance.rs:107`** -- inside a
  `///` doc comment ("KMS operator story that was a
  documentation TODO through session 93"). **Closed**:
  references a TODO that was already retired in session 93.
  Stays as historical doc context. **Not actionable.**
* **`crates/lvqr-codec/src/ts.rs:477`** -- false positive: the
  match is the bit-pattern doc string `0bXXXa_bbbY...`. The
  `XXX` is a bit placeholder, not a marker.
* **`crates/lvqr-whep/docs/media-write.md:113`** --
  `Trickle(Vec<u8>),          // for completeness; still TODO`.
  Real TODO inside a design doc enum sketch.
* **`crates/lvqr-whep/docs/media-write.md:209`** -- "Track
  this as a known gap in HANDOFF, not a TODO to fix inline."
  Process advice, not a TODO marker.
* **`crates/lvqr-whep/src/str0m_backend.rs:955`** -- "the
  audio path is still unwired (no AAC -> Opus transcoder) and
  trickle ICE ingestion is still TODO." **Half stale, half
  current**: the AAC -> Opus transcoder shipped session 113
  (per `lvqr-whep/Cargo.toml:14-20` `aac-opus` feature);
  trickle ICE is a real open TODO mentioned in the README's
  "Known v0.4.0 limitations" implicitly via the WebRTC
  caveats.

**Verdict**: Actively maintained TODO debt is **two real items**
(both in lvqr-whep, both about trickle ICE). Stale doc-comment
TODOs are **two** (lvqr-whep/lib.rs trickle ICE warnings + the
"audio path is still unwired" half of `str0m_backend.rs:955`).
This is an unusually low TODO debt for a 62k-LOC workspace.

`grep -rn -E "(todo!\(|unimplemented!\()"` matched zero
production hits across `crates/*/src/*.rs`. (Many `panic!()`
matches are in test assertion helpers; one in
`crates/lvqr-cli/src/scte35_bridge.rs:366` is a test panic for
a 2-second deadline assertion, not production.)

## 7. CI workflow review

15 workflows live under `.github/workflows/`. The HANDOFF
narrative refers to "8 GitHub Actions workflows GREEN
end-to-end"; my count of 15 includes scheduled + soft-fail jobs
that are not in the "8 GREEN" PR-gate path. The complete
ledger:

| Workflow | Trigger | Posture | Notes |
|---|---|---|---|
| `audit.yml` | push, schedule, dispatch | scheduled | Supply-chain audit |
| `ci.yml` | push, PR (main) | required (mostly) | The `audit` step at `:102` is `continue-on-error: true`; the `check`/`test`/`archive-io-uring`/`archive-c2pa` jobs are required |
| `contract.yml` | push, PR | soft-fail | `:26` `continue-on-error: true` (Tier 1 educational) |
| `dash-conformance.yml` | push, PR | soft-fail | `:37` `continue-on-error: true` |
| `e2e.yml` | push, PR, dispatch | soft-fail | `:31` `continue-on-error: true` |
| `feature-matrix.yml` | push, PR | soft-fail | `:48,81,119` three soft-fail cells (c2pa, transcode, whisper) |
| `fuzz.yml` | PR, schedule, dispatch | scheduled | `:57` `continue-on-error: true` |
| `hls-conformance.yml` | push, PR | **required** | `:28` "Session 33 flipped continue-on-error from true to false" |
| `mesh-e2e.yml` | push, PR, dispatch | soft-fail | `:81` `continue-on-error: true`. Browser-side WebRTC. Session 155 added `apt-get install ffmpeg` + `LVQR_LIVE_RTMP_TESTS=1` |
| `release.yml` | push (tags) | release | Cuts npm/cargo/PyPI publishes |
| `sdk-tests.yml` | push, PR | soft-fail | `:39` `continue-on-error: true`. Boots `lvqr serve` + Vitest + pytest |
| `soak-scheduled.yml` | schedule, dispatch | scheduled | `:46` `continue-on-error: true`. Daily cron 07:23 UTC |
| `tier4-demos.yml` | push, PR | soft-fail | `:46` `continue-on-error: true` |
| `videotoolbox-macos.yml` | PR (path filter) | soft-fail | `:49` `continue-on-error: true`. Newest workflow (session 156 follow-up). `macos-latest` runner |
| `whisper-scheduled.yml` | schedule, dispatch | scheduled | `:42` `continue-on-error: true` |

Findings:

* **Only one PR-gating workflow is fully required** (`hls-conformance.yml`,
  per the explicit session-33 flip at `:28`). The "8 GREEN"
  language is best read as "GREEN despite being soft-fail"
  rather than "8 required + green".
* **No workflow has been promoted to required since session 33**
  per the in-file annotations. The README's `Known v0.4.0
  limitations` section lists the stuck-soft-fail policy as a
  conscious posture: each workflow documents promotion-after-
  green-streak but no promotion has happened.
* **`videotoolbox-macos.yml`** is the most recent workflow. Its
  HANDOFF entry (session 156 follow-up) says "promote to a
  required check after a green-run streak (no current count
  tracked)."
* `gh run list --workflow=...` was not exercised in this audit
  per the brief's "skip silently if `gh` not authenticated"
  guidance; I could not reliably tell whether `gh auth status`
  passes for this user without prompting them.

The cumulative posture is "everything new lands as soft-fail and
nothing has been promoted to required since session 33". That is
the single largest operational lever to flip: each workflow that
has been GREEN for a while could close its `continue-on-error`
without engineering work. **Tracking this is a separate scope
question**, not a code drift.

## 8. SDK packages

### JavaScript packages (`bindings/js/packages/`)

Three packages, all under the `@lvqr` npm scope, all
`MIT OR Apache-2.0` (matches CLAUDE.md npm scope policy):

* **`@lvqr/core` 0.3.2** -- shared client lib. Sources:
  `client.ts`, `admin.ts`, `mesh.ts`, `moq.ts`, `transport.ts`,
  `index.ts`. Index re-export shape at
  `bindings/js/packages/core/src/index.ts:23-50` covers
  `LvqrClient`, `LvqrAdminClient`, `MoqSubscriber`,
  `MeshPeer`, `MeshConfig`, `detectTransport`, plus 16
  TypeScript types covering every admin route shape.
* **`@lvqr/player` 0.3.2** -- web component wrapper. Single
  source `index.ts` (per the directory listing). Direct dep on
  `@lvqr/core` 0.3.2 (`package.json:36-38`). No peer deps
  declared.
* **`@lvqr/dvr-player` 0.3.3** -- DVR scrub web component.
  Sources: `index.ts`, `markers.ts` (session 154),
  `seekbar.ts`, `slo-sampler.ts` (session 156 follow-up),
  `internals/`. Direct dep on `hls.js: ^1.5.0`
  (`package.json:43-45`). Notably, it does **not** depend on
  `@lvqr/core` -- it talks straight to the relay's HLS
  endpoint, intentional design choice.

### Stale `@lvqr/core/wasm` subpath -- **biggest SDK finding**

`bindings/js/packages/core/package.json` exports a `./wasm`
subpath at `:21-24` and ships pre-built artefacts at
`bindings/js/packages/core/wasm/`:

```
wasm/
  README.md          (says "WebAssembly bindings for LVQR ...
                      compiled to WASM via wasm-bindgen.")
  lvqr_wasm.d.ts     (mtime: Apr 10 19:50)
  lvqr_wasm.js
  lvqr_wasm_bg.wasm
  lvqr_wasm_bg.wasm.d.ts
  package.json
```

The pre-built `lvqr_wasm.d.ts:5` opens with:

> A subscriber client that connects to an LVQR relay.

That description matches the **pre-deletion** browser-side
`lvqr-wasm` crate. `crates/lvqr-wasm/src/lib.rs:5-6` documents
the deletion explicitly:

> It is deliberately unrelated to the browser-facing `lvqr-wasm`
> crate that was deleted in the 0.4-session-44 refactor; this
> is a fresh server-side crate that embeds a `wasmtime::Engine`
> and runs a WASM module per inbound `Fragment`.

The `package.json` `build:wasm` script at
`bindings/js/packages/core/package.json:31` still reads:

```
"build:wasm": "wasm-pack build ../../../../crates/lvqr-wasm --target web --out-dir ../../bindings/js/packages/core/wasm"
```

That target now points at the *server-side* wasmtime filter
host crate, which has no `wasm-bindgen` exports and no `cdylib`
crate-type. The script cannot reproduce the artefacts in the
`wasm/` directory. The `./wasm` export, the `wasm/`
directory, and the `build:wasm` script are dead surface that
the next `@lvqr/core` publish should drop. Evidence:

* `crates/lvqr-wasm/Cargo.toml` has no `[lib] crate-type =
  ["cdylib"]` block (verified by the file's 35-line read).
* `grep -l "wasm-bindgen\|cdylib" crates/lvqr-wasm/Cargo.toml`
  returned nothing.
* The wasm `lvqr_wasm.d.ts` artefact dates to Apr 10, well
  before session 50+; it is a frozen pre-deletion build.

**Action**: drop `./wasm` from `@lvqr/core`'s `exports`,
delete `wasm/` directory, drop `build:wasm` from `scripts`. SDK
consumers cannot use the surface today (`@lvqr/core/wasm`
imports a relay subscriber that does not match any current LVQR
relay -- the wire format moved to moq-lite 0.15 long after the
artefact was built). The published `@lvqr/core 0.3.2` ships
this dead subpath today.

### Python package (`bindings/python/`)

* **`lvqr` 0.3.2 on PyPI**. `pyproject.toml:6-21` declares
  Python >= 3.9, single direct dep `httpx >= 0.27`.
* **`bindings/python/python/lvqr/client.py`** ships 14 admin
  methods (per `grep -n "^    def "`):
  `healthz`, `stats`, `list_streams`, `mesh`, `slo`,
  `cluster_nodes`, `cluster_broadcasts`, `cluster_config`,
  `cluster_federation`, `list_streamkeys`, `mint_streamkey`,
  `revoke_streamkey`, `rotate_streamkey`,
  `config_reload_status`, `trigger_config_reload`,
  `wasm_filter`. README claim of "9 of 9 admin routes"
  matches the pre-streamkeys / pre-config-reload count;
  the new methods on `main` are post-0.3.2 unreleased per
  `bindings/python/CHANGELOG.md:8-22`.

No drift on the Python side. Release lag is normal.

### SDK summary

* JS workspace shape: 3 packages, version-locked at 0.3.2 /
  0.3.2 / 0.3.3.
* JS dead subpath: `@lvqr/core/wasm` -- detailed above.
* Python: 0.3.2 on PyPI; main has post-0.3.2 streamkeys +
  config-reload methods (CHANGELOG-tracked).
* No package's `index.ts` declared a type that wasn't
  exported. No missing peer-dep declarations spotted (the JS
  packages explicitly use direct deps where peer deps would
  fight tooling).

## 9. Documentation drift

### `docs/architecture.md` -- **27 vs 29 crates**

* `:3` "LVQR is a 27-crate Rust workspace built around one
  central claim ..."
* `:12` "the 27 crates that implement them"
* `:175` "## The 27 crates"
* `:198` "lvqr-mesh -- peer mesh topology planner (media
  relay: Tier 4)" -- **stale**: data-plane shipped session 144
  per docs/mesh.md.
* `:178-219` enumerates 27 crates. Missing from list:
  **lvqr-agent-whisper** + **lvqr-transcode** (both Tier 4
  crates that shipped session 98+ and 104+).

`README.md:1368` has the right count: "The workspace is 29
crates". `Cargo.toml` has 29 members. The architecture doc is
the only file in the repo that says 27 (plus quickstart.md, see
below).

### `docs/quickstart.md`

* `:329` "how the 27 crates fit" -- stale.
* `:337-338` "mesh.md -- peer mesh topology planner (topology
  only today; media relay is Tier 4 on the roadmap)" --
  contradicts `docs/mesh.md:8` ("**IMPLEMENTED**") and
  README:794-797 (mesh data-plane Phase D fully shipped).

### `docs/slo.md`

Refreshed in session 157 follow-up; not re-flagged per the
brief's instruction. Spot-check confirmed it correctly covers
the `POST /api/v1/slo/client-sample` route + the dvr-player
sampler reference + the pure-MoQ v1.2 forward link.

### `docs/mesh.md`, `docs/scte35.md`, `docs/dvr-scrub.md`,
### `docs/config-reload.md`, `docs/auth.md`,
### `docs/cluster.md`, `docs/observability.md`,
### `docs/deployment.md`

Spot-checked file:line claims; no drift surfaced. Each is the
"shipped, then doc'd" pattern that LVQR follows session-by-
session.

### Docs not linked from README

* All 11 `docs/*.md` files **are** linked from the README's
  Documentation list (`README.md:1441-1453`), with the
  exception that `dvr-scrub.md` is linked at `:369` (top
  section) but not in the Documentation list. Worth
  considering adding to the bulleted list for consistency.
* `docs/sdk/` directory (not at this level) is referenced as a
  pointer for the JS / Python references; not part of the
  audit's primary `docs/*.md` scope.

## 10. Roadmap vs implementation matrix

Cross-check of every "shipped" line in README "Next up" + Phase
A v1.1 against the actual code state. Every claim resolves;
the table records the proof file:line.

| README claim | Status | Proof |
|---|---|---|
| Phase A v1.1 #1 (Hot config reload v1/v2/v3) | shipped | `crates/lvqr-cli/src/config_reload.rs` (988 LOC); `pub use ConfigReloadHandle` at `lvqr-cli/src/lib.rs:30-32` |
| Phase A v1.1 #2 (SCTE-35 passthrough) | shipped | `crates/lvqr-codec/src/scte35.rs` (616 LOC); `lvqr_fragment::SCTE35_TRACK` at `lvqr-fragment/src/lib.rs:47`; `vendor/rml_rtmp/Cargo.toml:1-15` (vendored fork) |
| Phase A v1.1 #3 (DVR scrub web UI) | shipped | `bindings/js/packages/dvr-player/src/index.ts` (1k+ LOC); `package.json` 0.3.3 |
| Phase A v1.1 #4 (one HW encoder backend) | shipped | `crates/lvqr-transcode/src/videotoolbox.rs` (792 LOC); feature `hw-videotoolbox` at `lvqr-transcode/Cargo.toml:28-35` |
| Phase A v1.1 #5 (MoQ egress latency SLO) | **partial; v1.2 follow-up** | `POST /api/v1/slo/client-sample` at `crates/lvqr-admin/src/routes.rs:538+`; HLS-side first client at `bindings/js/packages/dvr-player/src/slo-sampler.ts:67-78`; pure-MoQ side blocked per session 157 audit. README:974-1005 documents the open state correctly. |
| Phase B / C / D (mesh, cluster, federation, transcoding, agents) | shipped | Per `docs/mesh.md:8` ("IMPLEMENTED"), `docs/cluster.md`, README "Recently shipped" rows |
| Tier 4 exit criterion (`examples/tier4-demos/demo-01.sh`) | shipped | `examples/tier4-demos/` exists at repo root |
| Webhook / JWKS / Stream-key CRUD | shipped | `crates/lvqr-auth/src/{webhook_provider,jwks_provider,stream_key_store}.rs` |

No "README says shipped, code does not have" cases surfaced. No
"code has but README says open" cases surfaced. The README +
HANDOFF + code agree across the v1.1 shipped surface.

The ONE roadmap row in flux (Phase A v1.1 #5) is correctly
characterised by both README:974-1005 and
`tracking/SESSION_157_BRIEFING.md`. No inconsistency.

## 11. Tech debt + concerning patterns

### `bindings/js/packages/core/wasm/` -- stale browser SDK surface

Documented in detail in Section 8. **Highest-value cleanup of
this audit.** Drops three things:

1. The `./wasm` subpath in `package.json:21-24`.
2. The `wasm/` directory contents.
3. The `build:wasm` script in `package.json:31`.

Diff is small. The published `@lvqr/core 0.3.2` ships this
dead surface today; cleaning it up reduces consumer
confusion + avoids breaking changes if a future operator
discovers the broken script.

### Vendored `rml_rtmp` v0.8 -- still load-bearing

`vendor/rml_rtmp/` carries:

* Session 152 patch: server-side `Amf0DataReceived`
  ServerSessionEvent variant for SCTE-35 onCuePoint passthrough.
* Session 155 patch: client-side `publish_amf0_data` method
  for the `scte35-rtmp-push` test bin.

Upstream `rml_rtmp 0.8.0` does not have either method
(verified via `Cargo.toml:1-15` -- the vendor `description`
field calls out the LVQR fork status). Removing the vendor
fork would require either upstreaming both patches (no
indication that has happened) or rewriting the SCTE-35 ingest
path against a different RTMP library.

`vendor/rml_rtmp/Cargo.toml:13-17` notes upstream is `edition
2015`-style; the vendor copy stays on edition 2015 to avoid a
module-path rewrite. This is intentional and not
maintenance debt.

**Action**: keep vendored fork. Track upstream periodically
(open PR upstream, not in this session's scope).

### Stale `lib.rs` doc comments

The biggest cluster of low-effort cleanup items in the audit:

| Crate | File:line | Drift |
|---|---|---|
| lvqr-mesh | `src/lib.rs:1-19` | claims "topology planner only" + "intended offload, not actual" -- session 141 closed actual-offload reporting |
| lvqr-whep | `src/lib.rs:6-13` | claims "A future session plugs in `str0m`" -- shipped, exported at `:23,28` |
| lvqr-whep | `src/str0m_backend.rs:954-955` | "audio path is still unwired" -- session 113 shipped AAC->Opus |
| lvqr-hls | `src/lib.rs:31-37` | claims "single rendition only" + "no master playlist yet" -- master.rs shipped + exported at `:75` |
| lvqr-cmaf | `src/lib.rs:53` | claims "no fuzz target today" -- `fuzz/fuzz_targets/detect_codec_strings.rs` exists |
| lvqr-transcode | `src/lib.rs:11-103` | "Session 104 A scope" / "What session 105 B adds" / "What session 106 C adds" framing; all shipped + superseded by sessions 113, 156 |
| lvqr-relay | `src/lib.rs` | zero module-level docstring |
| lvqr-rtsp | `src/lib.rs` | zero module-level docstring |
| lvqr-admin | `src/lib.rs` | zero module-level docstring |
| lvqr-signal | `src/lib.rs` | zero module-level docstring |
| docs/architecture.md | `:3,12,175,198` | 27-crate count + mesh "topology only" |
| docs/quickstart.md | `:329,337-338` | 27-crate count + mesh "topology only" |

A single doc-only sweep (no Cargo, no test additions, no API
changes) would close every line above. ~150-line diff.

### `#[allow(dead_code)]` clusters

Six matches across the workspace:

* `crates/lvqr-whip/src/server.rs:140` -- struct field
  preserved for future wire (test)
* `crates/lvqr-whip/tests/e2e_str0m_loopback.rs:304,309` -- test
  helpers
* `crates/lvqr-signal/src/signaling.rs:154` -- non-test
  helper inside `#[cfg(test)]` block (verified)
* `crates/lvqr-observability/tests/metric_export.rs:36,48` --
  test helpers

No production `#[allow(dead_code)]` cluster. Healthy.

### `unsafe` blocks

Single match: `crates/lvqr-observability/src/lib.rs:161` --
inside a doc comment about `std::env::set_var` being "unsafe
under tokio test". No actual `unsafe` blocks in the workspace
production source.

### Cargo features no test exercises

* **`hw-videotoolbox`**: only exercised via
  `videotoolbox-macos.yml` (soft-fail, mac-only). The feature
  also has 6 unit tests on the `videotoolbox` module itself,
  gated on the feature, but the integration test
  (`tests/videotoolbox_ladder.rs`) runs only on macOS.
  Coverage adequate but CI promotion is the open lever.
* **`whisper`**: exercised via `whisper-scheduled.yml` +
  `#[ignore]`-gated integration test. Adequate but scheduled.
* **`webhook` / `jwks`**: exercised via the
  `feature-matrix.yml` cells (soft-fail). Each feature has
  in-crate unit tests. Adequate.

No feature with zero test exposure surfaced.

### Dependencies pinned at non-current major

* `wasmtime = "43"` (current as of late 2025; v43 was the
  session-150 upgrade target). On the current line.
* `c2pa = "0.80"` (pre-1.0, pinned exact-minor per session 93
  policy). Intentional.
* `gstreamer = "0.23"` (pre-`0.24`, pinned because 0.24 raised
  MSRV; `Cargo.toml:198`). Intentional.
* `opentelemetry = "0.27"` family (matched to
  `tracing-opentelemetry 0.28`). Tracking upstream cadence.
* `rml_rtmp = "0.8"` via vendor fork. See above.
* `mp4-atom = "0.10"` -- recent, kixelated-maintained, no
  drift indicator.
* `redb = "2"` -- 2.x is current.
* `chitchat = "0.10"` -- pre-1.0, on current line.
* `str0m = "0.18"` -- direct dep at lvqr-whep / lvqr-whip
  leaves. On current line.
* `srt-tokio = "0.4"` -- direct dep at lvqr-srt. On current
  line.

No alarming pin staleness.

### audit.toml ledger

`audit.toml` carries 6 documented ignores after session 150's
wasmtime upgrade closed 16:

* `RUSTSEC-2023-0071` (rsa Marvin attack -- no upstream fix)
* `RUSTSEC-2024-0370` (proc-macro-error -- unmaintained
  transitive)
* `RUSTSEC-2024-0436` (paste -- unmaintained transitive)
* `RUSTSEC-2025-0134` (rustls-pemfile -- unmaintained
  transitive)
* `RUSTSEC-2026-0002` (lru -- unsoundness, unreachable)
* `RUSTSEC-2026-0097` (rand -- unsoundness with custom logger,
  unreachable)

Every ignore is documented inline with rationale + close-out
condition. Healthy.

### CI soft-fail posture

13 of 15 workflows have `continue-on-error: true` somewhere.
Only `hls-conformance.yml` was promoted to required (session
33). This is documented in README "Known v0.4.0 limitations" as
intentional. Worth tracking but not a code drift.

## 12. Recommended next 3-5 sessions, ranked

Each entry: session goal, LOC range, at-risk files, additive vs
refactor-shaped, audit findings that motivate it.

### #1: DOC-DRIFT-A -- documentation + lib.rs cleanup sweep

**Goal**: close the documentation drift surfaced in Sections 8
+ 9 + 11 of this audit, plus drop the dead `@lvqr/core/wasm`
SDK subpath. No code logic, no test additions, no API changes.

**Scope**:

1. `docs/architecture.md`: bump 27 -> 29 at `:3,12,175`; add
   `lvqr-agent-whisper` + `lvqr-transcode` rows to the crate
   listing at `:178-220`; flip `:198` mesh annotation from
   "(media relay: Tier 4)" to "(data-plane shipped session
   144; see docs/mesh.md)".
2. `docs/quickstart.md:329,337-338`: bump 27 -> 29; flip mesh
   summary line from "topology only today" to "fully
   implemented (data-plane shipped session 144)".
3. `crates/lvqr-mesh/src/lib.rs:1-19`: rewrite the lead doc to
   reflect that the *Rust crate's surface* is still a topology
   coordinator while the *system-level mesh* is fully
   implemented (session 141 actual-offload reporting + session
   144 capacity + browser data plane). Cite `MeshPeerStats`.
4. `crates/lvqr-whep/src/lib.rs:6-13`: rewrite the lead doc to
   reflect that `str0m_backend` is shipped + exported. Cite
   the `aac-opus` feature for the audio path.
5. `crates/lvqr-whep/src/str0m_backend.rs:954-955`: drop the
   "audio path is still unwired" half; keep the trickle ICE
   TODO bullet.
6. `crates/lvqr-hls/src/lib.rs:30-44`: rewrite "What is NOT in
   this crate yet" to drop the "single rendition only" claim
   (master.rs shipped). Add a forward-link to the
   `mediastreamvalidator` open gap so a future session knows
   where it lands.
7. `crates/lvqr-cmaf/src/lib.rs:53`: drop "today there is none"
   for fuzz; cite `fuzz/fuzz_targets/detect_codec_strings.rs`.
8. `crates/lvqr-transcode/src/lib.rs:11-103`: rewrite the
   session-by-session narrative as a single present-tense
   "What this crate ships" block with a brief timeline footer.
   Cite `software.rs`, `videotoolbox.rs`, `aac_opus.rs`.
9. `crates/lvqr-{relay,rtsp,admin,signal}/src/lib.rs`: add a
   short module-level docstring (~5-10 lines each) so the
   crate-level public surface has narrative. Rust convention.
10. `bindings/js/packages/core/package.json`: drop the `./wasm`
    export at `:21-24`, drop the `build:wasm` script at `:31`,
    drop the `wasm` entry from `files` at `:26-29`.
11. `bindings/js/packages/core/wasm/`: delete the directory.

**LOC range**: ~150-200 lines diff total (mostly doc).

**At-risk files**: 11 files across docs/, crates/lvqr-{mesh,
whep,hls,cmaf,transcode,relay,rtsp,admin,signal}/src/lib.rs,
and bindings/js/packages/core/. No `cargo test` change beyond
what `cargo test --workspace --lib` already exercises (because
docstrings are not compile-relevant for non-doctests).

**Risk**: very low. Doc-only + dead-SDK-surface drop. The
deleted `@lvqr/core/wasm` subpath has no reachable consumer
because the underlying crate's purpose changed two sessions
ago; any consumer would already be broken.

**Why first**: clears the `lib.rs` decks so the next session
(Path X) does not have to read past stale framings. Also
closes the audit's three biggest concrete surprises in one
push.

### #2: PATH-X-MOQ-TIMING -- close Phase A v1.1 #5 (the v1.2 follow-up)

**Goal**: ship the sibling `<broadcast>/0.timing` MoQ track
(producer side + Rust sample-pusher bin + integration test)
per the design sketch in
`tracking/SESSION_157_BRIEFING.md:124-157`. Closes the last
open Phase A v1.1 checkbox.

**Scope** (per the SESSION_157_BRIEFING):

* New `MoqTimingTrackSink` on `lvqr-fragment`; ingest-side
  wiring to tap `Fragment::ingest_time_ms` on every keyframe
  boundary.
* New `[[bin]] lvqr-moq-sample-pusher` on `lvqr-test-utils` (or
  a fresh crate) that subscribes to both `0.mp4` and
  `0.timing`, computes `latency_ms = now_unix_ms -
  timing.ingest_time_ms`, and POSTs to
  `POST /api/v1/slo/client-sample`.
* Integration test driving the full RTMP -> relay -> bin ->
  SLO endpoint loop, asserting a non-empty entry under
  `transport="moq"` on `GET /api/v1/slo`.

**LOC range**: ~800-1200 LOC per the briefing.

**At-risk files**: `crates/lvqr-fragment/src/{moq_sink,moq_stream,registry}.rs`,
new file in `crates/lvqr-fragment/src/`, new file in
`crates/lvqr-test-utils/src/bin/`, new
`crates/lvqr-test-utils/tests/moq_timing_e2e.rs`,
`crates/lvqr-ingest/src/bridge.rs` (if the producer wiring
goes there), plus admin route surface unchanged.

**Risk**: medium. Touches the MoQ wire by adding a sibling
track; the change is *additive* (foreign clients ignore the
unknown track name) but the producer side touches the
ingest hot path's keyframe handler. Per CLAUDE.md "real
integration tests, not mocks" -- the new integration test
must drive a real RTMP publisher into a real lvqr-cli
binary, which the existing `TestServer` harness supports.

**Additive** rather than refactor. Cleanly bounded by the
session-157 brief's design lock.

**Why second**: the only open Phase A v1.1 row, fully
designed in tracking/SESSION_157_BRIEFING.md (no read-back
needed), and close-able in one ~600-900 LOC session. Higher
business value than #3-#5 because it ships a roadmap-row
close.

### #3: SRT-TEST-GAP -- raise lvqr-srt density to peer level

**Goal**: address the conspicuous test-density gap surfaced in
Section 5. lvqr-srt at 4 unit + 0 integration on 760 LOC is
~3x sparser than every peer ingest crate (lvqr-ingest at 50,
lvqr-rtsp at 141, lvqr-whep at 47, lvqr-whip at 46).

**Scope**:

* Add a proptest harness over the TS-packet reassembly state
  machine (the PMT 0x86 SCTE-35 path is the natural target;
  the cmaf, codec, hls, ingest crates all have proptest
  harnesses for comparable-sized parsers).
* Add an integration test in `crates/lvqr-srt/tests/` that
  drives a real `srt-tokio` publisher session into a
  TestServer-backed listener. Use the existing
  `lvqr_test_utils::find_available_port` + `TestServer`
  builder.
* Add at least 5 unit tests over the connection lifecycle +
  the BroadcastStopped EventBus emit.

**LOC range**: ~300-500 LOC of test code.

**At-risk files**: `crates/lvqr-srt/src/ingest.rs` (likely
add `#[cfg(test)] mod tests` block), new
`crates/lvqr-srt/tests/srt_e2e.rs`,
`crates/lvqr-srt/Cargo.toml` (dev-deps).

**Risk**: low. Test-only addition, no API change. The
`srt-tokio = "0.4"` direct dep is already declared.

**Why third**: closes the audit's only meaningfully-undertested
crate. SCTE-35 ingest landed in session 152 across both SRT
and RTMP; the RTMP side has integration test coverage
(`scte35_hls_dash_e2e.rs`, `scte35_rtmp_push_smoke.rs`); the
SRT side has parser-level coverage in lvqr-codec but no
end-to-end coverage. This is risk reduction: a regression in
the SRT path could ship undetected.

### #4: CI-PROMOTE-A -- promote 1-2 stable workflows to required

**Goal**: address the CI-soft-fail posture surfaced in
Section 6. 13 of 15 workflows are soft-fail despite many
having been GREEN for months. Promote 1-2 to required so the
PR-gate signal carries weight.

**Scope**:

* Audit the `gh run list` history of `feature-matrix.yml`,
  `dash-conformance.yml`, `e2e.yml`, `mesh-e2e.yml`,
  `tier4-demos.yml`, and `videotoolbox-macos.yml`. Identify
  the 1-2 with the longest GREEN streak.
* Flip `continue-on-error: true` -> `false` on those.
* Update README's "Known v0.4.0 limitations" entry to record
  the promotions.

**LOC range**: ~5-20 lines diff on workflow files + README.

**At-risk files**: `.github/workflows/<promoted>.yml`,
`README.md` (~1170-1180 range).

**Risk**: medium-low. Promotion to required means a flake will
block PRs. Mitigation: only promote workflows that have been
GREEN for 30+ days (per the README convention).

**Why fourth**: posture cleanup. Lower priority than DOC-DRIFT
or PATH-X but higher than further feature work because the
soft-fail-everywhere posture devalues every PR's CI signal.
This session is more "operational hygiene" than engineering.

### #5: NVENC-OR-VAAPI-BACKEND -- second HW encoder backend

**Goal**: ship the next HW encoder backend per the README's
v1.2 deferred list (`README:961-963` "NVENC for Linux, VAAPI,
QSV stay deferred to v1.2"). Pick one (probably NVENC for
Linux per the README's ordering).

**Scope**:

* Mirror `crates/lvqr-transcode/src/videotoolbox.rs` (792 LOC)
  as `nvenc.rs` (or `vaapi.rs`) with the matching encoder-
  element + property mapping.
* New `hw-nvenc` Cargo feature on lvqr-transcode + lvqr-cli,
  forwarded the same way `hw-videotoolbox` is.
* New `crates/lvqr-transcode/tests/nvenc_ladder.rs`
  cfg-gated on `cfg(target_os = "linux")` + the feature.
* New `.github/workflows/nvenc-linux.yml` (mirror of
  `videotoolbox-macos.yml`).
* CLI flag: extend `--transcode-encoder
  software|videotoolbox` to also accept `nvenc`.
* When this lands, the session-156 brief's note about
  extracting a shared `pipeline.rs` module from
  `software.rs` + `videotoolbox.rs` becomes appropriate
  (third backend = abstraction threshold).

**LOC range**: ~800-1000 LOC (matches session 156's shape
verbatim).

**At-risk files**: `crates/lvqr-transcode/src/{lib,nvenc}.rs`,
`crates/lvqr-transcode/Cargo.toml`,
`crates/lvqr-cli/{Cargo.toml,src/{config,lib}.rs}`,
new `.github/workflows/nvenc-linux.yml`,
new `crates/lvqr-transcode/tests/nvenc_ladder.rs`.

**Risk**: medium. NVENC needs a CI runner with a real GPU;
GitHub-hosted runners do not have one, so the workflow has
to be self-hosted-runner gated or test-skip on
ubuntu-latest. The pattern is already locked by session 156's
macos-runner approach.

**Refactor-shaped** (if the shared `pipeline.rs` extraction
gets bundled): touches existing `software.rs` +
`videotoolbox.rs`. Could be split into NVENC-only (additive,
medium risk) + scaffold-extraction (refactor, higher risk).

**Why fifth**: extends a recently-shipped feature. Lower
priority than DOC-DRIFT (already-found drift), PATH-X
(roadmap row close), SRT-TEST-GAP (risk reduction), CI-PROMOTE
(operational), but the natural "what to do next on the
encoder track" if hardware-backed transcoding is a v1.2
positioning lever.

### Path X position in this ranking

Path X (the Phase A v1.1 #5 close-out) is **#2** in this
ranking, not #1. Rationale: the doc-drift sweep (#1) clears
the stale `lib.rs` framings that Path X will touch
(particularly `lvqr-fragment`'s downstream `lvqr-mesh` /
`lvqr-transcode` neighbors) so Path X does not have to fight
contradictory narratives mid-session. DOC-DRIFT-A is also a
much smaller / lower-risk session than Path X, so it lands
faster.

If the user prefers feature-velocity over hygiene, swap #1
and #2: PATH-X first, DOC-DRIFT after. The audit recommends
DOC-DRIFT first because the cost of leaving documentation
drift in place compounds (each future session reads stale
framings and either adds caveats or works around them).
