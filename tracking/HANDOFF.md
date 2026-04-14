# LVQR Handoff Document

## Project Status: v0.4-dev -- WHEP video egress honest end-to-end

**Last Updated**: 2026-04-14 (session 22 close)
**Tests**: `cargo test --workspace` green under the default feature
set: 70 test binaries, 276+ individual tests passing, 1 doctest
marked `ignore` (a non-runnable doc example in
`lvqr-fragment/src/moq_sink.rs:39`), 0 failures. `cargo clippy
--workspace --all-targets -- -D warnings` clean. `cargo fmt --all
--check` clean.

## Session 22 (2026-04-14): str0m-backed WHEP end-to-end

Closed the entire WHEP media write arc in four commits on top of
session 19's baseline. By the end of the session the
RTMP -> WHEP -> WebRTC client path is real: ICE, DTLS, SRTP all
complete, video samples from the ingest bridge flow through
`Str0mSessionHandle::on_raw_sample` into the sans-IO poll loop,
get AVCC-to-Annex-B converted, handed to
`str0m::media::Writer::write` with the negotiated H.264 `Pt`, and
arrive as decoded `Event::MediaData` events on a str0m-based
client driven in-process by the E2E test.

### Commits (origin/main)

1. **580d152** -- `Str0mAnswerer` implementing `SdpAnswerer` behind
   the session-16 trait boundary; sans-IO poll loop per session
   spawned as a tokio task owning `Rtc` + `tokio::net::UdpSocket`;
   `oneshot` shutdown on handle Drop; ICE + DTLS completes
   against real browsers. `--whep-port` CLI flag (env
   `LVQR_WHEP_PORT`, default 0 = disabled) wired into
   `lvqr-cli::start` with `WhepServer` clone attached as
   `SharedRawSampleObserver` on the bridge builder before the
   `Arc` freeze. `TestServer` sets `whep_addr: None` so the 200+
   existing integration tests are untouched.
2. **db9fd10** -- `cargo fuzz` slot for `SdpOffer::from_sdp_string`
   (the first untrusted-input entry point on the WHEP POST path).
   Initial media-write design note with four load-bearing
   decisions. Decision 2 in that initial note was wrong and is
   corrected inline by commit ddcb599.
3. **ddcb599** -- Video media write via `str0m::media::Writer`.
   `SessionMsg::Video` pumped from `on_raw_sample` over an
   `mpsc::UnboundedSender`; `SessionCtx` captures `video_mid`
   (from `Event::MediaAdded`), `video_pt` (lazy via
   `Writer::payload_params` filtered on `Codec::H264`), and a
   `connected` flag (from `Event::Connected`). AVCC -> Annex B
   converter at the boundary because str0m's `H264Packetizer`
   scans for Annex B start codes and silently drops AVCC input;
   six unit tests cover single NAL, multi NAL, empty, truncated,
   overrun, and zero-length entries.
4. **ed9c6e3** -- In-process str0m loopback E2E test
   (`crates/lvqr-whep/tests/e2e_str0m_loopback.rs`) spinning up a
   client `Rtc` + `Str0mAnswerer` over loopback UDP, completing
   ICE + DTLS + SRTP in-process, pushing synthetic SPS + PPS +
   IDR samples into the server via `on_raw_sample`, and asserting
   `Event::Connected` + at least one `Event::MediaData` on the
   client side. Runs in ~0.15-0.18s wall time, 10/10 green on a
   local flakiness smoke run. Slot 4 (E2E) of the 5-artifact test
   contract for lvqr-whep is now closed. `tests/CONTRACT.md`
   updated to reflect proptest + fuzz + integration + E2E all
   shipped; only conformance (cross-impl against
   `simple-whep-client`) remains open.

### Important session-22 finding

The session-21 design note (`crates/lvqr-whep/docs/media-write.md`)
claimed str0m's `H264Packetizer` accepts AVCC passthrough. Reading
`src/packet/h264.rs` in step 1 of the execution plan revealed the
opposite: it scans for Annex B start codes via `next_ind`, and an
AVCC buffer has none, so the whole buffer gets handed to `emit`,
where the length-prefix high byte is read as a NAL header of type
0 and silently dropped. A naive build would complete ICE + DTLS +
SRTP and then emit zero packets with no error anywhere. The
boundary converter `avcc_to_annex_b` lives at
`crates/lvqr-whep/src/str0m_backend.rs`; the design note is
corrected in place.

### What is real after session 22

* **RTMP -> fMP4 -> MoQ -> browser**: real, unchanged.
* **RTMP -> CMAF -> LL-HLS -> browser**: real, unchanged.
* **RTMP -> RawSample -> `Str0mAnswerer` -> WebRTC client**: real
  end-to-end. The in-process E2E test exercises every byte of
  this path except the public internet and a real browser's
  H.264 decoder. A real browser connecting to `--whep-port`
  should see video decode once it negotiates a compatible H.264
  profile, though the real-browser leg is not yet automated in
  CI (gated on `simple-whep-client` packaging).

### What is still not real

* **Audio over WHEP**. RTMP ingest carries AAC, WHEP negotiated
  Opus. No in-tree AAC -> Opus transcoder. One-shot warn on first
  audio sample, then silent drop. Permanently deferred; see
  `crates/lvqr-whep/docs/media-write.md` for rationale.
* **Trickle ICE ingestion**. `Str0mSessionHandle::add_trickle`
  still one-shot warns and returns success. WHEP rarely needs
  trickle once the offer embeds every host candidate.
* **Real-browser E2E in CI**. Gated on packaging a WHEP client
  binary (`simple-whep-client` or a `webrtc-rs` thin wrapper)
  into the CI image.
* **CORS restrictive default** and **`lvqr-wasm` deletion**:
  still correctly deferred audit items.

### Recommended entry point (session 23)

1. **Real-browser E2E via `simple-whep-client`** soft-skip,
   landing slot 5 (conformance) of the test contract for
   lvqr-whep. Closes every slot of the 5-artifact contract for
   this crate. Requires the CI image to carry the binary.
2. **Tier 2.4 start: archive + redb index**. Gate for Tier 3.
3. **CORS restrictive default** (`crates/lvqr-cli/src/lib.rs`
   `CorsLayer::permissive()` replacement). One-commit breaking
   change with a release note.
4. **`lvqr-wasm` deletion**. One-commit mechanical removal.
5. **Keyframe request handling**. WebRTC PLI / FIR feedback from
   the client should eventually trigger an upstream keyframe
   request on the ingest side. Low priority until a real browser
   client surfaces the need.

## Session 19 (2026-04-14): audit sweep + README refresh

Session 19 is a bookkeeping + drift-closure session. No code
changes, one doc commit. The eight sessions before it had
accumulated enough state drift in the top-level `README.md` that
a new contributor reading the repo cold would get a materially
wrong picture of what ships. The audit sweep also re-verified the
tracked debt backlog so session 20 inherits an honest status
table rather than a stale "maybe closed" hint.

### Audit findings

A full sweep over the tree + the tracking docs surfaced one real
drift case and several already-closed items whose status was not
reflected in the docs:

1. **`README.md` was stale by approximately 40 test binaries and
   139+ individual tests**. The Status section still claimed "29
   test binaries workspace-wide, 130+ individual tests including
   2560 generated proptest cases" from what looks like a
   Tier 1-era snapshot. Reality at session 18 close is 69
   binaries and 269 tests. The feature list explicitly said "No
   HLS, LL-HLS, DASH, WHIP, WHEP, SRT, or RTSP egress or ingest
   yet", which is now false: LL-HLS with multi-broadcast routing
   and an audio rendition group has been on `main` since
   session 13, and the WHEP signaling router has been on `main`
   since session 16. The crate table omitted `lvqr-fragment`,
   `lvqr-cmaf`, `lvqr-hls`, `lvqr-codec`, `lvqr-moq`, and
   `lvqr-whep` entirely -- six of the seven Tier 2.x data-plane
   crates were invisible. The CLI reference omitted `--hls-port`
   (default `8888`). The architecture diagram showed only the
   original RTMP -> MoQ -> Browser path with no LL-HLS or WHEP
   fork. All four drift vectors addressed in this commit.

2. **`AUDIT-INTERNAL-2026-04-13.md` "Fix Plan for This Session"
   is 100% closed.** Session 17 closed the admin auth-failure
   metric; earlier sessions had silently closed the other four.
   The HANDOFF session 17 entry already lists this, but the
   top-of-file status line lacked a pointer.

3. **`AUDIT-INTERNAL` "Deferred" items**. Status re-verified
   against the current tree:

   | Item | Status |
   |---|---|
   | Delete `lvqr-core::{Registry, RingBuffer, GopCache}` | CLOSED (types gone from source; README fixed session 17) |
   | Delete `lvqr-wasm` | OPEN (scheduled for v0.5; crate is still marked `# DEPRECATED` in its own lib.rs header) |
   | Admin auth-failure metric | CLOSED (session 17) |
   | CORS restrict | OPEN. `CorsLayer::permissive()` at `crates/lvqr-cli/src/lib.rs:438`. Breaking change; scope as its own commit with a release note. |
   | Rate limits on every auth surface | OPEN (Tier 3) |
   | `lvqr-signal` input validation | CLOSED. `is_valid_peer_id`, `is_valid_track`, `MAX_PEER_ID_LEN`, `MAX_TRACK_LEN` all in `crates/lvqr-signal/src/signaling.rs`. |
   | `lvqr-record` integration test via event bus | CLOSED. `crates/lvqr-record/tests/record_integration.rs`. |
   | fMP4 `esds` multi-byte descriptor length | CLOSED. `write_mpeg4_descriptor` in `lvqr-ingest::remux::fmp4` uses the 4-byte variable-length encoding. |

4. **Test-count drift in the HANDOFF top-of-file status line**.
   Session 18's HANDOFF entry reported "269 individual tests";
   the actual accounting is "269 passing + 1 `ignore`d doctest"
   because `cargo test --workspace` shows `1 ignored` in a
   `lvqr-fragment` doc block marked `ignore` (a code example
   that intentionally does not compile). Cosmetic but worth
   being accurate. Top line refined here.

5. **`docs/architecture.md` + `docs/quickstart.md`**. Still
   stale per the `AUDIT-READINESS-2026-04-13.md` findings --
   architecture.md references `tokio::select!` (pre-Tier 0
   shape), quickstart.md references a `your-server:8080/watch/my-stream`
   URL that does not exist. `AUDIT-READINESS` deliberately gated
   these on a "Tier 5 docs site pass", so they stay out of scope
   for session 19. `README.md` is the authoritative public
   surface for now.

### What session 19 landed

One logical change, one commit, docs-only:

* **`README.md`**: Rewrote the Status section to reflect Tier 2.3
  closure (`lvqr-ingest` -> `lvqr-cmaf` -> `lvqr-hls` /
  `lvqr-whep`, real RTMP-with-audio E2E, retired hand-rolled
  video writer, WHEP signaling router shipped behind an
  `SdpAnswerer` trait boundary, audio timescale fix). Refreshed
  the test counts to the authoritative 69 binaries / 269 tests.
  Replaced the "No HLS, LL-HLS, DASH, WHIP, WHEP, SRT, or RTSP"
  line with an honest list of remaining limitations (no str0m
  backend yet, no DASH / WHIP / SRT / RTSP, mesh is topology
  only, CORS is still permissive, HEVC / AV1 / Opus surface
  untested in the full ingest path). Added `lvqr-fragment`,
  `lvqr-cmaf`, `lvqr-hls`, `lvqr-codec`, `lvqr-moq`, `lvqr-whep`
  to the crate table with one-line descriptions matching the
  actual crate surface. Updated the architecture diagram to
  fork from a single bridge output into four egress paths
  (MoQ / WebSocket fMP4 / LL-HLS / WHEP). Added `--hls-port`
  to the CLI reference. Added `tracking/HANDOFF.md` to the
  "Read before contributing" list as the canonical source of
  truth for current state, pointing new contributors at the
  session-by-session entries rather than the frozen three-audit
  snapshot.

