# LVQR v1.1 Plan

Authored 2026-04-21 on top of a deep audit after the Tier 4 close.
Supersedes the "post-Tier-4 candidate table" in `SESSION_110_BRIEFING.md`.

## Premise

Tier 4 is closed by its own rubric (eight items shipped, workspace at v0.4.0,
25 Rust crates + 2 npm packages + 1 PyPI package published). A 2026-04-21
audit surfaced that five "COMPLETE" claims in the handoff and README paper
over real gaps, the briefing's post-Tier-4 candidate table misranks two
items, and three test-coverage holes meaningfully undercut the "shipped"
story. This document resequences the v1.1 work around those findings.

## Verified findings the reprioritization is built on

### Silent gaps inside "COMPLETE" claims

1. **Live HLS + DASH routes are unauthenticated** even when `--auth` is
   configured. `crates/lvqr-hls/src/server.rs:7-9` explicitly defers auth
   to the CLI composition root; `crates/lvqr-cli/src/lib.rs:1492` and
   `:1507` mount both routers with only `CorsLayer::permissive()`. `/ws/*`,
   `/playback/*`, and `/api/v1/*` enforce tokens; live HLS/DASH segments
   do not. The "One token, every protocol" README claim is true for
   ingest + DVR + admin, misleading for live egress.

2. **WHEP silently drops AAC.** `crates/lvqr-whep/src/str0m_backend.rs:42-46`
   admits "There is no in-tree AAC -> Opus transcoder". Every RTMP or SRT
   or RTSP publisher feeding a WHEP subscriber is video-only. This is the
   single most user-visible v1.1 gap for the OBS-to-browser workflow.

3. **WASM filters are already mutation-capable.** `crates/lvqr-wasm/src/lib.rs:17-23`
   lets guests drop or rewrite payload bytes. The README still advertises
   v1 as a read-only tap. Docs drift, not a code gap.

4. **Hardware encoder backends are absent from Cargo.** `crates/lvqr-transcode/Cargo.toml`
   has only the `transcode` feature (software x264). No `hw-nvenc`,
   `hw-vaapi`, `hw-videotoolbox`, or `hw-qsv` feature flags. Comments in
   `lvqr-transcode/src/lib.rs` reference them as future work.

5. **Tier 4 exit criterion unshipped.** `TIER_4_PLAN.md:795` enumerates a
   working `examples/tier4-demos/` public demo script as the Tier 4 exit
   gate. It never landed. Tier 4 closed without meeting this gate.

### Test coverage holes

- **Zero WHIP to non-WebRTC E2E.** WHIP is the canonical webcam-in ingest;
  no test exercises WHIP to HLS, WHIP to DASH, WHIP to WS, WHIP to WHEP,
  WHIP to archive.
- **Zero RTMP to WHEP E2E.** OBS to browser WebRTC is the most common user
  story and has no coverage.
- **Zero browser-playback E2E.** `tests/e2e/test-app.spec.ts` lines 3-17
  document that the existing Playwright test does not exercise any
  streaming code path.
- **Zero JS and Python SDK tests in CI.** `bindings/js` has no test script;
  `bindings/python/tests/test_client.py` exists but no workflow invokes
  pytest.
- **DVR archive READ E2E absent.** Write side exercised by
  `rtmp_archive_e2e.rs`; no archive-read test covers the `/playback/*`
  scrub routes.
- **Nightly 24h soak not scheduled.** `lvqr-soak` tests run fast-path only;
  no scheduled long-duration CI job.
- **whisper, cluster, transcode features off-CI.** `.github/workflows/ci.yml`
  runs default plus `c2pa` plus `io-uring` only; three feature flags have
  no CI protection.

### Audit debt from 2026-04-13 audits with UNCONFIRMED status

Rate limiting, `/metrics` token flag, admin auth-failure metric,
`CorsLayer` tightening, `cargo audit` CI job, 5-artifact contract
enforcement (script exists, soft-fail only).

### Docs drift

- `README.md:231` marks `@lvqr/core` and `@lvqr/player` as "on the roadmap";
  both published at v0.3.1 on npm.
- `README.md:244-246` treats Tier 5 client SDKs as unchecked; JS + Python
  admin + Rust `lvqr-core` clients all exist.
- `README.md:260-264` treats "Client-side DataChannel parent/child relay
  in `@lvqr/core`" as unchecked; already implemented in
  `bindings/js/packages/core/src/mesh.ts`.

## Reprioritized phase plan

### Phase A: stop the bleeding (sessions 111-112, low risk)

| Session | Scope | Rationale |
|---|---|---|
| **111-A** | Docs accuracy sweep. Fix README drift on published SDKs, WASM mutation capability, mesh client state, Tier 4 exit criterion. Add "Known v0.4.0 limitations" section naming HLS/DASH auth gap and WHEP AAC drop. Refresh `docs/mesh.md` to reflect the JS `MeshPeer` is already shipped. | The README actively misleads anyone landing on the repo today. Zero risk, immediate value. |
| **111-B1** | **SHIPPED.** Narrower half of the original 111-B. Gated `/signal` with the subscribe-auth provider via `?token=<token>` query parameter (escape hatch: `--no-auth-signal`). Hoisted `MeshCoordinator` out of the admin-router conditional onto `ServerHandle::mesh_coordinator()`. Added `ServerHandle::signal_url()` + `ServeConfig::mesh_root_peer_count` for test harnesses. Documented the MoQ-over-DataChannel wire format (8-byte big-endian `object_id` + raw frame bytes) in `docs/mesh.md`. 6 new integration tests. | Low-risk, clear deliverable, unblocks 111-B2. |
| **111-B2** | **SHIPPED.** Every `ws_relay_session` registers the subscriber with `MeshCoordinator::add_peer` (server-generated `ws-{counter}` peer_id), sends a leading `peer_assignment` JSON text frame on the WS, and deregisters on disconnect. Two integration tests lock in the tree shape + teardown. Side fix: session main loop now polls `socket.recv()` alongside the frame-rx so idle subscribers exit promptly on client close. `/signal` register callback is idempotent via `MeshCoordinator::get_peer` so a client with an existing WS peer_id does not double-register via signaling. Sec-WebSocket-Protocol echo on `lvqr-signal` stays deferred to 111-B3. | Narrow scope shipped; 111-B3 picks up the subprotocol header auth path. |
| **112** | **SHIPPED.** Security hardening. Applied the `SubscribeAuth` provider to HLS and DASH live routes via a tower middleware at the CLI composition root. Auth on by default whenever the provider is configured (static token, JWT); `--no-auth-live-playback` flag is the escape hatch. Noop provider deployments see no behavior change. Shipped `crates/lvqr-cli/tests/hls_live_auth_e2e.rs` (7 tests) and `.github/workflows/audit.yml` (cargo audit, scheduled daily + push to main). Workspace tests went from 917 to 924 on the default gate. | Closes the live-route auth gap with zero impact on unauthed deployments and an explicit escape hatch for the narrow "I want open live HLS but gated ingest" case. |

### Phase B: user-visible wins (sessions 113-116, medium risk)