* **`tracking/HANDOFF.md`** (this file): Top status line refined
  to distinguish passing tests from the `ignored` doctest.
  Added this session-19 section.

### Verification run (session 19)

* `cargo test --workspace` -- 69 binaries, 269 passing + 1
  ignored doctest, 0 failures.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.
* Audit-item status table above re-verified against the current
  tree via `grep` / `ls` / source reads, not by trusting earlier
  HANDOFF claims.

### Recommended entry point (session 20)

Unchanged from session 18's handoff. The code-work picking list:

1. **str0m-backed `Str0mAnswerer`**. Full session of its own;
   session 20 should start by reading str0m's crate docs in
   the cargo registry cache before writing any code. Expect
   offer -> answer to require binding a UDP socket for ICE
   candidates, and expect `Rtc::sdp_api().accept_offer` to need
   explicit media direction + codec configuration (H264 is not
   always default enabled in str0m).
2. **`--whep-addr` flag in `lvqr-cli`** + `RawSampleObserver`
   attachment on the bridge. Small follow-up once item 1 is
   real. Should not ship without item 1 because an `--whep-addr`
   flag that returns 501 on every POST is worse than no flag.
3. **Fuzz slot for the SDP offer parser**
   (`crates/lvqr-whep/fuzz/fuzz_targets/parse_offer_sdp.rs`).
   Lands naturally with item 1 because the fuzz corpus needs
   the real offer parser to target.

Optional low-risk cleanup items that do not require str0m:

* **Delete `lvqr-wasm`**. Scheduled for v0.5, crate is marked
  `# DEPRECATED`, no consumers. One-commit mechanical deletion
  + workspace Cargo.toml + CI `wasm` job removal.
* **CORS restrictive default**. Scope: replace
  `CorsLayer::permissive()` in `crates/lvqr-cli/src/lib.rs:438`
  with a tight default allowing only the configured admin
  origin plus localhost. Add a `--cors-allow-origin` flag for
  opt-in. Breaking change; ship with a release note.

## Session 18 (2026-04-14): fix LL-HLS audio partial duration reporting

Session 18 is a one-commit session (3058ee3) closing the cosmetic
follow-up that session 14 flagged: the LL-HLS audio playlist was
rendering `#EXT-X-PART:DURATION` values scaled by
48_000 / 44_100 ≈ 1.088 for 44.1 kHz AAC content because session
13 hardcoded `audio_config_from` to `timescale: 48_000`. For a
typical 1024-sample AAC-LC frame the playlist reported
`DURATION=0.021333` (1024 / 48000) instead of the correct
`DURATION=0.023220` (1024 / 44100). Routing and serving were
always correct -- only the rendered duration was wrong.

### What changed (six files touched)

1. **`crates/lvqr-cmaf/src/policy.rs`**. New
   `CmafPolicy::for_timescale(timescale: u32)` constructor that
   builds a policy scaled to any timescale using the standard
   LL-HLS targets (200 ms partials, 2 s segments).
   `VIDEO_90KHZ_DEFAULT` and `AUDIO_48KHZ_DEFAULT` are the
   specialised shapes this constructor returns for 90_000 Hz and
   48_000 Hz respectively -- both constants kept for source-level
   compatibility with the proptest / segmenter / coalescer test
   suites that already name them.

2. **`crates/lvqr-ingest/src/observer.rs`**. `FragmentObserver::on_init`
   signature grows a `timescale: u32` parameter carrying the
   track's native sample rate. Docstring explains why: downstream
   consumers need the real denominator to render wall-clock
   durations from tick counts. `NoopFragmentObserver` impl
   updated.

3. **`crates/lvqr-ingest/src/bridge.rs`**. Video on_init fire
   passes `90_000` (hardcoded because `video_init_segment_with_size`
   writes `mvhd.timescale = 90000` unconditionally). Audio on_init
   fire captures `config.sample_rate` into a local `audio_timescale`
   before the `AudioConfig` moves into `stream.audio_config`, then
   passes it through. No other bridge semantics change.

4. **`crates/lvqr-hls/src/server.rs`**.
   `MultiHlsServer::ensure_audio(broadcast, timescale)` now takes
   the audio track timescale as a second argument. The derived
   `audio_config_from(video, timescale)` swaps the hardcoded
   48_000 for the passed value so
   `PlaylistBuilderConfig::timescale` on the audio rendition
   reflects the real sample rate. The session-13 TODO comment on
   `audio_config_from` called for exactly this.

5. **`crates/lvqr-cli/src/hls.rs`**. `HlsFragmentBridge::on_init`
   builds the per-track `CmafPolicy` via
   `CmafPolicy::for_timescale(timescale)` and passes the same
   `timescale` into `ensure_audio`. `on_fragment`'s audio and
   video branches switch from `ensure_video` / `ensure_audio`
   (producer-side side-effects) to pure `self.multi.video()` /
   `self.multi.audio()` lookups that skip cleanly if the init
   has not landed yet -- a defensive branch since the FLV
   sequence header always arrives before any raw frame.

6. **`crates/lvqr-hls/tests/integration_master.rs`**. Session-13
   test updated to pass `48_000` as the audio timescale, matching
   the test's pre-existing `audio_chunk(..., 96_000, ...)`
   duration assumptions.

### Verification

`rtmp_hls_e2e` now prints the exact expected value in its audio
playlist body: `#EXT-X-PART:DURATION=0.023220` for both audio
partials (the test publishes two AAC frames at 44.1 kHz). That
is `1024 / 44100 = 0.0232199...` rounded to six decimals by the
playlist renderer's `{:.6}` format specifier. `cargo test
--workspace` passes 269 tests. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo fmt --all --check`
clean.

### Recommended entry point (session 19)

With the audio timescale follow-up closed, the remaining open
threads are unchanged from the session 17 handoff:

1. **str0m-backed `Str0mAnswerer`**. The full WHEP signaling →
   transport integration. Still a full session of its own.
2. **`--whep-addr` flag in `lvqr-cli`** + `RawSampleObserver`
   attachment. Small follow-up after item 1.
3. **Fuzz slot for the SDP offer parser**. Lands with item 1.

Session 18 did not touch any of these.

## Session 17 (2026-04-14): close deferred audit findings

Session 17 is a small bookkeeping session that closes two items
from the `AUDIT-INTERNAL-2026-04-13.md` deferred list which had
been tracked for multiple sessions without landing. One logical
commit (9f1c3e0), four files touched:

1. **`crates/lvqr-admin/Cargo.toml`** + **`crates/lvqr-admin/src/routes.rs`**.
   Added `metrics` as a dep and emits
   `lvqr_auth_failures_total{entry="admin"}` from the admin
   middleware on every `AuthDecision::Deny`. Before this commit,
   the admin surface was the only LVQR auth entry point that
   denied silently -- RTMP, MoQ, WS ingest, and WS subscribe all
   already emitted the same counter with different `entry`
   labels. Brute-force attempts against the admin token are now
   visible to Prometheus scrapers with the exact same query shape
   operators already use for the other entry points.

2. **`crates/lvqr-core/README.md`**. Replaced the stale crate
   overview which still documented `Registry`, `RingBuffer`, and
   `GopCache` as shipping API. Those types were deleted from the
   source tree at the Tier 2.1 fragment-model landing; the Rust
   lib.rs module doc already reflected the new reality but the
   README did not. The refreshed README lists the actual
   remaining surface (EventBus, RelayEvent, TrackName, Frame,
   RelayStats, CoreError) with a working usage example.

### Audit sweep verifying closed items

While scoping session 17 I re-verified every item on the
`AUDIT-INTERNAL-2026-04-13.md` "Deferred" list against the current
tree. The items tagged closed during earlier sessions without a
HANDOFF note are:

| Item | Status found in tree |
|---|---|
| Delete `lvqr-core::{Registry, RingBuffer, GopCache}` | CLOSED in the Rust sources already; only README drift remained. Fixed this session. |
| `lvqr-signal` input validation | CLOSED. `is_valid_peer_id`, `is_valid_track`, `MAX_PEER_ID_LEN`, `MAX_TRACK_LEN` all present in `crates/lvqr-signal/src/signaling.rs`. |
| `lvqr-record` integration test via event bus | CLOSED. `crates/lvqr-record/tests/record_integration.rs` exists. |
| fMP4 esds multi-byte descriptor length encoding | CLOSED. `write_mpeg4_descriptor` in `lvqr-ingest::remux::fmp4` uses the 4-byte variable-length encoding. |
| Admin auth-failure metric | CLOSED (this commit). |

Items still correctly deferred:

* **Delete `lvqr-wasm`**. Scheduled for v0.5 per the original
  audit. The crate is marked `# DEPRECATED` in its own `lib.rs`
  header and the browser client now lives in TypeScript. No
  consumers; deletion is safe but mechanical and can land
  whenever a session wants the bookkeeping.
* **CORS restrict in `lvqr-cli`**. `CorsLayer::permissive()` is
  still applied at `crates/lvqr-cli/src/lib.rs:438`. The audit
  recommended a restrictive default allowing only the configured
  admin origin plus localhost. Deferred from this session because
  changing the CORS default is potentially a breaking change for
  any existing browser client that depends on the wide-open
  policy; should land as its own commit with a matching release
  note, not piggybacked.
* **Rate limits on every auth surface**. Tier 3 hardening; a
  `tower::limit::RateLimit` layer on the admin router plus a
  per-IP accept budget on WS and MoQ. Still blocked on the full
  Tier 3 gate.

## Session 16 (2026-04-14): `lvqr-whep` signaling router + integration slot

Session 16 closed the second artifact slot of the 5-artifact
contract for `lvqr-whep` by landing the full HTTP signaling
surface. No `str0m` yet; the router talks to the WebRTC side
through a clean trait boundary (`SdpAnswerer` + `SessionHandle`)
so a concrete `str0m`-backed answerer drops in later as a single
type swap at construction time. One commit (3b1433b), three new
files plus `lib.rs` + `Cargo.toml` updates:

1. **`crates/lvqr-whep/src/server.rs`** (new). `SessionId` (32-char
   random hex via `rand::thread_rng().fill_bytes`), `WhepError`
   enum with four variants (`UnsupportedContentType`,
   `MalformedOffer`, `SessionNotFound`, `AnswererFailed`) and an
   `IntoResponse` impl mapping each onto 415 / 400 / 404 / 500,
   the `SdpAnswerer` and `SessionHandle` traits that form the
   plug point for a real WebRTC stack, `WhepServer` (cheap
   `Clone` around `Arc<WhepState>` so one instance lives in both
   the axum router and the ingest bridge's `RawSampleObserver`
   slot), and a `RawSampleObserver` impl that fans each upstream
   sample out to every session whose `broadcast` field matches.
   Three unit tests covering session-id entropy and error status
   mapping.