| Session | Scope | Rationale |
|---|---|---|
| **113** | **SHIPPED.** WHEP AAC-to-Opus transcoder landed behind the existing `transcode` Cargo feature (via a new `lvqr-whep/aac-opus` sub-feature that the CLI's `transcode` meta-feature activates). New `lvqr_transcode::AacToOpusEncoder` + `AacToOpusEncoderFactory` wrap a GStreamer `appsrc ! aacparse ! avdec_aac ! audioconvert ! audioresample ! opusenc ! appsink` pipeline on a worker thread (mirroring session 105's pattern). `RawSampleObserver` + `SessionHandle` gained an `on_audio_config` hook so the RTMP bridge's first AAC SequenceHeader carries the `AudioSpecificConfig` through to the WHEP session. `Str0mAnswerer::with_aac_to_opus_factory` builder; session poll loop drains Opus packets via a new `select!` arm and writes them through the negotiated Opus Pt. Integration-test target `crates/lvqr-transcode/tests/aac_opus_roundtrip.rs` generates real AAC bytes via a `audiotestsrc ! avenc_aac` pipeline, pushes them through the encoder, and asserts the Opus output shape (skips cleanly on hosts without GStreamer). | Single highest-impact v1.1 item. Closes the OBS-to-browser audio gap. |
| **114** | **SHIPPED.** Three E2E gap closers landed: WHIP-to-HLS (`crates/lvqr-cli/tests/whip_hls_e2e.rs`) via a real str0m client POSTing to the WHIP HTTP surface + completing ICE/DTLS/SRTP over loopback UDP + writing synthetic H.264 samples + asserting an LL-HLS partial or segment appears on `/hls/live/test/playlist.m3u8`; SRT-to-DASH (`crates/lvqr-cli/tests/srt_dash_e2e.rs`) via an SRT caller pushing minimal PAT+PMT+H.264 MPEG-TS + asserting the live `type="dynamic"` MPD + init segment + numbered media segment; RTMP-to-WHEP-audio (`crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs`, landed in session 115) via an `rml_rtmp` publisher pushing real AAC-LC access units generated by `audiotestsrc ! avenc_aac ! aacparse` into the session 113 AAC-to-Opus encoder path + a str0m Opus-only WHEP subscriber on `POST /whep/live/test` + asserting at least one `Event::MediaData` on the negotiated Opus Pt. The RTMP-to-WHEP test is feature-gated on `transcode` and runs on the GStreamer-enabled CI matrix alongside `aac_opus_roundtrip.rs`; it is compile-only on default-gate hosts. 5 session-113 audit unit tests landed in 114-partial (ASC parse refactored into a `parse_aac_asc` free function + 4 unit tests + 1 tokio integration-lite test on `Str0mSessionHandle::on_audio_config`). Workspace tests 934 -> 941 on the default gate (unchanged by session 115 because the new target is feature-gated out). `TestServerConfig` gained `with_whep()` + `TestServer` gained `whep_addr()` to support the new test. | Covers the three biggest cross-crate E2E gaps surfaced by the audit. |
| **115** | **SHIPPED.** Mesh data-plane step 2 closed via a Playwright two-browser E2E at `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts`. `lvqr serve --mesh-enabled --mesh-root-peer-count 1 --no-auth-signal` boots via Playwright's `webServer` block (new `--mesh-root-peer-count` CLI flag surfaces the existing `ServeConfig` field). Two browser contexts register as `peer-one` (Root) and `peer-two` (Relay with parent=peer-one). The SDP offer/answer and ICE candidates flow through `/signal`. The DataChannel opens. peer-one pushes a known 8-byte payload via a new `MeshPeer.pushFrame(data)` public method (previously the client had no way for a root peer to inject media received from the server; `forwardToChildren` was only reached from the child-side `dc.onmessage`). peer-two's `onFrame` callback observes the bytes via the DataChannel. Completes in ~270 ms on loopback. Docs/mesh.md flipped to "topology planner + signaling + subscribe-auth + server-side subscriber registration + client-side WebRTC relay + two-peer DataChannel media relay end-to-end test all ship"; phase-D scope (actual-vs-intended offload, per-peer capacity advertisement, TURN recipe, 3+ browser matrix) explicitly called out as still pending. | First real mesh data-plane milestone. |
| **116** | **SHIPPED** in session 117. Authored `examples/tier4-demos/demo-01.sh` + `examples/tier4-demos/README.md` chaining the WASM per-fragment filter (`--wasm-filter` against the in-repo `frame-counter.wasm` fixture), whisper live captions (`--whisper-model`, conditional on `LVQR_WHISPER_MODEL` env; skipped cleanly when unset), the software ABR transcode ladder (`--transcode-rendition 720p + 480p + 240p`), and the DVR archive (`--archive-dir`). A 20-second ffmpeg synthetic publish drives the chain; the script polls `/healthz`, the HLS `master.m3u8` for 4 advertised variants, `/metrics` for the WASM tap keep counter, and the scratch archive dir for per-track finalized MP4s. Prints a flat summary block + exits non-zero when the ladder or archive assertions fail. C2PA sign + verify was deliberately scoped OUT of the demo because the CLI surface for `ServeConfig.c2pa` does not exist today (it is programmatic-only via `TestServerConfig::with_c2pa`); the demo README names the gap, points at `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` as the programmatic fixture, and adds CLI C2PA wiring as a phase-C row. README reality sweep rode along: test count 917 -> 941 + 1 Playwright; WHEP AAC-to-Opus flipped to SHIPPED; mesh two-peer browser E2E flipped to SHIPPED; Tier 4 exit criterion flipped to CLOSED; `--mesh-root-peer-count` + `--no-auth-signal` added to the CLI reference; phantom `/readyz` endpoint removed; `pushFrame` / `onChildOpen` / `connectTimeoutMs` / `fetchTimeoutMs` noted in the `@lvqr/core` row; C2PA CLI-wiring gap named as a Known v0.4.0 limitation. | Marketing-grade output; required by Tier 4 rubric. |