2. **`crates/lvqr-whep/src/router.rs`** (new). axum `Router` built
   on a `/whep/{*path}` catch-all with `post(handle_offer).patch(handle_trickle).delete(handle_terminate)`
   method routing. The catch-all exists because broadcast names
   follow the RTMP `{app}/{stream_key}` convention and therefore
   carry a `/` (e.g. `live/test`), and axum path parameters only
   match single URL segments. On POST, the captured `path` is
   the broadcast name verbatim; the handler mints a random
   `SessionId`, registers it in the state's `DashMap<SessionId,
   SessionEntry>`, and returns 201 Created with a `Location:
   /whep/{broadcast}/{session_id}` header plus the SDP answer
   body. On PATCH and DELETE the handler splits the captured
   path on the last `/` via a `split_session_path` helper to
   recover `(broadcast, session_id)`. POST content-type accepts
   `application/sdp` with or without parameters; PATCH also
   accepts `application/trickle-ice-sdpfrag` per the WHEP draft.

3. **`crates/lvqr-whep/src/lib.rs`**. Added `pub mod router; pub
   mod server;` plus re-exports for `router_for`, `SdpAnswerer`,
   `SessionHandle`, `SessionId`, `WhepError`, `WhepServer`.

4. **`crates/lvqr-whep/Cargo.toml`**. Runtime deps: `axum`,
   `dashmap`, `lvqr-cmaf`, `lvqr-ingest`, `rand`, `thiserror`,
   `tracing`. Dev-deps: `tokio` (features `macros`, `rt`) and
   `tower` for `ServiceExt::oneshot`.

5. **`crates/lvqr-whep/tests/integration_signaling.rs`** (new).
   The integration slot of the 5-artifact contract. Twelve tests
   driving the real axum router via `tower::ServiceExt::oneshot`
   with two stub answerers: `StubAnswerer` (shared atomic
   counters for trickle + sample call counts) and
   `TaggingAnswerer` (tags handles by broadcast so the fanout
   test can assert which broadcast saw each sample). Coverage:

   * `post_offer_returns_created_with_location_and_answer` --
     full happy path with 201 + Location header format assertion
     + `Content-Type: application/sdp` + SDP answer body + session
     count increment.
   * `post_offer_without_content_type_returns_415`
   * `post_offer_with_wrong_content_type_returns_415`
   * `post_offer_accepts_content_type_with_parameters` --
     `application/sdp; charset=utf-8` must be accepted.
   * `post_offer_with_empty_body_returns_400` -- `MalformedOffer`
     path; session must not be registered.
   * `delete_unknown_session_returns_404`
   * `session_lifecycle_post_then_delete` -- POST -> DELETE ->
     second-DELETE, asserts the session count roundtrips to zero
     and the second delete is 404.
   * `patch_unknown_session_returns_404`
   * `patch_existing_session_forwards_to_handle` -- PATCH body
     actually reaches `SessionHandle::add_trickle` via the
     shared atomic counter.
   * `patch_with_wrong_content_type_returns_415`
   * `raw_sample_observer_routes_only_to_subscribed_sessions` --
     subscribes one session per broadcast, pushes samples for
     `live/one` (2x), `live/two` (1x), and `live/three`
     (unsubscribed). Asserts per-broadcast counters land on
     2 / 1 / 0. This is the load-bearing correctness property for
     the fanout design.
   * `unknown_route_returns_404` -- actually asserts 405 since
     GET matches the catch-all path but no GET handler is
     registered. Test name is slightly stale; assertion is
     correct.

### Routing bug caught by the integration slot

Session 16's first-pass router used axum's
`/whep/{broadcast}` + `/whep/{broadcast}/{session_id}` two-route
shape, which compiled clean and passed clippy but failed 9 of 12
tests on first run with 405s. The bug: `{broadcast}` only matches
single URL segments, so `/whep/live/test` was matching the
two-segment `/whep/{broadcast}/{session_id}` route with
broadcast = `live` and session_id = `test`, leaving POST without
a handler (hence 405). Fixed by flipping to the `/whep/{*path}`
catch-all with manual splitting on the last `/` inside each
handler -- the exact pattern `lvqr-hls::MultiHlsServer::router`
already uses for the same problem. Without the integration slot
landing alongside the code, session 17 would have inherited a
dead router that returns 405 for every real client; the
integration slot paid for itself on its first CI run.

### Design decisions answered (session 16)

The session-11 `lvqr-whep` design note lists four open questions.
Session 16 answered them and the answers are baked into the
router and the trait boundary:

1. **Packetizer home**: private module `lvqr-whep::rtp`. Promote
   to a standalone `lvqr-rtp` crate later when `lvqr-whip` needs
   the inverse depacketizer. No speculative abstraction.
2. **Socket strategy**: one UDP socket per session for v0.x.
   Simpler control flow, no ICE-lite demux to write. Shared
   sockets are a perf-driven refactor later.
3. **WHEP bind**: `Option<SocketAddr>` under a future
   `--whep-addr` flag on `lvqr-cli`, default disabled. Users
   opt in during the v0.x cycle. The flag itself lands with the
   first concrete `SdpAnswerer` so the route stops returning
   "not yet implemented" bodies.
4. **Token transport**: `Authorization: Bearer <token>` on the
   offer POST. The WS surface's query-param and subprotocol
   fallbacks stay WS-specific. Not wired yet; the router already
   reads `HeaderMap` so plumbing `SharedAuth` through
   `WhepServer` is a small local diff in the CLI integration
   session.

### Contract slot status after session 16

`lvqr-whep` is now at **2 of 5 contract slots closed**:

| Slot | Status |
|---|---|
| proptest | CLOSED (session 15, `tests/proptest_packetizer.rs`) |
| integration | CLOSED (session 16, `tests/integration_signaling.rs`) |
| fuzz | OPEN (offer SDP parser lives in str0m; lands with str0m) |
| e2e | OPEN (`lvqr-cli/tests/rtmp_whep_e2e.rs` once str0m + webrtc-rs client subprocess is available) |
| conformance | OPEN (cross-implementation against `simple-whep-client`, not yet installed in CI) |

### Recommended entry point (session 18)

With session 16 closing signaling and session 17 closing the
audit debt, the next block of work is the str0m integration
itself. The picks for session 18:

1. **Bring in `str0m` as a workspace dep and implement
   `Str0mAnswerer`**. Replace the stub answerers in
   integration tests with a real one for at least the offer ->
   answer path. Expect the first implementation to need UDP
   socket binding at construction time (str0m needs a local
   host ICE candidate to include in the answer) and session-
   scoped state that stores the `Rtc` state machine. Leave
   `add_trickle` and `on_raw_sample` as TODO with tracing
   warnings -- driving the `Rtc` state machine forward and
   packetizing samples into RTP is a separate follow-up.
2. **Wire `--whep-addr` in `lvqr-cli::ServeArgs`** and
   construct the `WhepServer` with `Str0mAnswerer` in
   `lvqr_cli::start`, attach it to the bridge via
   `RtmpMoqBridge::with_raw_sample_observer`, and mount the
   router on the configured binding. Small follow-up once item
   1 is real.
3. **Fuzz slot for the SDP offer parser** under
   `crates/lvqr-whep/fuzz/fuzz_targets/parse_offer_sdp.rs`
   seeded from captured browser offers. Lands naturally
   alongside item 1.

Item 1 is the full session. Items 2 and 3 are follow-ups that
assume str0m is actually producing answers. E2E (`rtmp_whep_e2e.rs`)
and conformance (`simple-whep-client` soft-skip) slots are
session 19 or later.

### Audio timescale follow-up (tracked from session 14)

Session 14 flagged a cosmetic bug where `HlsFragmentBridge`
pushes audio chunks through the `AUDIO_48KHZ_DEFAULT` policy
while the bridge itself emits audio at the AAC sample rate
(44100 Hz via `audio_init_segment` + `audio_segment`), so the
`#EXT-X-PART:DURATION` values reported in the LL-HLS audio
playlist are scaled by 48000 / 44100 (a 1024-sample AAC frame
reports 0.021333 s instead of the true 0.023220 s). Still open.
Candidate fix: either retire `audio_segment` alongside the
session-14 `video_segment` deletion by routing AAC through
`lvqr_cmaf::build_moof_mdat` with a proper audio-timescale
policy, or have the bridge construct a per-broadcast
`CmafPolicy` with the right timescale at init time. Routing
and serving are correct today; only the reported duration is
off. Not blocking WHEP work.

## Session 15 (2026-04-14): begin `lvqr-whep` implementation

Session 15 closed item 2 from the session-14 entry-point list by
starting the WHEP egress implementation. No networking yet; this
session lands the two pieces that have no dependency on `str0m` or
axum so future sessions can iterate on signaling against a stable
packetizer. Three files added, three files touched:

1. **`crates/lvqr-ingest/src/observer.rs`**. New `RawSampleObserver`
   sibling trait alongside the existing `FragmentObserver`, plus
   `SharedRawSampleObserver = Arc<dyn RawSampleObserver>` and
   `NoopRawSampleObserver`. The observer takes a
   `&lvqr_cmaf::RawSample` and is fired from the bridge's video and
   audio callback paths **before** the sample is muxed into an
   fMP4 fragment. Consumers that need per-NAL AVCC or raw AAC bytes
   subscribe here instead of re-parsing `CmafChunk` mdat bodies
   downstream. The dep on `lvqr-cmaf` was already normal-dep via
   session 14's deletion of the hand-rolled writer, so importing
   `RawSample` into observer.rs is free.

2. **`crates/lvqr-ingest/src/bridge.rs`**. `RtmpMoqBridge` gained a
   `raw_observer: Option<SharedRawSampleObserver>` field plus
   `with_raw_sample_observer` / `set_raw_sample_observer` builder
   methods matching the existing `FragmentObserver` builders. In
   the video callback, the already-constructed `lvqr_cmaf::RawSample`
   (pre-`build_moof_mdat`) is handed to the observer as
   `(broadcast, "0.mp4", &sample)`. In the audio callback, a fresh
   `RawSample { track_id: 2, payload: aac_data.clone(), keyframe:
   true, ... }` is built for the observer only (the existing
   `audio_segment` mux path is unchanged); `aac_data.clone()` is a
   `Bytes` refcount bump, not an allocation.

3. **New crate `lvqr-whep`**. Registered as a workspace member in
   `Cargo.toml` and exposed through the standard
   `lvqr-whep = { version = "0.3.1", path = "crates/lvqr-whep" }`
   workspace-dep entry. The crate ships:
   * `Cargo.toml` with `bytes` as the only runtime dep and
     `proptest` as a dev-dep. No `str0m`, no `axum`, no `tokio`
     yet -- those land with the networking layer.
   * `src/lib.rs` -- module-level doc note pointing at
     `crates/lvqr-whep/docs/design.md` plus re-exports of
     `H264Packetizer` and `H264RtpPayload` from the new `rtp`
     module. STAP-A aggregation is explicitly called out as a v0.x
     non-goal.
   * `src/rtp.rs` -- stateless `H264Packetizer { mtu }` that walks
     AVCC length-prefixed NAL sequences and emits RFC 6184 RTP
     payloads (the bytes placed after the RTP fixed header; the
     sender writes the header itself). Single-NAL-unit mode (§5.6)
     for NALs that fit within the MTU budget; FU-A fragmentation
     (§5.8) for oversized NALs with correct Start / End bit
     handling across fragments and `is_start_of_frame` /
     `is_end_of_frame` flags tracked across multi-NAL inputs so a
     sender can map end-of-frame onto the RTP marker bit. The MTU
     is clamped to a minimum of `FU_HEADER_SIZE + 1` so a
     single-byte fragment is always representable; the default is
     `DEFAULT_MTU = 1200` to match the `str0m` / Pion / libwebrtc
     safe Ethernet budget.
   * `split_avcc` helper that walks `[u32-be length][body]` tuples
     and skips malformed entries silently: truncated length
     prefixes, zero-length bodies, and length fields that overrun
     the buffer all stop the walker cleanly without panicking. The
     proptest slot below pins the never-panic property.
   * `tests/proptest_packetizer.rs` -- the proptest slot of the
     5-artifact contract. Four properties: (1) `packetize` never
     panics on arbitrary bytes with arbitrary MTUs, (2) on
     well-formed AVCC input every payload respects the MTU budget
     and the start-of-frame / end-of-frame flags land on the first
     and last packet only, (3) FU-A fragments round-trip back to
     the original NAL body byte-for-byte after header
     reconstruction, (4) single-NAL-unit mode emits a single
     verbatim payload. 512 cases per property plus a persisted
     regression file under
     `tests/proptest_packetizer.proptest-regressions` pinning the
     one degenerate case proptest found during initial development
     (length-1 output slicing into `out[1..0]`).

### Design decisions answered

The session-11 design note at `crates/lvqr-whep/docs/design.md`
lists four open questions. Session 15 picks:

1. **Packetizer home**: private module `lvqr-whep::rtp`. Promote to
   a standalone `lvqr-rtp` crate later when `lvqr-whip` needs the
   inverse (depacketizer). No speculative abstraction.
2. **Socket strategy**: one UDP socket per session for v0.x.
   Simpler control flow over `str0m`'s sans-IO state machine, no
   ICE-lite demux to write. Shared sockets become a perf-driven
   refactor later.
3. **WHEP bind**: new `--whep-addr` flag mirroring `--hls-port`,
   default disabled (`Option<SocketAddr>`). Users opt in during the
   v0.x cycle. Wiring the flag is deferred to the networking
   session; session 15 does not touch `lvqr-cli`.
4. **Token transport**: `Authorization: Bearer <token>` on the
   offer POST only. The WS surface's query-param + subprotocol
   fallbacks stay WS-specific; WHEP takes the standards-track
   header path.

### Verification run

* `cargo test --workspace` -- 68 binaries, 254 individual tests,
  0 failures. New binary: `lvqr-whep::tests::proptest_packetizer`
  (4 proptest cases). The `lvqr-whep::rtp::tests` unit block adds
  8 tests to the `lvqr-whep` lib binary.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.

### What session 15 did NOT land

* **No networking**. No `str0m` dep, no SDP offer/answer parser,
  no axum router, no `WhepServer` state, no UDP socket task, no
  ICE / DTLS handshake wiring, no `--whep-addr` CLI flag. The
  signaling layer lands in the next session once the packetizer is
  proven as a building block.
* **Fuzz / integration / e2e / conformance slots**. Four of the
  five contract slots for `lvqr-whep` are still open. They land
  alongside the signaling layer so each closed slot has something
  real to exercise. `crates/lvqr-whep/docs/design.md` §5 has the
  full plan.
* **HEVC / AV1 packetizer**. AVC-only for the first WHEP release,
  matching the design-note non-goals.
* **RawSampleObserver wiring in `lvqr-cli`**. The trait is
  registered and the bridge fires it, but no consumer is attached
  yet. A future WHEP server constructor calls
  `RtmpMoqBridge::with_raw_sample_observer` to subscribe.
* **Audio byte-sharing**. The raw-sample observer sees the AAC
  access unit and the HLS path sees the same access unit re-muxed
  into an fMP4 fragment; the two views are reference-counted
  `Bytes` clones (cheap), not literally the same buffer. Not a
  problem in v0.x; flagged here in case a future session tries
  to share a single allocation across both paths.

### Recommended entry point (session 16)

The session-14 entry-point list is now closed: item 1 (deletion)
and item 3 (audio E2E) landed in session 14; item 2 (WHEP start)
landed in session 15. The natural session-16 picks are:

1. **Bring up the WHEP signaling layer**. Add `str0m` as a
   workspace dep, land `lvqr-whep::server::WhepServer` as an
   `Arc<WhepState>` wrapping `DashMap<SessionId,
   ActiveSubscriber>` and a handle to the `RawSample` tap, mount
   an axum router under `/whep/{broadcast}` with the POST /
   PATCH / DELETE handlers the design note specifies, and land
   the integration slot
   (`crates/lvqr-whep/tests/integration_signaling.rs`) via
   `tower::ServiceExt::oneshot` against a synthetic SDP offer.
   This is the entire session.
2. **Wire `--whep-addr` in `lvqr-cli` and attach the
   `RawSampleObserver`**. Small second commit once item 1 lands:
   add the flag to `ServeArgs`, construct the `WhepServer` in
   `lvqr_cli::start` when the flag is set, pass it as a
   `RawSampleObserver` into `RtmpMoqBridge::with_raw_sample_observer`,
   and mount the router on the configured axum binding.
3. **Fuzz slot for the offer SDP parser**. Can land in the same
   session as item 1 or immediately after. Seeds from the
   webrtc-rs / Pion offer fixtures plus captured Chrome devtools
   offers.

Item 1 is the full session. Items 2 and 3 are follow-ups that
assume item 1 landed first. E2E and conformance slots
(`rtmp_whep_e2e.rs` + cross-implementation test against
`simple-whep-client`) are session 17 or later: the E2E slot
requires a working webrtc-rs client dep and the conformance slot
needs `simple-whep-client` installed in CI, which today is not.

### AUDIT-INTERNAL-2026-04-13 "Fix Plan for This Session" status

All five items verified closed on main as of session 15:

| Item | Status |
|---|---|
| 1. Validator for broadcast names in `lvqr-relay::parse_url_token` | **CLOSED** (`is_valid_broadcast_name` + unit tests in `server.rs`) |
| 2. Defensive old-parent cleanup in `lvqr-mesh::reassign_peer` | **CLOSED** (live rebalance path retains old-parent children list correctly) |
| 3. `--jwt-secret` CLI flag wired to `JwtAuthProvider` | **CLOSED** (`lvqr-cli::main::ServeArgs` + integration test in `crates/lvqr-cli/tests/auth_integration.rs`) |
| 4. `lvqr-mesh/src/lib.rs` topology-planner disclaimer comment | **CLOSED** (lines 1-19) |
| 5. Heartbeat theatrical test | **CLOSED** (`heartbeat_keeps_peer_alive` uses a real 1 s timeout) |

Tracked-for-later items (still correctly deferred):

* Delete `lvqr-core::{Registry, RingBuffer, GopCache}` dead code.
  Needs verification that nothing in the Tier 2.3 data plane started
  consuming them transitively. Low-risk cleanup; session 16 or 17.
* Delete `lvqr-wasm`. Scheduled for v0.5.
* Admin auth-failure metric / CORS restrict / rate limits. Tier 3.
* `lvqr-signal` peer_id input validation. Tier 2 hardening, can
  land opportunistically; the scope is one `validate_peer_id`
  helper enforcing `^[A-Za-z0-9_-]{1,64}$` plus a 1-cap on
  registrations per connection.
* `lvqr-record` integration test via EventBus. Tier 1 follow-up;
  non-trivial because the WS ingest handler is private in the
  binary crate.

## Session 14 (2026-04-14): delete hand-rolled fMP4 writer + RTMP audio E2E

Session 14 closed items 1 and 3 from the session-13 entry-point
list. Two logical landings in one commit (6d86214):

### Item 1: retire `lvqr_ingest::remux::fmp4::video_segment`

The hand-rolled video media-segment writer, its `VideoSample`
adapter, its `build_video_segment` dispatch wrapper, and all its
surrounding test + feature-flag scaffolding are gone. The
`cmaf-writer` feature was default-on for a full release cycle
(sessions 12.2 -> 13), the parity gate caught every drift in the
transition, and the legacy path can be removed without risk.

Files touched:

* **`crates/lvqr-ingest/src/remux/fmp4.rs`**. Deleted: the
  `VideoSample` struct, `video_segment` (the ~80-line hand-rolled
  writer), `build_video_segment` (the feature-flag dispatch
  wrapper), `build_video_segment_via_cmaf` (the cmaf-writer
  branch), and the four unit tests (`video_segment_structure`,
  `video_segment_data_offset_correct`, `video_segment_multiple_samples`,
  `empty_samples_returns_empty`). Kept: `video_init_segment`,
  `video_init_segment_with_size`, `audio_init_segment`,
  `audio_segment`, `patch_trun_data_offset` (used by
  `audio_segment`), `write_mpeg4_descriptor`, all the box-writing
  helpers. The audio path is untouched; the handoff directive
  explicitly carved out `audio_segment` as unrelated.

* **`crates/lvqr-ingest/src/remux/mod.rs`**. Re-export list
  pruned to `audio_init_segment, audio_segment,
  video_init_segment, video_init_segment_with_size`.

* **`crates/lvqr-ingest/src/bridge.rs`**. Video callback now
  constructs `lvqr_cmaf::RawSample { track_id: 1, dts: base_dts,
  cts_offset: cts * 90, duration: duration_ticks, payload: nalu_data,
  keyframe }` and calls `lvqr_cmaf::build_moof_mdat(stream.video_seq,
  1, base_dts, &[sample])` directly. No dispatch wrapper.

* **`crates/lvqr-ingest/Cargo.toml`**. `default = ["rtmp"]`,
  `cmaf-writer` feature removed, `legacy-fmp4` marker feature
  removed, `lvqr-cmaf` flipped from optional dev-dep to normal
  dep, `mp4-atom` dev-dep removed (was only used by the parity
  gate).

* **`crates/lvqr-cli/Cargo.toml`**. `lvqr-ingest` reverts from
  the inline path dep (session 12.2's default-features escape
  hatch) back to workspace inheritance. `cmaf-writer` forward
  flag and the `full` feature definition both drop to
  `["rtmp", "quinn-transport"]`.

* **`crates/lvqr-cli/src/lib.rs`**. WS ingest handler's two
  `remux::build_video_segment` call sites (keyframe branch +
  delta branch) swapped for direct `lvqr_cmaf::RawSample` +
  `lvqr_cmaf::build_moof_mdat` construction.

* **Deleted**: `crates/lvqr-ingest/tests/parity_avc_init.rs`
  (205 lines), `crates/lvqr-ingest/tests/parity_avc_segment.rs`
  (220 lines), the fixture
  `crates/lvqr-ingest/tests/fixtures/golden/video_segment_keyframe.mp4`.

* **Pruned**: `crates/lvqr-ingest/tests/golden_fmp4.rs` dropped
  the `video_keyframe_segment_matches_golden` test and rewrote
  `ffprobe_accepts_concatenated_cmaf` to feed the init segment
  plus a `lvqr_cmaf::build_moof_mdat`-produced media segment to
  ffprobe. The audio conformance test
  (`ffprobe_accepts_audio_init_and_frame`) is unchanged; it
  still exercises the AAC path end-to-end.