### Phase C: operator-grade polish (sessions 117-121)

| Session | Scope |
|---|---|
| **117-D** | **SHIPPED** in session 121. Audit + fix for the session-120 deferred integration test. c2pa-rs source read at `crates/c2pa-0.80.0/src/crypto/cose/certificate_profile.rs` + `crypto/cose/verifier.rs:159` traced the failure to TWO issues rcgen's defaults don't handle: (1) `use_authority_key_identifier_extension: false` by default -- rcgen elides the AKI extension, c2pa-rs's `aki_good` flag stays false, `check_certificate_profile` rejects the cert with a generic "certificate is invalid" that doesn't name the missing AKI; (2) missing `Organization` (O) attribute in the subject DN -- c2pa-rs's COSE verifier calls `sign_cert.subject().iter_organization().last()` AFTER the signature itself has validated successfully, and a missing O turns into `MissingSigningCertificateChain` which gets folded into the generic "claim signature is not valid" with NULL signer. Fix: set `use_authority_key_identifier_extension = true` on leaf params + push `DnType::OrganizationName` on leaf DN. Integration test `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs` now ships with TWO test functions: `certkeyfiles_signer_source_yields_valid_c2pa_manifest` (rcgen-minted cert material) + `openssl_generated_certkeyfiles_also_yields_valid_manifest` (openssl-minted, skips cleanly if openssl not on PATH). The openssl recipe is byte-for-byte the same commands `examples/tier4-demos/demo-01.sh` runs when `LVQR_DEMO_C2PA=1` is set, so demo operators get guaranteed c2pa-rs-acceptable cert material. demo-01.sh extended: new LVQR_DEMO_C2PA opt-in, new C2PA_ARGS array appended to the `lvqr serve` command line, new verify probe in the summary block that curls `/playback/verify/live/demo` + prints `valid=<bool> state=<str> signer="<str>"`. Tier 4 item 4.3 coverage in the demo flipped from "no" to "yes, opt-in via LVQR_DEMO_C2PA=1". README strikes the session-120 "programmatic-only" bullet + replaces with "both signer paths covered by two integration tests". Default-gate test count 963 -> 965 (+2). | Completes the session-120 CLI wiring with real end-to-end coverage + rolls the proven recipe into the demo so operators can verify C2PA signing in under 30 seconds. |
| **117-C** | **SHIPPED** in session 120. CLI C2PA wiring. Six new flags (`--c2pa-signing-cert`, `--c2pa-signing-key`, `--c2pa-signing-alg`, `--c2pa-assertion-creator`, `--c2pa-trust-anchor`, `--c2pa-timestamp-authority`) on `lvqr serve` with matching `LVQR_C2PA_*` env-var fallbacks, feature-gated on the existing `c2pa` Cargo feature. New `C2paAlgArg` clap `ValueEnum` over the seven algorithm variants c2pa-rs supports. New `build_c2pa_config(&args)` helper constructs `Option<C2paConfig>` from the parsed args with `C2paSignerSource::CertKeyFiles` when both cert+key are set, reads the trust-anchor PEM contents eagerly so a missing file fails at CLI time, and returns an `Err(anyhow)` with a clear message when exactly one of cert/key is set. Closes the "C2PA signing is programmatic-only" gap carried over from session 117's Known Limitations. Test coverage: eight unit tests in `main.rs::c2pa_cli_tests` covering both-set / only-one-set / default-alg / alg-override / assertion-creator-override / TSA-override / missing-trust-anchor-file. README Known Limitations bullet rewritten (strikes "programmatic-only"; names the follow-up test-coverage gap instead). README CLI reference gains a dedicated "C2PA signing" block. Integration test attempted via rcgen-generated cert chain but reverted -- c2pa-rs's sign-time `verify_certificate_profile` check is stricter than the documented EKU + KU requirements (likely wants specific X.509 v3 extensions / validity bounds that rcgen does not emit by default); adding happy-path on-disk coverage is its own follow-up session with a pre-staged PEM fixture. | Completes the v1.0 CLI story: C2PA joins `--whisper-model`, `--wasm-filter`, `--transcode-rendition` as a first-class operator-opt-in surface. |
| 117 | **SHIPPED** in session 118. Session 119 added two operator-polish follow-ups riding on top: HTTP `Range: bytes=` on `/playback/file/*` (RFC 7233 single-range requests with 206/416/fall-through semantics + 10 unit tests on the range-spec parser + a 4th integration test function in `archive_dvr_read_e2e.rs`) and `.github/workflows/tier4-demos.yml` (Ubuntu runner, apt-install ffmpeg + GStreamer plugin set + libclang-dev, build `lvqr-cli --features full`, invoke `examples/tier4-demos/demo-01.sh`, upload artifact, `continue-on-error: true` initially). Original 118 delivery: authored `crates/lvqr-cli/tests/archive_dvr_read_e2e.rs` (+3 `#[tokio::test]` functions, ~500 LOC) targeting three scenarios not covered by the existing `rtmp_archive_e2e.rs` (which DOES already exercise the happy-path read shape + auth + traversal guard): (a) multi-keyframe scrub window arithmetic -- publishes five keyframes spaced 2 s apart, asserts `/playback/{broadcast}?from=&to=` halves obey `find_range`'s `[start_dts, end_dts)` overlap semantics, and verifies the half-window segment_seq union equals the full-window set; (b) live-DVR scrub while publisher still active -- holds the RTMP session open across two scan passes, asserts the admin handler does not block on the writer's redb exclusive lock and that `/playback/latest/*` advances during the live publish; (c) Content-Type assertions (`application/json` on range + latest; `application/octet-stream` + correct `Content-Length` on file route). All three tests pass on the default feature gate in ~1.6 s on macOS. Authored `.github/workflows/dash-conformance.yml` patterned on `hls-conformance.yml`: ubuntu-latest runner, `apt install ffmpeg gpac`, boot `lvqr serve --dash-port 8889`, 20 s ffmpeg synthetic RTMP publish, GPAC `MP4Box -dash-check` as primary validator + ffmpeg-as-client pull + ffprobe as always-on fallback, artifact upload with 14-day retention. `continue-on-error: true` initially (matches hls-conformance.yml's early-days posture); promotion to required check waits for first clean run on main. Authored `tracking/SESSION_118_BRIEFING.md` in-session per the PLAN's "author a briefing before opening source files when the row has a non-trivial design decision" rule; briefing called out the "read side has zero E2E" claim as stale and re-scoped the test file to uncovered scenarios. Design decisions baked in: (a) DASH-IF authoritative validator deferred -- its REST-API-only container does not match the one-shot validator shape used by every other workflow; follow-up row will wire the `dashif/conformance` container when we can afford the day of integration work; (b) helpers copy-pasted from `rtmp_archive_e2e.rs` rather than factored into `lvqr-test-utils` -- the duplication pattern is live across 6+ tests and factoring is a separate hygiene session; (c) no `Range: bytes=` header tests -- the file handler does not implement range requests today, that is a documented gap not a regression, adding range-request support is its own follow-up. | Closes the first phase-C row: DVR reliability + DASH egress conformance both guarded by dedicated tests in CI. |
| **118-B / 119-A** | **SHIPPED** in session 123. Two audit follow-throughs bundled: (a) Python admin client 3/9 -> 9/9 parity (mirrors the session-122 JS expansion; adds `mesh`, `slo`, `cluster_nodes`, `cluster_broadcasts`, `cluster_config`, `cluster_federation` methods + 9 new dataclasses in `bindings/python/python/lvqr/types.py` + an optional `bearer_token` kwarg on `LvqrClient.__init__`; pytest coverage grows from 8 to 21 tests + 0 regressions); (b) new `.github/workflows/feature-matrix.yml` with three dedicated jobs locking every feature-gated integration test target into CI (`c2pa` runs `c2pa_verify_e2e` + `c2pa_cli_flags_e2e` + `lvqr-archive --features c2pa`; `transcode` installs GStreamer + ffmpeg and runs `aac_opus_roundtrip` + `software_ladder` + `transcode_ladder_e2e` + `rtmp_whep_audio_e2e`; `whisper` installs libclang + cmake for bindgen and runs `whisper_basic` + unit tests -- the full `whisper_cli_e2e` stays `#[ignore]` because it needs a ~78 MB ggml model that a scheduled-workflow follow-up will cache). `continue-on-error: true` soft-fail initial posture. Closes session-121 audit Next Up #3 + the JS/Python SDK asymmetry bullet from session 122. | SDK admin coverage is now 9/9 on both published language targets; every feature-gated integration test target has a dedicated CI cell. |
| **118-A** | **SHIPPED** in session 122. Slice-A of PLAN row 118: `@lvqr/core` admin client grows from 3 of 9 `/api/v1/*` routes to 9/9 (adds `mesh()`, `slo()`, `clusterNodes()`, `clusterBroadcasts()`, `clusterConfig()`, `clusterFederation()` alongside the pre-existing `healthz()`, `stats()`, `listStreams()`); nine new TypeScript interfaces + one union type mirror the underlying Rust serde structs. New `LvqrAdminClientOptions.bearerToken` closes the admin-auth gap. Vitest smoke-test suite at `bindings/js/tests/sdk/admin-client.spec.ts` hits every admin method against a live `lvqr serve` (10 tests, 246 ms). New `.github/workflows/sdk-tests.yml` boots `lvqr serve --mesh-enabled --cluster-listen` as a background process, runs Vitest + pytest (existing 8 tests on `bindings/python/tests/test_client.py`), uploads lvqr log artifact. `continue-on-error: true` soft-fail initially per the dedicated-workflow convention. Workflow does NOT expand the Python admin client (still 3/9); that mirror is tracked as a follow-up row so the JS / Python asymmetry is visible. | SDK drift surfaces at PR time; operator tooling can now call every admin route without falling back to raw `fetch`. |
| 118 | SDK completion. Flesh out `@lvqr/core` admin client to cover all `/api/v1/*` routes (mesh, slo, cluster, federation). Add Vitest smoke tests. Add pytest invocation in CI. |
| 119 | Nightly 24h soak CI job (scheduled workflow; soft-fail for first week then hard). Enable whisper, cluster, transcode features in a CI matrix. |
| 120 | OAuth2 or JWKS dynamic key discovery. Closes README v1.1 checklist item. |
| **121** | **SHIPPED** in session 124. `--hmac-playback-secret` CLI flag + `LVQR_HMAC_PLAYBACK_SECRET` env activate a short-circuit auth path on every `/playback/*` handler: `?exp=<unix_ts>&sig=<base64url>` where `sig = HMAC-SHA256(secret, "<path>?exp=<ts>")`. Valid signature grants access without a bearer; tampered or expired returns 403 (not 401) so clients can distinguish missing auth from wrong auth. Pure public function `lvqr_cli::sign_playback_url(secret, path, exp)` generates the query suffix for operator-side URL minting. Signature covers path + exp only -- other query params (`track`, `from`, `to`, `token`) are NOT bound by sig, deliberately; the grant is broadcast-path-scoped but allows DVR scrub within the broadcast. New workspace deps: `hmac`, `sha2`, `subtle` (direct) pulled into lvqr-cli. `TestServerConfig::with_hmac_playback_secret` added for integration wiring. Three new `#[tokio::test]` functions in `crates/lvqr-cli/tests/playback_signed_url_e2e.rs`: `sign_playback_url_matches_hand_rolled_hmac` (unit), `signed_url_grants_access_and_denies_tampering` (integration, 4 scenarios + 3 bonus checks including cross-path tamper rejection), `signed_url_works_on_file_route` (file-route variant). Default-gate tests 965 -> 968. README Next Up #4 flipped to shipped, Auth+ops-polish checklist item flipped, CLI reference block gains the flag, `docs/auth.md` gains a new "Signed playback URLs" section documenting URL shape + operator helper + scope boundaries. | Closes PLAN row 121 + README v1.1 auth-polish checklist item. Narrow but real operator use case: "share a one-off DVR scrub link with someone who cannot authenticate". |

### Phase D: v1.1 marquee (sessions 122+)

| Session | Scope |
|---|---|
| 122-125 | Mesh data-plane completion. Client-side parent/child relay hardening, actual-vs-intended offload reporting, per-peer capacity advertisement, TURN deployment recipe, 3+ browser Playwright E2E. Flip `docs/mesh.md` to "IMPLEMENTED". |
| 126-129 | One hardware encoder backend. Pick based on deployment target: VideoToolbox for macOS dev, NVENC for Linux prod. Defer the other three to v1.2. |
| 130-132 | Stream-modifying WASM pipelines v2 with chaining. Documented v1.1 marquee feature. |
| 133+ | Tier 5 ecosystem: Helm chart, Kubernetes operator, Terraform module, docs site. |

## Items intentionally deferred to v1.2 or later

- Three of the four hardware encoder backends (only one ships in v1.1).
- MoQ frame-carried ingest-time propagation (110 scoping rejected it;
  Tier 5 client SDK push-back endpoint is the preferred path).
- Webhook auth provider (row on v1.1 checklist; lower leverage than OAuth2).
- SCTE-35 passthrough.
- Dedicated DVR scrub web UI.
- Hot config reload.
- SIP ingest (explicit anti-scope per ROADMAP).
- Room-composite egress (explicit anti-scope per ROADMAP).
- Live-signed C2PA streams (explicit anti-scope per TIER_4_PLAN).
- GPU WASM filters (explicit anti-scope per TIER_4_PLAN).

## Rationale for the reordering

### Why docs first

The README at head `11a5989` tells readers that `@lvqr/core` is "on the
roadmap". It was published to npm at v0.3.1. Anyone reading the README
today leaves with the wrong impression. Fix cost is 30 minutes; value is
unambiguous.

### Why security second, not first

The HLS and DASH live-route auth gap is real. Noop-provider deployments
(no auth configured) see no behavior change from a fix because the
provider passes everything through. The break surface is narrow:
deployments that explicitly set `--subscribe-token` or `--jwt-secret`
and rely on live HLS and DASH staying open. That intersection is likely
empty (operators who set subscribe-auth generally want subscriber auth
everywhere) and has an explicit escape hatch via
`--no-auth-live-playback`. Land the fix in the next release as
auth-on-by-default.

### Why WHEP audio third, not first

The candidate table in `SESSION_110_BRIEFING.md` ranks WHEP audio as row 4
with "medium risk" and ~2-3 sessions scope. In practice, the GStreamer
pipeline from session 106 already handles the decode side; the remaining
work is an Opus encoder on the output side and wiring into the WHEP
session's audio PT. Scope is closer to 1-2 sessions. User impact is huge:
every RTMP publisher to WebRTC subscriber workflow today gets video only.

### Why the Tier 5 SDK scope call in the briefing is wrong

The briefing estimates Tier 5 client SDK at ~20 sessions with "high" risk
and treats it as greenfield. Reality: `@lvqr/core` at 1143 LOC across six
TypeScript modules already ships a MoQ-Lite subscriber, WebTransport +
WebSocket transport detection, admin client, and a full `MeshPeer` client.
`@lvqr/player` at 316 LOC ships the `<lvqr-player>` web component.
`lvqr` on PyPI ships the admin client. `lvqr-core` on crates.io ships the
Rust side. The remaining work is **completion and CI** (expand admin
client coverage, add reconnect semantics, wire Vitest + pytest into CI,
end-to-end test against a running server), not authoring from scratch.
Realistic scope: 5-8 sessions across phase C and D.

### Why mesh data-plane completion is phase D, not phase B

The full mesh data-plane checklist in `README.md:251-278` has ten items.
Sessions 111-B and 115 close the first three (server-side subscriber
registration, signal `AssignParent`, client parent/child relay E2E).
The remaining seven (actual-vs-intended offload, per-peer capacity
advertisement, TURN recipe, 3+ browser Playwright) are a multi-session
commitment with real product decisions (NAT traversal strategy, capacity
measurement protocol, fallback semantics). Better to ship the first three
as a working slice, validate with real users, then commit to the rest.

## Success criteria for v1.1 exit

- Every item on the README roadmap checklist is either:
  (a) checked as shipped with a linked integration test, or
  (b) deliberately moved to a v1.2 anti-scope bucket with rationale.
- No "COMPLETE" claim in any handoff or docs file contradicts a silent
  gap in the code.
- Every Cargo feature has at least one CI job exercising it.
- WHEP can deliver audio from any AAC ingest.
- Live HLS and DASH routes enforce the same auth as `/ws/*` by default.
- `examples/tier4-demos/` has at least one scripted demo that runs.
- Mesh data-plane handles the happy path with two browser peers end to
  end.

## How to kick off the next session

Pick the top unshipped phase-A or phase-B row. Read this document and
the matching row's scope line. Author `tracking/SESSION_N_BRIEFING.md`
before opening any source file if the row has a non-trivial design
decision. Honor the absolute rules in `CLAUDE.md` (no Claude attribution,
no emojis, no em-dashes, 120-col max, real ingest and egress in
integration tests, only edit in-repo, no push without direct instruction).

## Revisit triggers

Revise this document if any of the following happens:
- A new audit surfaces additional "COMPLETE" gaps not listed here.
- An external deployment surfaces a priority this plan deranks too low.
- A phase takes significantly longer than the session estimate and
  threatens the phase-C exit window.