* **Pruned**: `crates/lvqr-ingest/tests/proptest_parsers.rs`
  dropped the `video_segment_is_well_formed` proptest target and
  the `video_sample_strategy` helper. The `video_init_segment_is_well_formed`
  proptest is unchanged and still pins the init writer's
  structural invariants.

* **`.github/workflows/ci.yml`**. The `test-legacy-fmp4-path`
  job is deleted wholesale. The main `test` matrix is now the
  only test pipeline; there is no second feature-flag axis to
  maintain.

`cargo tree -p lvqr-cli -e normal` before and after confirms the
dep graph stays sound: `lvqr-ingest` still reaches `lvqr-cmaf` as
a normal dep, and `lvqr-cli` keeps its own direct dep on
`lvqr-cmaf` for the WS ingest fallback.

### Item 3: real RTMP-publish-with-audio E2E

`crates/lvqr-cli/tests/rtmp_hls_e2e.rs` gained two FLV audio
helpers (`flv_audio_seq_header`, `flv_audio_raw`) and a new test
`rtmp_publish_with_audio_reaches_master_playlist` that:

1. Spins up a `TestServer` with HLS enabled.
2. Publishes a single broadcast (`live/av`) via real `rml_rtmp`:
   video seq header, AAC seq header (AAC-LC 44100 stereo), first
   keyframe at t=0, raw AAC frame at t=0, second keyframe at
   t=2100 ms, second raw AAC frame at t=2100 ms.
3. Fetches `/hls/live/av/master.m3u8` and asserts `#EXTM3U`,
   `#EXT-X-MEDIA:` with `TYPE=AUDIO`, `#EXT-X-STREAM-INF` with
   `AUDIO="audio"`.
4. Fetches `/hls/live/av/audio.m3u8` and asserts `#EXTM3U` plus
   `#EXT-X-MAP:URI="audio-init.mp4"`.
5. Fetches `/hls/live/av/audio-init.mp4` and asserts the body
   starts with `ftyp`.
6. Fetches `/hls/live/av/playlist.m3u8` and asserts the video
   playlist still references `init.mp4`.

Passed first run. Closes the session-13 gap where the audio
bridge was only exercised through a router oneshot
(`integration_master.rs`), not through a real RTMP publish.

### Known cosmetic issue flagged for follow-up

The bridge's audio path writes the media segment at the AAC
sample rate (44100 Hz via `audio_init_segment` + `audio_segment`)
but `HlsFragmentBridge` pushes the chunk through the
`AUDIO_48KHZ_DEFAULT` policy, so the emitted `#EXT-X-PART:DURATION`
values are scaled by 48000 / 44100. The test output shows
`DURATION=0.021333` for a 1024-sample AAC frame, which is
`1024 / 48000` rather than the true `1024 / 44100`. Cosmetic
only: routing and serving are correct, only the reported
duration is off. A future session should either pick the audio
policy from the actual sample rate or retire `audio_segment`
alongside the video writer by routing AAC through
`lvqr_cmaf::build_moof_mdat` with an audio-timescale policy.
Tracked here so the next session catches it.

### Verification run (session 14)

* `cargo test --workspace` -- 67 binaries, 0 failures.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.
* `cargo tree -p lvqr-cli -e normal` confirms `lvqr-cmaf`
  reachable through both `lvqr-cli` directly and via
  `lvqr-ingest`.

## Session 13 (2026-04-13): audio rendition group + master playlist

Session 13 closed item 2 from the session-11 work list: audio
rendition group in HLS, including the master-playlist generation
that was its prerequisite. Five files touched (one new):

1. **`crates/lvqr-hls/src/master.rs`** (new). Pure-library
   `MasterPlaylist` + `VariantStream` + `MediaRendition` +
   `MediaRenditionType` types and a `render()` method that emits a
   minimal HLS multivariant playlist: `#EXTM3U`, `#EXT-X-VERSION:9`,
   `#EXT-X-INDEPENDENT-SEGMENTS`, one `#EXT-X-MEDIA` per rendition,
   and one `#EXT-X-STREAM-INF` (with optional `RESOLUTION` and
   `AUDIO=` attributes) followed by the variant URI per variant.
   Six unit tests cover the empty case, the single-rendition audio
   case, language attribute presence/absence, and variant lines
   without an audio group or a resolution. Exported from
   `lvqr_hls::lib` alongside the existing media-playlist exports.

2. **`crates/lvqr-hls/src/server.rs`**. `MultiHlsServer` was
   single-rendition per broadcast (one `HlsServer` per broadcast
   key); session 13 turned the inner map into `HashMap<String,
   BroadcastEntry>` where `BroadcastEntry { video: HlsServer,
   audio: Option<HlsServer> }`. The session-12 `ensure_broadcast` /
   `get_broadcast` API renamed to `ensure_video` / `video`; new
   `ensure_audio` / `audio` accessors create the audio rendition on
   demand using a derived `audio_config_from(template)` that swaps
   the timescale to 48 kHz, the `map_uri` to `audio-init.mp4`, and
   the `uri_prefix` to `audio-` so audio chunks never collide with
   video chunks in either the cache or the wire. The
   `/hls/{*path}` catch-all dispatch now matches `master.m3u8`
   (synthesizes a master playlist, including the audio rendition
   declaration when the broadcast has called `ensure_audio`),
   `audio.m3u8` and `audio-init.mp4` (audio HlsServer's playlist
   and init), URIs prefixed `audio-` (audio HlsServer's chunk
   cache), and falls through to the video HlsServer for everything
   else. The session-12 video routes (`playlist.m3u8`, `init.mp4`,
   chunk URIs) are unchanged. Unknown broadcasts and unknown audio
   renditions return 404 instead of empty 200s. The session-12
   `rtmp_hls_e2e.rs` test still passes against the renamed API.

3. **`crates/lvqr-cli/src/hls.rs`**. `HlsFragmentBridge` now keeps
   two policy state maps (video keyed by broadcast, audio keyed by
   broadcast) and dispatches fragments by track id: video samples
   (`0.mp4`) go to `multi.ensure_video(broadcast)` with the
   `VIDEO_90KHZ_DEFAULT` policy, audio samples (`1.mp4`) go to
   `multi.ensure_audio(broadcast)` with the `AUDIO_48KHZ_DEFAULT`
   policy. Tracks other than `0.mp4` and `1.mp4` are still
   ignored. The `dispatch_init` / `dispatch_chunk` /
   `classify` / `reset` helpers are factored out so the video and
   audio code paths share the same Tokio task-spawning shape.

4. **`crates/lvqr-hls/tests/integration_master.rs`** (new). Three
   integration tests driving `MultiHlsServer::router` via
   `tower::ServiceExt::oneshot`:

   * `master_playlist_includes_audio_rendition_when_both_tracks_present`:
     pushes a video init + segment chunk and an audio init +
     segment chunk into a `live/test` broadcast, fetches
     `/hls/live/test/master.m3u8` and asserts it contains the
     audio `EXT-X-MEDIA` line, an `EXT-X-STREAM-INF` line with
     `AUDIO="audio"`, and the variant URI on the next line. Also
     fetches `/hls/live/test/playlist.m3u8`, `/hls/live/test/audio.m3u8`,
     `/hls/live/test/init.mp4`, and `/hls/live/test/audio-init.mp4`
     and asserts each is served correctly with the right body and
     `Content-Type`.
   * `master_playlist_omits_audio_when_only_video_has_published`:
     same flow but only video is published. Master playlist must
     not contain `EXT-X-MEDIA` or `AUDIO=`. The audio playlist
     and audio init both 404.
   * `master_playlist_returns_404_for_unknown_broadcast`: the
     happy-path 404 case for a broadcast that has no renditions
     at all.

5. **`crates/lvqr-hls/src/lib.rs`**. New `master` module exported
   alongside the existing `manifest` and `server` modules.

### Verification run

* `cargo test --workspace` -- 67 binaries, 251 individual tests,
  0 failures. The new binary is `integration_master` (3 tests);
  the other 6 new tests are the `master::tests` unit tests in
  `lvqr-hls`'s lib binary.
* `cargo test -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --test rtmp_hls_e2e --test rtmp_ws_e2e` --
  both legacy-path E2E tests still green.
* `cargo clippy --workspace --all-targets -- -D warnings` clean
  under default.
* `cargo clippy -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --all-targets -- -D warnings` clean under
  the legacy fMP4 path.
* `cargo fmt --all --check` clean.

### What session 13 did NOT land

* **Audio in the RTMP-driven E2E test**. The
  `rtmp_hls_e2e.rs` test still publishes video only. Extending
  it to publish FLV audio requires AAC sequence-header plumbing
  and an audio-aware FLV fixture. The audio bridge code path is
  exercised by the new `integration_master.rs` test through the
  `MultiHlsServer` API directly. A full RTMP-publish-with-audio
  E2E lands in a later session, ideally bundled with the
  `rtmp_ws_e2e.rs` audio extension that the WS handler already
  expects.
* **Real bandwidth and resolution in the master playlist**.
  Session 13 emits a hardcoded `BANDWIDTH=2500000`,
  `CODECS="avc1.640020,mp4a.40.2"`, no `RESOLUTION`. Real values
  will come from the producer-side catalog once the codec
  parsers feed back into the bridge -- tracked as part of the
  Tier 2.2 codec wiring.
* **Per-rendition mediastreamvalidator coverage**. The
  `lvqr-hls` conformance slot still soft-skips when Apple's
  `mediastreamvalidator` is not on PATH. Adding a master-playlist
  conformance test is a follow-up once the validator is
  installed in CI.
* **Deletion of the hand-rolled fMP4 writer**. Session 12.2 set
  the `legacy-fmp4` marker and flipped the default; deletion is
  still a session 14 candidate (the cmaf-writer matrix has now
  been green on main for one session of additions on top, which
  is the soonest a "release cycle" can be argued).

### Recommended entry point (session 14)

The session-11 work list is now closed apart from item 3 (WHEP).
The natural session-14 picks:

1. **Delete the hand-rolled fMP4 writer behind `legacy-fmp4`**.
   Removes `lvqr_ingest::remux::fmp4::video_segment` plus its
   unit tests, the golden tests at `tests/golden_fmp4.rs` that
   reference it, the proptest target at
   `tests/proptest_parsers.rs::video_segment_is_well_formed`,
   the parity tests at `tests/parity_avc_segment.rs` +
   `tests/parity_avc_init.rs`, the `legacy-fmp4` feature on
   `lvqr-ingest/Cargo.toml`, and the `test-legacy-fmp4-path`
   CI job. Mechanical, half-session.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`. Full session by itself.
3. **Real RTMP-publish-with-audio E2E**. Extend
   `rtmp_hls_e2e.rs` and `rtmp_ws_e2e.rs` to publish FLV audio
   alongside the existing video sequence so the audio path
   through the bridge gets covered end-to-end. Half-session
   bundled with item 1.

My recommendation: pair **(1) deletion + (3) audio E2E**. Both
are mechanical, both compound the session 12 + 12.2 + 13 work
into a coherent "Tier 2.3 closed" milestone. Item 2 (WHEP) is
the right pick for session 15 once Tier 2.3 is fully closed.

## Session 12.2 (2026-04-13): `cmaf-writer` default-on + `legacy-fmp4` marker

Session 12.2 closed item 4 from the session-11 work list: flip
`lvqr-ingest`'s `cmaf-writer` feature to default-on and move the
in-crate hand-rolled fMP4 writer into retirement under a
`legacy-fmp4` marker feature. No code was gated out this session;
the retirement is a bookkeeping + dispatch flip, and a CI job
continues to exercise the hand-rolled path end-to-end until it is
deleted in a later session. Four files touched:

1. **`crates/lvqr-ingest/Cargo.toml`**. `default = ["rtmp"]` ->
   `default = ["rtmp", "cmaf-writer"]`. Added `legacy-fmp4 = []`
   as a marker feature. `cmaf-writer`'s docstring updated to
   reflect the new default state; `legacy-fmp4`'s docstring
   names the code slated for deletion in the next session and
   points at the CI matrix job that still exercises it.

2. **`crates/lvqr-cli/Cargo.toml`**. Added `cmaf-writer` to
   `default` and `full`. Changed the `lvqr-ingest` dep from
   `workspace = true` to an inline path dep with
   `default-features = false` so the `--no-default-features
   --features rtmp,quinn-transport` CI invocation actually
   cascades through and disables `cmaf-writer` transitively.
   Cargo disallows overriding a workspace dep's
   `default-features` when a crate inherits via `workspace =
   true`, so the path dep is inlined here with the same version
   pin to keep the graph consistent. Workspace `Cargo.toml`
   stays untouched.

3. **`crates/lvqr-cli/src/lib.rs`** (WS ingest handler). The
   WS ingest path was calling `remux::video_segment` directly,
   bypassing the `build_video_segment` dispatch. Switched both
   call sites (keyframe + delta) to `remux::build_video_segment`
   so the WS-ingest bridge honors the feature flag exactly like
   the RTMP-ingest bridge does. Under the new default this
   routes through `lvqr_cmaf::build_moof_mdat`.

4. **`.github/workflows/ci.yml`**. Renamed `test-cmaf-writer` to
   `test-legacy-fmp4-path`. The job now runs
   `cargo build -p lvqr-cli --no-default-features --features
   rtmp,quinn-transport`, `cargo test -p lvqr-ingest
   --no-default-features --features rtmp`, and `cargo test -p
   lvqr-cli --no-default-features --features rtmp,quinn-transport`
   so both the bridge dispatch and the two E2E integration tests
   exercise the legacy writer on every PR. The job cache-prefix
   was updated to `legacy-fmp4-v1`.

### Verification run

* `cargo test --workspace` -- 66 binaries, 0 failures (default:
  cmaf-writer on).
* `cargo test -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --test rtmp_hls_e2e --test rtmp_ws_e2e` --
  both E2E tests green under the legacy writer path (verified the
  legacy dispatch branch is active by inspecting `cargo tree
  --no-default-features --features rtmp,quinn-transport` and
  confirming `lvqr-ingest` no longer pulls `lvqr-cmaf` as a
  normal dep under this config).
* `cargo test -p lvqr-ingest --test parity_avc_init --test
  parity_avc_segment` -- both parity gates green (they run under
  the default config, which has both writers available: the
  hand-rolled one is unconditionally compiled for this cycle,
  and `lvqr-cmaf` is on as a dev-dep).
* `cargo clippy --workspace --all-targets -- -D warnings` clean
  under the default feature set.
* `cargo clippy -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --all-targets -- -D warnings` clean on the
  legacy path.
* `cargo fmt --all --check` clean.

### What session 12.2 did NOT land

* **Deletion of the hand-rolled `video_segment` writer.** That is
  the follow-up session's job, now that the feature flag is in
  place and the cycle clock has started. Deletion removes
  `remux::fmp4::video_segment` plus its unit tests, the golden
  file tests at `tests/golden_fmp4.rs` that still reference it,
  the proptest target at `tests/proptest_parsers.rs::video_segment_is_well_formed`,
  the parity tests at `tests/parity_avc_init.rs` +
  `tests/parity_avc_segment.rs`, the `legacy-fmp4` feature on
  `lvqr-ingest/Cargo.toml`, and the `test-legacy-fmp4-path` CI
  job. That is a cohesive single-commit change once a future
  session decides the retirement cycle is over.
* **Audio rendition group in HLS.** Still deferred; see the
  session 12 entry below.
* **`lvqr-whep` implementation.** Still scoping-doc only.

### Recommended entry point (session 13)

With session 12.2's flip landed, the session 11 work list has
shrunk to two remaining candidates:

1. **Audio rendition group in HLS** (was item 2). Scope
   unchanged. Forces `lvqr-hls` to learn master-playlist
   (`EXT-X-STREAM-INF`) generation. Full-session item.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`. Full-session item.

Plus the natural follow-up from this session:

3. **Delete the hand-rolled fMP4 writer**. Scoped above. The
   risk here is purely "did we miss a caller?"; the CI job has
   been guarding the dispatch for multiple sessions and the
   parity gate has caught every drift during the transition. A
   half-session task if it is bundled with one of items 1 or 2.

My recommendation: pair **(3) deletion + (1) audio rendition
group start**. Deletion is mechanical enough to fit in the first
half of a session; the remaining time covers adding master-playlist
scaffolding to `lvqr-hls` and a single-rendition master-playlist
rendering test. If (1) blows the time budget, item (3) can be
deferred to the next cycle with no harm done -- the flag is
already in place and the CI job already exists.

## Session 12 (2026-04-13): multi-broadcast LL-HLS routing

Session 12 closed item 1 from the session-11 work list
("Multi-broadcast HLS routing"). One logical change landed across
four files:

1. **`MultiHlsServer` in `lvqr-hls`**
   (`crates/lvqr-hls/src/server.rs`). New type that owns a
   `std::sync::Mutex<HashMap<String, HlsServer>>` keyed by broadcast
   name plus a template `PlaylistBuilderConfig` for lazily creating
   per-broadcast state. Exposes `ensure_broadcast(name) -> HlsServer`
   for the producer side, `get_broadcast(name) -> Option<HlsServer>`
   for the consumer side (so unknown broadcasts return 404 instead
   of an empty 200), `broadcast_count()` for tests, and
   `router()` which mounts a single `/hls/{*path}` catch-all.
   The catch-all exists because broadcast names contain a slash
   today (the RTMP bridge names broadcasts `{app}/{key}`, e.g.
   `live/test`), so a simple `/hls/{broadcast}/...` path param
   would not capture them. A `split_broadcast_path` helper splits
   the tail off the path, matches it against `playlist.m3u8`,
   `init.mp4`, or a chunk URI, and dispatches to one of three
   new shared `render_*` helpers extracted from the old free
   handlers. The single-broadcast `HlsServer::router()` still
   exists and still works; the `render_playlist` / `render_init`
   / `render_uri` helpers are the only rendering path, so the
   blocking-reload semantic lives in one place.

2. **`HlsFragmentBridge` in `lvqr-cli`**
   (`crates/lvqr-cli/src/hls.rs`). Rewritten around
   `MultiHlsServer`. Removed the "first broadcast wins" logic;
   every broadcast that publishes a video track now gets its own
   per-broadcast `CmafPolicyState` keyed by broadcast name in a
   `Mutex<HashMap<String, CmafPolicyState>>`. A fresh
   `VIDEO_90KHZ_DEFAULT` entry is installed the first time a
   broadcast publishes its init segment; a new init on the same
   broadcast resets the entry so a mid-stream codec change starts
   from a clean slate. Audio is still ignored here; audio
   rendition groups land separately when `lvqr-hls` grows
   master-playlist support.

3. **`lvqr-cli::start()`** (`crates/lvqr-cli/src/lib.rs`). Swapped
   the single `HlsServer::new(...)` construction for
   `MultiHlsServer::new(...)`. The axum serve task still just
   calls `server.router()`; the import line is the only other
   change. `ServerHandle::hls_url` stays unchanged (still a base-
   URL helper); tests compose `/hls/{broadcast}/...` paths
   explicitly.

4. **`crates/lvqr-cli/tests/rtmp_hls_e2e.rs`**. Renamed to
   `rtmp_publish_reaches_multi_broadcast_hls_router`. Extracted
   the publish-two-keyframes sequence into a
   `publish_two_keyframes(addr, app, key)` helper and the
   playlist-fetch-and-parse check into a
   `fetch_playlist_and_part_uris(hls_addr, app, key)` helper.
   The test now publishes two concurrent RTMP broadcasts
   (`live/one` and `live/two`) to the same `TestServer`, fetches
   `/hls/live/one/playlist.m3u8` and `/hls/live/two/playlist.m3u8`,
   asserts each playlist is well-formed LL-HLS (starts with
   `#EXTM3U`, carries `#EXT-X-VERSION:9`, names `init.mp4` via
   `#EXT-X-MAP`, and references at least one `#EXT-X-PART:` URI),
   fetches one part from each broadcast and asserts both bodies
   start with a `moof` box, fetches `/hls/live/one/init.mp4` and
   `/hls/live/two/init.mp4` and asserts both start with `ftyp`,
   and finally fetches `/hls/live/ghost/playlist.m3u8` and
   asserts it returns 404. Passed first run under both the
   default feature set and `--features cmaf-writer`.

### What session 12 did NOT land

* **`cmaf-writer` flipped to default-on.** The session 11
  directive called for at least one release cycle on main before
  flipping; session 12 honored that by leaving the feature
  default-off. Candidate for session 13 if the matrix stays
  green.
* **Hand-rolled `video_segment` retirement behind `legacy-fmp4`.**
  Same gating. Parity test at
  `crates/lvqr-ingest/tests/parity_avc_segment.rs` still owns
  the correctness property.
* **Audio rendition group in HLS.** Still deferred. Forces
  `lvqr-hls` to learn master-playlist / `EXT-X-STREAM-INF`
  generation; scoped as a full session by itself in the session
  11 handoff.
* **`lvqr-whep` implementation.** Still scoping-doc only at
  `crates/lvqr-whep/docs/design.md`.

### Recommended entry point (session 13)

The four candidates from session 11 minus the one that landed:

1. **Audio rendition group in HLS** (was item 2). Scope
   unchanged; forces master-playlist generation in `lvqr-hls`.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`.
3. **Flip `cmaf-writer` to default-on + retire the hand-rolled
   writer behind `legacy-fmp4`** (was item 4). Session 11's
   gating language ("at least one release cycle on main") is
   now satisfiable: the matrix shipped in session 11, session 12
   added to the surface it exercises, and both writer paths
   stayed green.

The safest single-session pair is (3) plus a start on (1):
flipping `cmaf-writer` is mechanical once the release-cycle
clock is up, and master-playlist work in `lvqr-hls` is
incremental enough that even landing just the master-playlist
type plus a single-rendition rendering test is forward
progress toward (1).

## Session 11 (2026-04-13): CLI HLS composition + `cmaf-writer` feature flag

Session 11 closed every item from the "Recommended Tier 2.3 entry
point (session 11)" work list below. Two commits land on top of
`f83a280` (the session-10 audit + handoff refresh):

1. **Dev-dep cycle broken** (item 1). Both `parity_avc_init.rs` and
   `parity_avc_segment.rs` moved out of `crates/lvqr-cmaf/tests/`
   and into `crates/lvqr-ingest/tests/`. `lvqr-cmaf` no longer
   dev-deps `lvqr-ingest`; `lvqr-ingest` now dev-deps `lvqr-cmaf`
   plus `mp4-atom = "0.10"` for the `Moof::decode` calls the parity
   tests need. The dep direction is one-way, which is what session
   11 item 3 requires. Both parity tests still pass byte-for-byte
   identically to the session-10 baseline (sizes equal at 600 bytes,
   bytes intentionally differ, structural fields match).

2. **`lvqr-cli serve` composes HLS** (item 2). Three pieces:

   * **`FragmentObserver` hook in `lvqr-ingest`**
     (`crates/lvqr-ingest/src/observer.rs`). New trait with
     `on_init(&self, broadcast, track, init: Bytes)` and
     `on_fragment(&self, broadcast, track, fragment: &Fragment)`.
     The bridge gets a builder method
     `RtmpMoqBridge::with_observer(SharedFragmentObserver)` plus a
     `set_observer` mutator. Both the video and audio paths fire
     `on_init` when an init segment becomes available and
     `on_fragment` after each `MoqTrackSink::push`. The bridge stays
     HLS-agnostic; the trait is the only wire between RTMP ingest
     and any non-MoQ consumer.

   * **`HlsFragmentBridge` in `lvqr-cli`**
     (`crates/lvqr-cli/src/hls.rs`). Implements
     `FragmentObserver`. Uses `lvqr_cmaf::CmafPolicyState` directly
     (re-exported from `lvqr-cmaf` this session) to classify each
     fragment as `Partial` / `PartialIndependent` / `Segment`, then
     spawns a tokio task per push that forwards the resulting
     `CmafChunk` into a shared `HlsServer`. Single-rendition
     today: the first broadcast that publishes a video track wins
     and subsequent broadcasts have their fragments dropped (with a
     `tracing::info!` at attach time so production operators see
     it). Multi-broadcast routing is a follow-up; the integration
     test publishes one broadcast so the limit is invisible at the
     contract layer.

   * **`ServeConfig.hls_addr` + `ServerHandle::hls_addr` /
     `hls_url`** (`crates/lvqr-cli/src/lib.rs`). New optional
     `hls_addr: Option<SocketAddr>` field on `ServeConfig`. When
     set, `start()` builds an `HlsServer`, attaches an
     `HlsFragmentBridge` observer to the RTMP bridge, pre-binds the
     HLS TCP listener, and spins up a fourth `axum::serve` task
     under the same shutdown token as the relay / RTMP / admin
     subsystems. `ServerHandle` grows `hls_addr() ->
     Option<SocketAddr>` and `hls_url(path: &str) -> Option<String>`
     accessors. `lvqr-cli serve` gains a `--hls-port` /
     `LVQR_HLS_PORT` flag (default `8888`, set to `0` to disable).
     `TestServer` enables HLS by default and exposes `hls_addr` /
     `hls_url` helpers; `TestServerConfig::without_hls()` turns the
     surface off for tests that do not need it.

   * **`crates/lvqr-cli/tests/rtmp_hls_e2e.rs`**. Real end-to-end
     test: spin up a `TestServer`, RTMP-publish two keyframes
     spaced 2.1 s apart through `rml_rtmp` so the segmenter's
     default `VIDEO_90KHZ_DEFAULT` policy closes one full segment,
     then drive a 30-line raw-TCP HTTP/1.1 client against
     `/playlist.m3u8`, assert the body contains an `EXTM3U` header,
     `EXT-X-VERSION:9`, `EXT-X-MAP`, and at least one
     `#EXT-X-PART` URI, fetch one of those URIs and assert the
     body starts with a `moof` box, then fetch `/init.mp4` and
     assert the body starts with `ftyp`. Passed first run. The
     intentional zero-new-deps choice (raw HTTP/1.1 vs. pulling
     `reqwest` or `hyper-util`) keeps the dev-dep budget small.

3. **`cmaf-writer` feature flag on `lvqr-ingest`** (item 3). New
   default-off feature `cmaf-writer` on `lvqr-ingest` that pulls in
   `lvqr-cmaf` as an optional normal dep. When the feature is on,
   the bridge's per-frame video media segment is built via a new
   `lvqr_ingest::remux::fmp4::build_video_segment` helper that
   delegates to `lvqr_cmaf::build_moof_mdat` instead of the
   hand-rolled `video_segment`. The hand-rolled path stays in
   place under the default feature set so the parity gate keeps
   working. `lvqr-cli` exposes a passthrough `cmaf-writer` feature
   so `cargo test -p lvqr-cli --features cmaf-writer` flips both
   crates in one shot. New `test-cmaf-writer` CI matrix job in
   `.github/workflows/ci.yml` that builds `lvqr-cli` with the
   feature on, runs `cargo test -p lvqr-ingest --features
   cmaf-writer`, and runs `cargo test -p lvqr-cli --features
   cmaf-writer` so both `rtmp_ws_e2e` and `rtmp_hls_e2e` exercise
   the alternate writer end-to-end on every PR. Both E2E tests
   pass under both writers locally.

4. **`lvqr-whep` scoping doc** (item 4).
   `crates/lvqr-whep/docs/design.md`. **No code, no Cargo.toml,
   not a workspace member.** The directory contains exactly one
   markdown file. Covers what WHEP needs from `CmafChunk` (path A:
   consume `RawSample` via `SampleStream`, preferred; path B: parse
   `mdat` on the wire, transitional shim), how the offer / trickle /
   terminate signaling maps onto axum routes in the same shape as
   `HlsServer::router`, the existing crates that get reused
   (`lvqr-cmaf::SampleStream`, `lvqr-fragment`, the Fragment
   Observer pattern, `lvqr-hls::server` as a routing template,
   `lvqr-auth`, `lvqr-core::EventBus`), the new external dep
   (`str0m`), the 5-artifact plan with concrete test file paths,
   sequencing constraints (waits on bridge raw-sample emission via
   either the `cmaf-writer` cutover or a sibling `RawSampleObserver`
   hook), and four open questions for the implementation session.

### What session 11 did NOT land

* **Multi-broadcast HLS routing.** `HlsFragmentBridge` is
  single-rendition / single-broadcast today. The router serves
  one playlist; subsequent RTMP broadcasts are tracked but their
  fragments are silently dropped. Adding a `/hls/{broadcast}/...`
  prefix or one `HlsServer` per broadcast is a session-12 follow-up.
* **Audio in HLS.** The `FragmentObserver::on_fragment` hook fires
  for both `0.mp4` (video) and `1.mp4` (audio), but
  `HlsFragmentBridge` only consumes video. Multi-track HLS (audio
  rendition group) lands when multi-track HLS lands.
* **`cmaf-writer` flipped to default-on.** Per the directive, the
  feature is default-off this session. The CI matrix exercises
  both paths; the default flips after the matrix has been green on
  main for a few release cycles.
* **Hand-rolled `video_segment` deletion.** Same gating as the
  default flip. The parity gate at
  `crates/lvqr-ingest/tests/parity_avc_segment.rs` keeps both
  writers honest until the deletion lands.
* **WHEP implementation.** Scoping only this session, per the
  directive.

### Contract slot status as of session 11

| Crate          | proptest | fuzz | integration | E2E              | conformance |
| lvqr-ingest    | yes      | yes  | yes         | yes              | yes         |
| lvqr-codec     | yes      | yes  | yes         | via rtmp_ws_e2e  | yes (multi-sub-layer covered) |
| lvqr-cmaf      | yes      | open | yes         | via rtmp_ws_e2e  | yes (AVC + HEVC + AAC init, AVC + AAC coalescer) |
| lvqr-hls       | yes      | open | yes         | via oneshot + lvqr-cli rtmp_hls_e2e | soft-skip (mediastreamvalidator) |
| lvqr-record    | yes      | open | yes         | workspace e2e    | yes         |
| lvqr-moq       | yes      | open | yes         | via rtmp_ws_e2e  | n/a         |
| lvqr-fragment  | yes      | open | yes         | via rtmp_ws_e2e  | n/a         |

`lvqr-hls` E2E slot grew from "via oneshot" alone to "via oneshot
plus lvqr-cli rtmp_hls_e2e". The `oneshot` test still runs every
HLS handler over the axum service trait; the new `rtmp_hls_e2e`
runs the same router over a real loopback TCP socket end-to-end
from RTMP publish to HTTP GET.

### Dependency graph snapshot (post-session-11)

* `lvqr-cli` normal-deps `lvqr-cmaf`, `lvqr-hls`, `lvqr-fragment`
  (added this session for the HLS bridge).
* `lvqr-hls` normal-deps `lvqr-cmaf`.
* `lvqr-ingest` normal-deps `lvqr-cmaf` ONLY when the `cmaf-writer`
  feature is on (optional dep). The default feature set leaves the
  dep edge absent.
* `lvqr-ingest` dev-deps `lvqr-cmaf` + `mp4-atom` for the parity
  tests (one-way, no cycle).
* `lvqr-cmaf` no longer dev-deps anything in the producer side.
* No other dep edges changed. Graph is still acyclic.

## Sessions 6-10 audit (2026-04-13, pre-session-11)

Ran a structural audit before writing the session 11 kickoff
prompt. Findings:

1. **Tree health**. `cargo fmt --all --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`, and `cargo test
   --workspace` all pass cleanly. 241 tests across 60 binaries.
   Local HEAD matches `origin/main`. Working tree is clean.

2. **Contract script drift**. `scripts/check_test_contract.sh`
   had `lvqr-hls` commented out in the "will be enabled as they
   land" section. Session 7 landed the crate and sessions 7-8
   closed 4-of-5 slots. Fixed in the audit commit: `lvqr-hls`
   moved into the `IN_SCOPE` list. The script now reports one
   crate-level warning per session 10 expected open slot (fuzz
   on lvqr-cmaf, lvqr-hls, lvqr-record, lvqr-moq, lvqr-fragment)
   and nothing unexpected.

3. **CONTRACT.md staleness**. `tests/CONTRACT.md` did not list
   `lvqr-hls`, still named the `mediastreamvalidator` wrapper
   and the kvazaar fixture as open items (both closed in
   sessions 7 and 8 respectively), and did not mention the
   coalescer conformance or the sample-segmenter integration
   test. Rewritten in the audit commit to reflect the real
   session 10 contract-slot state.

4. **Producer wiring gap (intentional)**. `TrackCoalescer`,
   `RawSample`, `SampleStream`, and `CmafSampleSegmenter` have
   zero consumers in `lvqr-ingest` / `lvqr-cli` / any producer
   crate. The raw-sample pipeline is fully tested inside
   `lvqr-cmaf` via scripted `VecDeque`-backed streams but does
   not yet drive a real ingest. This is expected; session 11 is
   where the wiring lands. Flagged so future sessions do not
   assume the pipeline is in production use.

5. **Expect/unwrap in coalescer production paths**. Five
   `.expect()` / `.unwrap()` calls in `crates/lvqr-cmaf/src/coalescer.rs`:
   three are state-machine precondition enforcement (pending
   implies partial_start / segment_start / pending_dts) and two
   are `mp4-atom` encoder calls that can only fail on
   structurally invalid `Moof` inputs. All are invariant-
   protected; none take untrusted input. No action required
   today but worth noting for future hardening.

6. **CI coverage**. `.github/workflows/ci.yml` installs ffmpeg on
   both Linux and macOS runners so every `ffprobe_bytes` check
   runs for real. It does NOT install kvazaar (not needed, the
   multi-sub-layer HEVC fixture is pinned bytes now) or
   `mediastreamvalidator` (soft-skip handles absence). Contract
   script still runs in Tier 1 educational mode; strict mode
   flips on when the remaining fuzz slots are either closed or
   documented as intentionally open.

7. **Dependency graph snapshot**. `lvqr-hls` normal-deps
   `lvqr-cmaf`. `lvqr-cmaf` dev-deps `lvqr-ingest` (for the
   parity tests only). `lvqr-ingest` does NOT depend on
   `lvqr-cmaf` in either direction today. Clean acyclic graph.
   Session 11 item 2 (feature flag retirement) requires
   `lvqr-ingest` to normal-dep `lvqr-cmaf`, which creates a
   cycle with the current dev-dep direction. The fix is to
   move `parity_avc_init.rs` and `parity_avc_segment.rs` out of
   `lvqr-cmaf/tests/` and into a top-level workspace test
   crate (or into `lvqr-ingest/tests/`) before flipping the
   normal-dep direction. Documented here rather than in the
   individual commits so session 11 does not trip on it.

## Session 10 follow-ups (2026-04-13): TrackCoalescer pipeline

Two commits landed between session 9 and the session 10 audit,
closing the remaining tier-2.3 items that did not require touching
the CLI composition root:

1. **Coalescer parity gate against the hand-rolled writer**
   (`crates/lvqr-cmaf/tests/parity_avc_segment.rs`, commit
   `6d41c5a`). Drives the same six-sample batch through both
   `lvqr_cmaf::TrackCoalescer` and
   `lvqr_ingest::remux::fmp4::video_segment`, decodes both
   `moof` boxes via `mp4_atom::Moof::decode`, and asserts every
   playback-critical field matches: `mfhd` sequence number,
   `tfhd` track id, `tfdt` base_media_decode_time, `trun`
   entry count, per-sample duration/size/flags/cts offset, and
   `data_offset` landing inside its own buffer. The two writers
   produce media segments of **identical total size** for this
   input (`cmaf=600, ingest=600, delta=0`), though not identical
   bytes. A second test pins the intentional non-equality so a
   future session cannot silently replace the structural gate
   with a byte-equality assertion.

2. **AAC audio coalescer ffprobe round trip**. Extended
   `crates/lvqr-cmaf/tests/conformance_coalescer.rs` with an
   AAC variant: 20 synthetic AAC frames (1024 ticks each, 128
   bytes of zero payload) through a `TrackCoalescer` +
   `CmafPolicy::AUDIO_48KHZ_DEFAULT`, concatenated with the
   `write_aac_init_segment` output and fed to ffprobe 8.1.
   ffprobe accepts. Both video and audio paths through the
   coalescer now have real-encoder validation on top of the
   structural unit tests.

3. **SampleStream trait + CmafSampleSegmenter**
   (`crates/lvqr-cmaf/src/sample.rs`, `crates/lvqr-cmaf/src/coalescer.rs`,
   `crates/lvqr-cmaf/tests/integration_sample_segmenter.rs`,
   commit `c24fe51`). Closes session 10 item 1 from the prior
   HANDOFF: a pull-based trait `SampleStream` with
   `next_sample()` returning `Pin<Box<dyn Future<Output =
   Option<RawSample>> + Send>>` (boxed future instead of
   async-fn-in-trait because Send bounds still require GAT
   plumbing this crate does not need), and a
   `CmafSampleSegmenter` type that owns a
   `HashMap<TrackId, TrackCoalescer>`, routes incoming samples
   into the right coalescer, queues the resulting chunks into a
   ready buffer, and drains every coalescer's trailing pending
   batch on stream exhaustion before returning `None`.

   Integration tests cover a single-track ffprobe round trip
   through the full pipeline (init segment + chunks concatenated
   and accepted by ffprobe 8.1) and a multi-track routing test
   that interleaves video (track 1) and audio (track 2) and
   asserts both tracks produce chunks tagged with the correct
   `"{track_id}.mp4"` string.

### What session 10 did NOT land

* **`lvqr-cli` HLS composition** -- the CLI serve path does not
  yet expose an HLS axum binding. Blocker for `TestServer`
  growing a real HLS address and for the loopback TCP E2E.
* **Hand-rolled `video_segment` retirement** behind a feature
  flag. Blocked on the dev-dep cycle surfaced by the audit
  (item 7 above).
* **First non-HLS egress crate** (WHEP / DASH / WHIP). Per the
  audit list, this waits until the CLI composition lands so the
  egress crates can be validated against a real end-to-end
  pipeline rather than a standalone router harness.

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

## Recommended Tier 2.3 entry point (session 12)

Session 11 closed every cross-crate item from the prior list (dep
cycle, CLI HLS composition, `cmaf-writer` feature flag, WHEP
scoping). Session 12 inherits the follow-ups that depend on either
the new HLS pipeline being on `main` for a release cycle, or the
WHEP scoping doc being ready to absorb implementation work.

1. **Multi-broadcast HLS routing.** The `HlsFragmentBridge` shipped
   in session 11 is intentionally single-rendition: only the first
   broadcast that publishes a video track feeds the HLS server.
   Production-grade routing requires either (a) a per-broadcast
   `HlsServer` instance keyed by broadcast name with the axum
   router demultiplexing under a `/hls/{broadcast}/...` prefix, or
   (b) a multi-tenant `HlsServer` that grows broadcast-aware
   routing internally. Option (a) keeps `lvqr-hls` simple and
   matches the LL-HLS single-rendition mental model; option (b)
   touches the manifest generator. Pick (a) unless option (b)
   surfaces a clean reuse path during implementation. Update
   `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` to publish two
   broadcasts and assert both playlists return distinct content.

2. **Audio rendition group in HLS.** `FragmentObserver::on_fragment`
   already fires for `1.mp4`. `HlsFragmentBridge` ignores it
   today. The work is: extend `HlsFragmentBridge` to track a
   second `CmafPolicyState` keyed on the audio track id; mount a
   sibling `HlsServer` (or sibling per-track tracks inside one
   server, depending on whether `lvqr-hls` learns rendition
   groups) at `/hls/audio/playlist.m3u8`; update the integration
   test to verify the audio playlist is fetchable and that an
   `EXT-X-MEDIA:TYPE=AUDIO` master playlist points at it. The
   scope is bigger than item 1 because it forces `lvqr-hls` to
   learn `EXT-X-STREAM-INF` master-playlist generation. Plan
   carefully before starting.

3. **Begin `lvqr-whep` implementation.** The session 11 design doc
   at `crates/lvqr-whep/docs/design.md` lays out a 5-artifact plan
   with concrete test file paths plus four open questions. Start
   by answering the four open questions in a 5-bullet design
   reply, then create `crates/lvqr-whep/Cargo.toml`, register the
   crate as a workspace member, and land item 1 of the 5-artifact
   plan (proptest on the H.264 RTP packetizer) before any
   networking code. **Prerequisite**: a `RawSampleObserver` hook on
   `RtmpMoqBridge` so WHEP can subscribe to per-sample data
   without re-parsing CmafChunks. The cleanest add is a sibling
   trait method (or a new trait altogether) following the same
   pattern as the session-11 `FragmentObserver`. Pick this only
   if items 1 and 2 above are deferred or already in progress;
   running all three in one session is too much surface area.

4. **Flip `cmaf-writer` to default-on.** Once the
   `test-cmaf-writer` matrix job has been green on `main` for at
   least one release cycle (track in `tracking/HANDOFF.md` cycle
   notes), flip `default = ["rtmp", "cmaf-writer"]` in
   `crates/lvqr-ingest/Cargo.toml`. Keep the hand-rolled
   `video_segment` writer in place under a `legacy-fmp4` feature
   for one more cycle, then delete in a later session. The parity
   gate at `crates/lvqr-ingest/tests/parity_avc_segment.rs`
   becomes unnecessary at deletion time and should be removed in
   the same commit.

Session 12 should pick **at most two** of the four items above and
land them cleanly. Items 1 and 4 are the safest pair to bundle.
Items 2 and 3 each blow most of a session by themselves.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, or
`lvqr-archive` this session. Every non-WHEP egress crate stays
gated on the items above, plus eventually `lvqr-whep` itself
landing as the proof point that the egress shape generalizes
beyond HLS.

## Recommended Tier 2.3 entry point (session 11, closed)

Session 10 closed every in-crate item from the session 10 list
(parity gate, AAC coalescer round trip, sample segmenter). Session
11 inherited the cross-crate items that required touching the CLI
composition root and flipping dep directions. All four landed.
The original work list is preserved here for historical reference:

1. **Break the `lvqr-cmaf <-> lvqr-ingest` dev-dep cycle** before
   any session 11 work that requires `lvqr-ingest` to normal-dep
   `lvqr-cmaf`. Option A: move `parity_avc_init.rs` and
   `parity_avc_segment.rs` out of `crates/lvqr-cmaf/tests/` and
   into a new top-level `tests/parity/` directory as a standalone
   test crate that normal-deps both `lvqr-cmaf` and
   `lvqr-ingest`. Option B: move the parity tests into
   `crates/lvqr-ingest/tests/` as a dev-dep on `lvqr-cmaf`; this
   reverses the current direction and sets up item 2 cleanly.
   Pick option B unless option A turns up a better reason during
   implementation.

2. **Wire `lvqr-cli serve` to compose HLS**. Add an `--hls-addr`
   flag to `ServeConfig`, have `lvqr_cli::start` spin up an axum
   binding on that address with `HlsServer::router()`, and
   adapt the RTMP bridge's fragment output into the `HlsServer`
   push API via the pass-through `CmafSegmenter` (no coalescer
   needed yet -- the bridge still emits pre-muxed `Fragment`
   values). Day-one E2E: extend
   `lvqr-test-utils::TestServer` with an `hls_url()` helper and
   write a new integration test in `crates/lvqr-cli/tests/`
   that publishes a real RTMP stream, fetches
   `GET /playlist.m3u8`, and asserts the playlist contains the
   ingested broadcast's segments. This is the canonical "can
   LVQR serve HLS to a real HTTP client" proof.

3. **Retire the hand-rolled
   `lvqr-ingest::remux::fmp4::video_segment` writer behind a
   feature flag**. Prerequisite: item 1 must be done so the
   `lvqr-ingest` -> `lvqr-cmaf` normal-dep can be added. Then:
   add a `cmaf-writer` feature on `lvqr-ingest` (default off
   during the transition) that routes through
   `lvqr_cmaf::TrackCoalescer::flush_pending` +
   `lvqr_cmaf::build_moof_mdat` instead of the hand-rolled
   `video_segment`. Flip the feature on in a CI matrix job so
   both paths are exercised on every PR. When both are green on
   main for a few sessions, flip the default to on, then delete
   the hand-rolled writer in a later session.

4. **Scope the first non-HLS egress crate**. Likely `lvqr-whep`
   because WHEP is the simplest WebRTC-based subscribe path and
   it slots cleanly onto `CmafChunk` (WHEP consumers see
   `CmafChunk`s, not raw samples). Do NOT start implementation
   until items 1-3 above land; otherwise the crate has no real
   producer to validate against.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, or
`lvqr-archive` this session. Every non-HLS egress crate is gated
on the session 11 wiring above. Stay focused on closing the Tier
2.3 loop before any new protocol crate begins.

### Session 10 items from the prior HANDOFF (all closed)

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
