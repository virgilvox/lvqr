# LVQR Handoff Document

## Project Status: v0.4.0 -- Tier 3 COMPLETE; Tier 4 items 4.2 + 4.1 + 4.3 COMPLETE (end-to-end C2PA provenance: signing primitive + composition helpers + cert fixture + finalize orchestrators + drain-terminated finalize + admin verify route + E2E); 758 tests, 26 crates

**Last Updated**: 2026-04-19 (handoff staged for session 95 entry; session 94 closed 2026-04-19). Tier 4 item 4.3 session B3 landed end-to-end C2PA provenance: `FragmentBroadcasterRegistry::on_entry_removed` lifecycle hook (mirrors `on_entry_created`, fires synchronously on successful `remove()` with lock released, NEVER from Drop -- load-bearing primitive for 4.4 + 4.5); `RtmpMoqBridge::on_unpublish` now calls `registry.remove` for both tracks so drain tasks see `next_fragment() -> None` per-broadcast (was per-server-shutdown); flat `<archive>/<broadcast>/<track>/init.mp4` layout + `write_init` writer helper; `BroadcasterArchiveIndexer::drain` invokes `finalize_broadcast_signed` inside spawn_blocking when drain terminates with `C2paConfig` configured; `GET /playback/verify/{broadcast}` admin route via `c2pa::Reader::with_manifest_data_and_stream`; E2E test `c2pa_verify_e2e.rs` exercises the full path (RTMP publish -> unpublish -> finalize -> verify). Breaking API refactor on `C2paConfig`: new `C2paSignerSource` enum replaces inline PEM fields with `CertKeyFiles { .. }` + `Custom(Arc<dyn c2pa::Signer + Send + Sync>)` variants -- the Custom variant is the E2E path (`c2pa::EphemeralSigner` without disk PEMs) and the HSM / KMS operator story. Feature plumbing: `lvqr-cli` gains `c2pa` feature; `lvqr-test-utils` gains `c2pa` + `TestServerConfig::with_c2pa(..)`. Workspace tests 758 passing on macOS (up from 739; +4 registry, +4 writer, +2 c2pa_sign Custom-source, +1 c2pa_verify_e2e, +5 provenance lib tests now active in workspace builds, +3 misc). Session 95 entry point is Tier 4 item 4.8 session A (One-token-all-protocols) -- see the refreshed `Plan-vs-code status` under TIER_4_PLAN.md section 4.8 for the scope-up vs. the session-84 original (three ingest crates have NO auth call-site today; session 95 must add new call-sites not just collapse existing extractors).

## Session 94 close (2026-04-19)

### What shipped

1. **Tier 4 item 4.3 session B3: drain-terminated
   C2PA finalize + admin verify route + E2E**
   (`56ba151`). Five deliverables in one commit,
   closing out item 4.3:

   **(a) `on_entry_removed` lifecycle hook on
   `FragmentBroadcasterRegistry`**. Mirror of
   `on_entry_created` -- `(broadcast, track, &Arc<
   FragmentBroadcaster>)` triple, fires synchronously
   from `remove()` after the map write lock is
   released (callbacks may freely re-enter the
   registry), in installation order, NEVER from Drop
   (deterministic fire point for 4.4 federation
   gossip + 4.5 agent shutdown; no Drop-reentrancy
   hazards). `RtmpMoqBridge::on_unpublish` now calls
   `registry.remove(stream_name, "0.mp4")` + audio so
   drain tasks see `next_fragment() -> None` per-
   broadcast (was per-server-shutdown).

   **(b) Init-bytes persistence** to flat
   `<archive>/<broadcast>/<track>/init.mp4`. Layout
   picked over `metadata.json` sidecar for three
   reasons (parallels segment layout for non-c2pa
   consumers, bytes already MP4 so concat is literal,
   no extra JSON surface needed today). New
   `lvqr_archive::writer::write_init` +
   `init_segment_path` + `INIT_SEGMENT_FILENAME`
   helpers. Drain task refreshes meta each loop
   iteration and persists on first fragment where
   init is set.

   **(c) Drain-task integration**.
   `BroadcasterArchiveIndexer::drain` takes
   `Option<C2paConfig>` (feature-gated) and, on
   while-loop exit, spawn_blocking's
   `finalize_broadcast_signed` which reads
   `init.mp4`, walks the redb segment index in
   `start_dts` order, concats, signs, writes
   `finalized.mp4` + `finalized.c2pa`. Errors log
   `warn!`; no retry.

   **(d) Admin verify route**.
   `GET /playback/verify/{*broadcast}` (`crates/lvqr-
   cli/src/archive.rs::verify_router`) reads the
   finalize pair off disk, calls
   `c2pa::Reader::from_context(Context::new()).
   with_manifest_data_and_stream(..)`, returns JSON
   `{ signer, signed_at, valid, validation_state,
   errors }`. `validation_state` is the stable
   string form of `c2pa::ValidationState`
   (`"Invalid"` / `"Valid"` / `"Trusted"`); `valid`
   is true for Valid + Trusted. `errors` filters out
   `signingCredential.untrusted` (c2pa-rs itself
   treats it as non-fatal). Auth runs the same
   subscribe-token gate the sister `/playback/*`
   routes use.

   **(e) E2E test** at
   `crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Real
   RTMP publish via `rml_rtmp`, drop publisher, poll
   for `finalized.c2pa` on disk with a 10 s budget,
   hit `/playback/verify/live/dvr`, assert
   `valid=true`, `validation_state="Valid"`,
   non-empty signer, empty errors; also asserts 404
   on an unknown broadcast.

   **Breaking API change**. New `C2paSignerSource`
   enum with `CertKeyFiles { signing_cert_path,
   private_key_path, signing_alg,
   timestamp_authority_url }` +
   `Custom(Arc<dyn c2pa::Signer + Send + Sync>)`
   variants. The old inline PEM fields on
   `C2paConfig` move into the `CertKeyFiles`
   variant; migration is a single-file diff per
   operator:

   ```
   // was:
   C2paConfig {
       signing_cert_path, private_key_path,
       signing_alg, timestamp_authority_url,
       assertion_creator, trust_anchor_pem,
   }
   // now:
   C2paConfig {
       signer_source: C2paSignerSource::CertKeyFiles {
           signing_cert_path, private_key_path,
           signing_alg, timestamp_authority_url,
       },
       assertion_creator, trust_anchor_pem,
   }
   ```

   The `Custom` variant covers two real shapes with
   one enum: tests using `c2pa::EphemeralSigner`
   (no disk PEMs -- the B3 E2E shape), operators
   with HSM / KMS-backed keys wrapping their signer
   behind `c2pa::Signer`. Per CLAUDE.md's no-backwards-
   compat-shims rule, there is no migration helper;
   existing callers update the struct literal. Two
   new unit tests
   (`sign_asset_bytes_with_custom_signer_source_
   delegates_to_ephemeral_signer`,
   `finalize_broadcast_signed_with_custom_signer_
   source_writes_pair_to_disk`) lock the enum-
   branching behaviour.

   **Feature plumbing**:
   * `lvqr-cli` gains a `c2pa` feature enabling
     `lvqr-archive/c2pa` + `dep:c2pa` (default off;
     `full` meta-feature adds it).
   * `ServeConfig.c2pa: Option<C2paConfig>` is
     `#[cfg(feature = "c2pa")]` so the struct stays
     ABI-stable across feature flips.
   * `lvqr-test-utils` gains a `c2pa` feature +
     `TestServerConfig::with_c2pa(..)` builder.
     Enabled via dev-deps on `lvqr-cli` so
     `cargo test -p lvqr-cli --features c2pa`
     activates the full stack.

   **Plan refresh**. `tracking/TIER_4_PLAN.md`
   section 4.3 header flipped to COMPLETE; the B3
   row flipped to DONE with a full description of
   what landed.

2. **Session 94 close doc** (this commit).

### Tests shipped

| # | Test surface | Added this session |
|---|---|---|
| a | `crates/lvqr-fragment/src/registry.rs` unit tests | 4 new: `on_entry_removed_fires_exactly_once_per_successful_remove`, `on_entry_removed_multiple_callbacks_all_fire_in_installation_order`, `on_entry_removed_callback_receives_the_just_removed_arc`, `on_entry_removed_callback_may_reenter_registry_without_deadlock` |
| b | `crates/lvqr-archive/src/writer.rs` unit tests | 4 new: `init_segment_path_follows_broadcast_track_layout`, `write_init_creates_missing_parent_dirs_and_writes_bytes`, `write_init_is_idempotent_overwrites_existing_file`, `write_init_returns_io_error_when_archive_dir_is_a_file` |
| c | `crates/lvqr-archive/tests/c2pa_sign.rs` | 2 new: `sign_asset_bytes_with_custom_signer_source_delegates_to_ephemeral_signer`, `finalize_broadcast_signed_with_custom_signer_source_writes_pair_to_disk`. Existing 3 migrated to the `C2paSignerSource::CertKeyFiles` enum shape. |
| d | `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` | 1 new: `rtmp_publish_then_unpublish_yields_verifiable_c2pa_manifest` -- the full RTMP + finalize + verify E2E |

Workspace totals: **758** passed, 0 failed, 1 ignored
(up from session 93's 739 / 0 / 1). The +19 breakdown:
+4 registry, +4 writer, +2 c2pa_sign, +1 c2pa_verify_e2e,
+5 provenance lib tests that are now activated in
workspace builds because `lvqr-test-utils`'s new `c2pa`
dev-dep feature pulls in `lvqr-archive/c2pa`, +3 misc
(re-counted doctests across feature configurations).
The 1 remaining ignored test is the pre-existing
`moq_sink` doctest unrelated to 4.3.

### Ground truth (session 94 close)

* **Head**: `56ba151` (feat) before this close-doc
  commit lands; after both land local main is 13
  commits ahead of `origin/main` (sessions 89-94 feat
  + close, plus the session-94 hygiene commit on top
  of 93's close-doc commit). Verify via `git log
  --oneline origin/main..main` before any push.
  Do NOT push without direct user instruction.
* **Tests**: **758** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`
  on lvqr-archive: 35 lib + 5 integration, 0 ignored.
  With `--features c2pa` on lvqr-cli: +1 E2E
  (`c2pa_verify_e2e`), 0 ignored.
* **CI gates locally clean**:
  * `cargo fmt --all`
  * `cargo clippy --workspace --all-targets --benches -- -D warnings`
  * `cargo clippy -p lvqr-archive --features c2pa --all-targets -- -D warnings`
  * `cargo clippy -p lvqr-cli --features c2pa --all-targets -- -D warnings`
  * `cargo test -p lvqr-archive --features c2pa`
  * `cargo test -p lvqr-cli --test rtmp_archive_e2e`
    (no regression after the `registry.remove` wiring)
  * `cargo test -p lvqr-cli --features c2pa --test c2pa_verify_e2e`
  * `cargo test --workspace`
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **COMPLETE** | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | PLANNED | 95-96 |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

Three of eight Tier 4 items are now complete (4.2, 4.1,
4.3). Downstream sessions unchanged from session 93's
view; tier budget still 27 sessions (85-111) with one
session reserve.

### Session 95 entry point

**Tier 4 item 4.8 session A: One-token-all-protocols.**

Scoped + scouted against the live code at session 94
close (2026-04-19). See `tracking/TIER_4_PLAN.md`
section 4.8 for the full deliverables table and the
Plan-vs-code status block that captures the three
drifts below.

**Drift 1: `normalized_auth` is really an extractor,
not a verifier.** `lvqr_auth::AuthProvider::check(
&AuthContext)` already returns a uniform
`AuthDecision` across protocols. `JwtAuthProvider`
already handles Publish + Subscribe + Admin variants.
What session 95 A must add is the protocol-specific
EXTRACTOR layer that turns each protocol's token
carrier into a uniform `AuthContext`. The verifier
side is done.

**Drift 2: three ingest crates have NO auth call-site
today.** Scout at session 94 close found:

  - `lvqr-ingest` (RTMP): calls `auth.check` at
    `bridge.rs:456` on `AuthContext::Publish`. JWT
    is carried as the stream key (existing
    `JwtAuthProvider` convention).
  - `lvqr-relay` (MoQ): calls `auth.check` at
    `server.rs:155` on `AuthContext::Subscribe`.
  - `lvqr-cli` (WS relay + WS ingest + playback):
    calls at `lib.rs:1289` (WS relay subscribe),
    `lib.rs:1415` (WS ingest publish), and the
    playback router in `archive.rs`.
  - `lvqr-whip`: **ZERO auth references anywhere.**
  - `lvqr-srt`: **ZERO auth references anywhere.**
  - `lvqr-rtsp`: **ZERO auth references anywhere.**

Session 95 A must ADD auth call-sites to whip / srt
/ rtsp, not "migrate existing one-offs". Estimate
shifts ~+200 LOC vs the session-84 plan.

**Drift 3: session decomposition table had stale
numbers.** Fixed in session 94 close: 4.8 is now 95
/96 (was 92/93); 4.5 is 97-100 (was 94-97); 4.4 is
101-103 (was 98-100); 4.6 is 104-106 (was 101-103);
4.7 is 107-108 (was 104-105). Tier 4 budget
unchanged at 27 sessions (85-111).

**Token-carrier inventory for the extractor layer**:

  - RTMP: stream key IS the JWT. Existing.
  - WHIP: `Authorization: Bearer <jwt>` on the
    POST /whip/{broadcast} HTTP offer. Standard.
  - SRT: `streamid` handshake parameter. No industry
    standard. Proposed LVQR shape: `m=publish,r=<
    broadcast>,t=<jwt>` (`,`-separated KV pairs).
    Document in `docs/auth.md`.
  - RTSP: `Authorization: Bearer <jwt>` on
    ANNOUNCE + RECORD. Verify `rtsp-types` passes
    the header through; if not, extend the server's
    header handling -- small isolated change.
  - WS: existing `?token=<jwt>` query fallback +
    `Authorization: Bearer` header. Already handled.

**Deliverables (per TIER_4_PLAN row 95 A)**:

(a) New `lvqr-auth::extract` module (or similar)
with per-protocol `extract_<proto>` helpers that
build `AuthContext` from the protocol's token
carrier. Unit tests per helper.

(b) Wire into `lvqr-whip` + `lvqr-srt` +
`lvqr-rtsp` (new call-sites) + `lvqr-ingest` +
`lvqr-cli` WS ingest (migrations to the shared
extractor).

(c) `docs/auth.md` (new): JWT claim shape (`sub`,
`exp`, `scope`, optional `iss`, `aud`, `broadcast`)
+ per-protocol carrier conventions + one worked
example per protocol.

(d) `TestServerConfig::with_whip()` helper added
if missing (needed by session 96 B's E2E).

Session 96 B lands the cross-protocol E2E at
`crates/lvqr-cli/tests/one_token_all_protocols.rs`.

**Pre-session checklist**:

1. Read `tracking/TIER_4_PLAN.md` section 4.8
   fully (lines 422-574 in current file).
2. Confirm the current `AuthContext` enum's
   coverage against the extractor plan. If SRT
   needs a new context variant or a
   `metadata: HashMap<String,String>` side
   channel, decide + update before wiring.
3. Read `crates/lvqr-whip/src/*`, `crates/lvqr-
   srt/src/*`, `crates/lvqr-rtsp/src/*` to pick
   the right plumbing point (typically the
   connection-accept / SDP-offer / streamid-parse
   path).
4. Verify the workspace default `cargo test`
   stays green after each call-site add; the
   `NoopAuthProvider` default means adding a
   gate is behaviour-preserving for existing
   tests.

**Verification gates (session 95 A close)**:

  - `cargo fmt --all`
  - `cargo clippy --workspace --all-targets --benches -- -D warnings`
  - `cargo test -p lvqr-auth --lib`
  - `cargo test --workspace` no regression from 758
  - `git log -1 --format='%an <%ae>'` reads
    `Moheeb Zara <hackbuildvideo@gmail.com>` alone

Expected scope: ~500-700 LOC split across 95 A + 96
B (scope-up from the session-84 plan's ~300-500
estimate because of drift 2).

**Biggest risks**, ranked:

1. SRT streamid format choice. Whatever session 95
   picks, other SRT ingestors (OBS, ffmpeg) must be
   able to produce it. The `m=publish,r=...,t=...`
   shape is de-facto in the SRT community; document
   first, code second.
2. RTSP header passthrough. `rtsp-types` may or may
   not surface `Authorization` to the server
   handler cleanly. If not, the extractor falls
   back to reading the raw request and extending
   the RTSP server's header handling.
3. `TestServerConfig::with_whip()` may not exist.
   Check before the E2E -- if absent, session 95 A
   adds it as a byproduct of the plumbing pass.

## Session 93 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session B2: cert fixture +
   sign-side composability + finalize orchestrators**
   (`868c378`). Three deliverables in one commit, all
   converging on "the c2pa primitive is now end-to-end
   testable and composable for the drain-task wiring."

   **Cert-fixture breakthrough**. Discovery:
   `c2pa::EphemeralSigner` is publicly re-exported from
   c2pa 0.80 (in `pub use utils::{ephemeral_signer::
   EphemeralSigner, ...}`). It generates C2PA-spec-
   compliant Ed25519 cert chains in memory using
   c2pa-rs's own private `ephemeral_cert` module +
   rasn_pkix -- exactly the extension layout
   (digitalSignature KU, emailProtection EKU, basic-
   constraints with cA=FALSE on EE, AKI/SKI, v3) the
   structural-profile check wants. The session-91
   happy-path test (`#[ignore]`'d through sessions
   91-92 because rcgen-generated chains kept tripping
   `CertificateProfileError::InvalidCertificate`)
   unignores via this signer with zero PEM-fixture
   maintenance + zero calendar-expiry risk. The chain
   is generated per-test-run.

   **Sign-side composability refactor**:

   * New `provenance::SignOptions { assertion_creator,
     trust_anchor_pem }` -- the subset of `C2paConfig`
     that is independent of PEM paths + signing alg.
     Lets `sign_asset_with_signer` callers construct
     only what the lower-level primitive needs.
   * New `provenance::sign_asset_with_signer(&dyn
     c2pa::Signer, &SignOptions, format, bytes) ->
     Result<SignedAsset, ArchiveError>` -- low-level
     primitive that takes any `c2pa::Signer` impl.
     Tests use `EphemeralSigner`; advanced operators
     with HSM-backed or KMS-backed keys call this
     directly.
   * `sign_asset_bytes` (path-based primitive) now
     delegates to `_with_signer` after reading PEMs +
     constructing the signer. The high-level shape
     for production operators is unchanged.

   **Finalize orchestrators**:

   * `finalize_broadcast_signed_with_signer(signer,
     options, init_bytes, segment_paths, format,
     asset_path, manifest_path) -> SignedAsset` --
     composes `concat_assets` (init + segments in
     order) + `sign_asset_with_signer` +
     `write_signed_pair`. Returns SignedAsset so
     caller can log size or inspect bytes without re-
     reading from disk. `init_bytes` is taken as a
     parameter so this primitive stays agnostic to
     where init persistence lives -- session 94's
     call.
   * `finalize_broadcast_signed(&C2paConfig, ...)`
     -- high-level convenience that reads PEMs then
     delegates. Single call site for session 94's
     drain integration.

   **Test suite migration in `tests/c2pa_sign.rs`**:
   3 tests, 0 ignored. The rcgen-based
   `build_test_chain` helper + the `#[ignore]`'d
   happy-path test are deleted in favor of:

   - `sign_asset_with_signer_emits_non_empty_c2pa_
     manifest_for_minimal_jpeg` (live, was ignored
     through 91-92).
   - `finalize_broadcast_signed_with_signer_writes_
     asset_and_manifest_pair_to_disk` (new; init-only
     "broadcast" exercising concat + sign + write
     end-to-end with real on-disk reads to verify
     round-trip).
   - `sign_asset_bytes_reports_c2pa_error_on_missing_
     cert_file` (live, unchanged).

   **Cleanup**: rcgen dropped from `lvqr-archive`'s
   dev-deps + Cargo.lock. The only consumer was the
   deleted fixture builder.

   **Plan refresh**: section 4.3 header "3 sessions,
   91-93" → "4 sessions, 91-94". B2 row flipped to
   **DONE (session 93)** with the cert-fixture-
   breakthrough note + composability + finalize-
   orchestrator scope. New B3 row covers the
   remaining drain integration + verify route + E2E.

2. **Session 93 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 2 | `sign_asset_with_signer_emits_non_empty_c2pa_manifest_for_minimal_jpeg` (was `#[ignore]`'d through sessions 91-92, now live) + `finalize_broadcast_signed_with_signer_writes_asset_and_manifest_pair_to_disk` (new) in `crates/lvqr-archive/tests/c2pa_sign.rs` | both ok (feature-gated; runs on the `archive-c2pa` CI cell + locally with `--features c2pa`) |

`cargo test -p lvqr-archive --features c2pa --test
c2pa_sign`: 3 passed, 0 ignored. Previously 1 passed +
1 ignored. The c2pa-sign happy-path ignore is gone.

Workspace totals on macOS: **739** passed, 0 failed,
1 ignored (default features). The 1 remaining ignored
test is unrelated to 4.3 -- it predates this work.

### Ground truth (session 93 close)

* **Head**: `868c378` (feat) on `main` before this
  close-doc commit lands; after both lands local main
  is 10 commits ahead of `origin/main` (sessions 89
  feat+close, 90 feat+close, 91 feat+close, 92
  feat+close, 93 feat+close). Verify via `git log
  --oneline origin/main..main` before any push. Do
  NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features). With `--features c2pa`:
  31 lib + 3 integration, 0 ignored.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A + B1 + B2 DONE**, B3 pending | 91 (A) / 92 (B1) / 93 (B2) / 94 (B3) |
| 4.8 | One-token-all-protocols | PLANNED | 95-96 |
| 4.5 | In-process AI agents | PLANNED | 97-100 |
| 4.4 | Cross-cluster federation | PLANNED | 101-103 |
| 4.6 | Server-side transcoding | PLANNED | 104-106 |
| 4.7 | Latency SLO scheduling | PLANNED | 107-108 |

Tier 4 item 4.3 grew from 3 sessions (post-92 split)
to 4 (post-93 split). Downstream items shift +1 vs.
session 92's view (e.g., 4.8 was 94-95, now 95-96).
Tier 4 budget unchanged at 27 sessions (85-111)
because the extension absorbs into the tier-wide
buffer.

### Session 94 entry point

**Tier 4 item 4.3 session B3: drain-task integration
+ admin verify route + E2E.**

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B3:

(a) **Broadcast-end lifecycle hook on
`lvqr_fragment::FragmentBroadcasterRegistry`**.
Current surface (line 102 of
`crates/lvqr-fragment/src/registry.rs`) has
`on_entry_created`; add a matching `on_entry_removed`
or a more general `LifecycleObserver` trait covering
both. Load-bearing primitive that 4.4 (cross-cluster
federation) + 4.5 (AI agents) will also consume --
**design the API shape before coding.** Specifically
decide:
  * Callback fires on `Drop` (risky -- callbacks from
    Drop can deadlock if they take locks the dropping
    thread holds; tokio runtime semantics in Drop are
    constrained) vs. explicit `registry.remove()`
    (safer but requires callers to know to remove).
  * Sync vs. async callback signature (the registry
    currently mixes both via `tokio::spawn` from
    callback closures -- consistent or split?).
  * Error propagation policy (callbacks panic-safe
    or panic-propagating).

(b) **Persist init bytes to disk at first-segment-
write time**. Today `FragmentBroadcaster::meta()`
holds them in memory only. Layout decision:
  * Flat `<archive>/<broadcast>/<track>/init.mp4` --
    simpler, parallel to the segment files,
    independently reachable for non-c2pa consumers.
  * `metadata.json` sidecar with the init bytes
    base64-encoded -- scales better if we later add
    per-track metadata (timescale, SPS/PPS,
    codec_string, etc.).

  Pick + document in B3's feat commit.

(c) **Extend `lvqr_cli::archive::
BroadcasterArchiveIndexer::drain`** to call
`lvqr_archive::provenance::finalize_broadcast_signed`
inside `tokio::task::spawn_blocking` when the drain
task terminates AND `C2paConfig` is `Some`. The B2-
landed orchestrator is one call: pass init bytes
(read from the layout decided in (b)), segment paths
(walk the redb index for this `(broadcast, track)`
in `start_dts` order), format (`"video/mp4"` for
CMAF), asset path
(`<archive>/<broadcast>/<track>/finalized.mp4`), and
manifest path (`finalized.c2pa`).

(d) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from disk, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes.

(e) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts a
`TestServer` with `C2paConfig` (using EphemeralSigner-
generated PEMs written to disk -- or, alternatively,
we expose a `C2paSignerConfig` enum that lets the
test pass an in-memory signer); publishes one RTMP
broadcast; drops the publisher to trigger finalize;
hits `GET /playback/verify/{broadcast}` and asserts
the JSON has `valid: true` (or expected
verification status given an ephemeral CA) + the
expected signer.

  Note on the E2E cert path: in production the
  operator points `C2paConfig.signing_cert_path` at a
  PEM file. For the E2E test we need to either (a)
  extract PEMs from EphemeralSigner via a
  `serialize_pem_pair() -> (cert_pem, key_pem)`
  helper added to `provenance` (would require c2pa-rs
  to expose them, which it does NOT -- the PEMs are
  built inside `EphemeralSigner::new` and not stored
  on the struct), or (b) extend `C2paConfig` with a
  `Signer` trait-object alternative, or (c) replicate
  EphemeralSigner's chain-generation logic ourselves
  (substantial new code). Decide before writing the
  E2E.

Expected scope: ~600-800 LOC (registry hook + init
persistence + drain integration + verify route + E2E
+ docs). Biggest risks:
- Registry lifecycle-hook API design affects 4.4 +
  4.5; budget time for prose-sketch + review before
  wiring.
- Cert-path-for-E2E decision (above).
- The drain-task termination path runs inside tokio;
  `finalize_broadcast_signed` is sync so it needs
  `spawn_blocking` like `write_segment` does.

Pre-session checklist:
- Read `tracking/TIER_4_PLAN.md` section 4.3 row B3
  fully.
- Sketch the registry lifecycle-hook API in prose +
  paste into the feat commit before wiring -- shared
  primitive for Tier 4 items 4.4 + 4.5 too.
- Decide init-bytes layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document.
- Decide E2E cert path (operator-shape PEM file vs.
  Signer-trait-object extension to `C2paConfig`).
- Confirm `c2pa::Reader::from_manifest_data_and_stream`
  is the right verify entry; check signature in
  c2pa 0.80 source.

## Session 92 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session B1: provenance composition
   primitives + trust-anchor config + plan split**
   (`6ca1889`). Two code deliverables plus a plan
   refresh that re-scopes B from one big session to
   two. Session-88 A1 precedent: honest acknowledgment
   that four independent surfaces in one session is
   too much.

   **B scope split rationale**. Original session 92 B
   combined four deliverables:
   (a) cert-chain fixture (debug c2pa's structural
       profile check OR vendor PKI; isolated),
   (b) finalize-asset orchestration (broadcast-end
       lifecycle hook on `FragmentBroadcasterRegistry`
       that 4.4 federation + 4.5 AI agents will also
       consume, plus init-bytes persistence + drain-
       task integration),
   (c) admin verify route (straightforward axum handler
       once the sign side wires up),
   (d) E2E that composes the above.
   Compressing them into one session risks bikeshedding
   the registry lifecycle API under E2E-failure
   pressure. Session B1 ships (a-prep) + the pure
   composition helpers that any caller needs; session
   B2 takes on the cross-crate orchestration + verify
   route + E2E.

   **Code landed**:

   * `C2paConfig.trust_anchor_pem: Option<String>`
     field. `sign_asset_bytes` routes it through
     `c2pa::Context::with_settings({"trust":
     {"user_anchors": ...}})` so operators with a
     private CA have a first-class path. This is the
     production workflow: point `trust_anchor_pem` at
     the CA bundle that issued the signing cert, and
     c2pa-rs's chain validator recognises it as a
     trust root.
   * `provenance::concat_assets(&[impl AsRef<Path>])
     -> Result<Vec<u8>, ArchiveError>`. Reads a
     caller-supplied ordered list of paths into one
     buffer. Session B2's finalize task walks the redb
     segment index in `start_dts` order, collects
     `PathBuf`s, and feeds them to this helper to
     produce the bytes-to-sign. Decoupling keeps the
     primitive redb-free and testable.
   * `provenance::write_signed_pair(asset_path,
     manifest_path, &SignedAsset) -> Result<(),
     ArchiveError>`. Writes both files with on-demand
     parent-dir creation, matching
     `writer::write_segment`'s semantics. Session B2
     lands
     `<archive>/<broadcast>/<track>/finalized.<ext>`
     +
     `<archive>/<broadcast>/<track>/finalized.c2pa`
     together.
   * 5 new unit tests in `provenance::tests`: concat
     order preservation, concat missing-path error
     naming, concat empty input, write_signed_pair
     parent-dir creation + overwrite semantics.

   **Cert-fixture debug outcome**. One time-boxed
   attempt this session to unignore the happy-path
   test via `Settings.trust.user_anchors` confirmed:
   that path addresses trust-chain validation only,
   not the structural-profile validation that is
   failing. c2pa 0.80's `verify.verify_trust`
   setting is `pub(crate)` so bypassing profile
   checks from outside the crate is not currently
   possible without either a c2pa upgrade or a light
   wrapper. Test docblock updated accordingly. Three
   viable fixture options remain for B2:
   (i) rcgen with full extension control (explicit
       AKI/SKI, basic-constraints criticality,
       validity window),
   (ii) vendored CA + leaf pair under
        `tests/fixtures/c2pa/` with a 2099 `notAfter`
        + README noting expiry,
   (iii) a test-only feature that wraps c2pa's
         `CertificateTrustPolicy::passthrough()`.

2. **Plan refresh** (same commit as item 1).
   `tracking/TIER_4_PLAN.md` section 4.3 re-headers
   from "2 sessions, 91-92" to "3 sessions, 91-93".
   Session 92 B row split into B1 DONE + B2 pending
   with expanded scope. Risks section unchanged.

3. **Session 92 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 5 | `provenance::tests::*` in `crates/lvqr-archive/src/provenance.rs` (feature-gated on `c2pa`) -- concat order, missing-path error, empty input, write_signed_pair parent-dir creation + overwrite | ok (run on the `archive-c2pa` CI cell + locally with `--features c2pa`) |

Totals: `cargo test -p lvqr-archive`: **26** (unchanged
on default features). `cargo test -p lvqr-archive
--features c2pa`: **31** lib (+5 from session 91) + 1
integration + 1 ignored. Workspace total: **739**
(unchanged; feature-gated tests do not count toward
default-feature workspace).

### Ground truth (session 92 close)

* **Head**: `6ca1889` (feat) on `main` before this
  close-doc commit lands; after it lands local main
  is 8 commits ahead of `origin/main` (sessions 89
  feat+close, 90 feat+close, 91 feat+close, 92
  feat+close). Verify via `git log --oneline
  origin/main..main` before any push. Do NOT push
  without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean. `cargo
  test -p lvqr-archive --features c2pa` green (31
  lib + 1 c2pa_sign + 1 ignored).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A + B1 DONE**, B2 pending | 91 (A) / 92 (B1) / 93 (B2) |
| 4.8 | One-token-all-protocols | PLANNED | 94-95 |
| 4.5 | In-process AI agents | PLANNED | 96-99 |
| 4.4 | Cross-cluster federation | PLANNED | 100-102 |
| 4.6 | Server-side transcoding | PLANNED | 103-105 |
| 4.7 | Latency SLO scheduling | PLANNED | 106-107 |

Tier 4 item 4.3 grew from 2 sessions to 3 at session
92's replan. Downstream items shift +1 (e.g., 4.8 was
93-94, now 94-95). Tier 4 budget unchanged at 27
sessions (85-111) because the extension absorbs into
the tier-wide buffer (same pattern 4.1 followed at
session 88).

### Session 93 entry point

**Tier 4 item 4.3 session B2: cert fixture +
finalize-asset orchestration + admin verify route +
E2E.**

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B2:

(a) **Cert-chain fixture** so the happy-path
`c2pa_sign::sign_asset_bytes_emits_non_empty_c2pa_
manifest_for_minimal_jpeg` test unignores. Three
options (ranked by likelihood-to-work):

  * rcgen with explicit extension control. Needs
    `rcgen::CustomExtension` for AKI/SKI content +
    basic-constraints criticality. Investigate which
    branch of c2pa's cert profile check rejects the
    current rcgen chain by enabling c2pa's
    `validation_log` or running c2pa's own tests in
    isolation to confirm what a passing cert looks
    like. Ideally the shortest path.
  * Vendor a static CA + leaf PEM pair with a 2099
    `notAfter`. Cleanest long-term: removes the
    rcgen dev-dep for this test + removes fixture
    construction flakiness. Generate once via `openssl
    req -new -x509 ...` (or a trusted CA fixture from
    c2pa-rs's own test suite if it has a reusable
    bundle) and commit under
    `crates/lvqr-archive/tests/fixtures/c2pa/` with a
    README noting the expiry.
  * Wrap `c2pa::CertificateTrustPolicy::passthrough()`
    behind a test-only feature. Problem: c2pa 0.80's
    `verify.verify_trust` setting is `pub(crate)` so
    this requires either upstreaming a PR or waiting
    on a c2pa version with public access. Last
    resort.

(b) **Finalize-asset orchestration**. Three moving
pieces:

  * Add a broadcast-end lifecycle hook to
    `lvqr_fragment::FragmentBroadcasterRegistry`.
    Current surface has `on_entry_created` (line 102
    of `crates/lvqr-fragment/src/registry.rs`); add a
    matching `on_entry_removed` or a more general
    `LifecycleObserver` trait that also covers
    `on_entry_created`. This is a load-bearing
    primitive that 4.4 (cross-cluster federation)
    and 4.5 (AI agents) will also want -- design the
    API shape before coding. Specifically think about
    whether the callback fires synchronously on drop
    (risky, callbacks from Drop can deadlock) or on
    an explicit `registry.remove()` call (safer but
    requires callers to know to remove).
  * Persist init bytes to disk at first-segment-write
    time. Today `FragmentBroadcaster::meta()` holds
    them in memory only. Layout decision: flat
    `<archive>/<broadcast>/<track>/init.mp4` vs.
    `metadata.json` sidecar. The flat approach is
    simpler but the JSON sidecar scales better if we
    later add timescale / SPS / PPS metadata. Pick
    and document in the B2 feat commit.
  * Extend `lvqr_cli::archive::BroadcasterArchiveIndexer::
    drain` to call `lvqr_archive::provenance::
    concat_assets` (walking the redb index for this
    `(broadcast, track)` in `start_dts` order and
    prepending the init bytes) + `sign_asset_bytes` +
    `write_signed_pair` when the drain task
    terminates AND `C2paConfig` is `Some`.

(c) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from
`<archive>/<broadcast>/<track>/finalized.<ext>` +
`.c2pa`, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes.

(d) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts a
`TestServer` with `C2paConfig` pointed at the
session-B2 cert fixture; publishes one RTMP
broadcast; drops the publisher to trigger finalize;
hits `GET /playback/verify/{broadcast}` and asserts
the JSON has `valid: true` + the expected signer.

Expected scope: ~600-900 LOC (cert fixture + three
archive-side changes + CLI route + E2E test).
Biggest risks:
- Registry lifecycle-hook API design affects 4.4 +
  4.5; worth a short design sketch before coding.
- Cert-fixture branch identification may still be
  non-obvious even after enabling validation_log;
  budget 1-2 hours for that alone.
- The drain-task termination path runs inside tokio;
  `write_signed_pair` is sync so it needs
  `spawn_blocking` like `write_segment` does.

Pre-session checklist:
- Read `tracking/TIER_4_PLAN.md` section 4.3 fully.
- Run `cargo test -p lvqr-archive --features c2pa
  --test c2pa_sign -- --ignored --nocapture` with
  any trace-logging added to c2pa's validation_log
  to pinpoint the specific profile-check branch
  that rejects the rcgen chain.
- Decide cert-fixture path (rcgen / vendored /
  passthrough) before coding the verify route.
- Decide finalize-asset layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document in the feat
  commit.
- Sketch the registry lifecycle-hook API shape in
  prose in the feat commit before wiring -- this is
  a shared primitive for Tier 4 items 4.4 + 4.5 too.

## Session 91 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.3 session A: C2PA feature +
   `provenance::sign_asset_bytes` primitive + plan
   refresh** (`1c34428`). Two deliverables in one
   commit, session-88-A1 style: a legitimate code
   landing plus the plan rewrite that makes sense of
   the landing's scope.

   **Plan-vs-code delta** captured in the refreshed
   `tracking/TIER_4_PLAN.md` section 4.3: the session-84
   plan said "on `finalize()` (broadcaster disconnect),
   the archive emits a C2PA manifest ... of the
   finalized MP4 bytes". The actual architecture has no
   finalize event, no init.mp4 on disk, and no single
   finalized MP4 -- the archive is a redb-indexed stream
   of `.m4s` fragments under
   `<archive_dir>/<broadcast>/<track>/`. "Sign the
   finalized MP4" has no referent today. A scout via the
   Explore agent confirmed three specifics:
   `BroadcasterArchiveIndexer::drain` exits silently on
   `FragmentStream::next_fragment` returning `None`,
   `FragmentBroadcasterRegistry` has `on_entry_created`
   but no matching broadcast-end hook, and
   `FragmentBroadcaster::meta()` holds init bytes in
   memory only. The refreshed plan re-scopes B to absorb
   the finalize-asset construction (init-bytes
   persistence + registry lifecycle hook + segment
   concatenation by dts) alongside the admin verify
   route + E2E. 4.3 stays at 2 sessions total.

   **Primitive** lives in a new
   `crates/lvqr-archive/src/provenance.rs` (~200 LOC)
   behind the `c2pa` feature (default off). Workspace
   pin `c2pa = { version = "0.80", default-features =
   false, features = ["rust_native_crypto"] }` so the
   crypto closure stays pure-Rust (no vendored OpenSSL
   C build) and the remote-manifest HTTP stacks
   (reqwest + ureq) are absent. Public surface:

   * `C2paConfig` -- cert path, key path, creator
     name, alg, optional TSA URL.
   * `C2paSigningAlg` -- LVQR-owned enum 1:1 with
     `c2pa::SigningAlg` so downstream consumers do not
     need a direct c2pa-rs dep to build a config.
   * `SignedAsset { asset_bytes, manifest_bytes }` --
     sidecar-mode output; asset passes through
     unchanged via `Builder::set_no_embed(true)`.
   * `sign_asset_bytes(&config, format, bytes)` --
     bytes-in / bytes-out primitive. Uses the non-
     deprecated `Builder::from_context(Context::new())
     .with_definition(manifest_json)` path (0.80
     deprecated `Builder::from_json`). Manifest carries
     one `stds.schema-org.CreativeWork` assertion
     whose `Person.name` is `config.assertion_creator`,
     constructed via `serde_json::json!` so operator-
     supplied names are JSON-escaped correctly.

   `ArchiveError::C2pa(String)` variant feature-gated
   so downstream consumers without c2pa do not see a
   dead variant.

   **Integration test** at
   `crates/lvqr-archive/tests/c2pa_sign.rs` gated on
   `#![cfg(feature = "c2pa")]`:

   * Error path live: `sign_asset_bytes_reports_c2pa_
     error_on_missing_cert_file` asserts missing-cert
     surfaces as `ArchiveError::Io` with the path in
     the message. Proves the primitive reads config +
     surfaces errors cleanly.
   * Happy path `#[ignore]`'d: c2pa-rs 0.80 validates
     the signing cert against C2PA spec §14.5.1 at
     sign time and rejects the rcgen-generated chain
     (even with a 2-cert CA + leaf using
     emailProtection EKU + digitalSignature KU) with
     the generic `CertificateProfileError::
     InvalidCertificate`. That variant collapses ~8
     failure branches without a validation_log hook at
     this API layer, so pinpointing the exact missing
     extension takes more iteration than session A
     budgets for. The test's doc comment documents
     three unignore paths for session B: (a) rcgen
     with full extension control, (b) vendored
     fixture with 2099 `notAfter`, (c) passthrough
     trust policy behind a new
     `c2pa-test-bypass-cert-check` feature.

   **CI**: new `archive-c2pa` job on `ubuntu-latest`
   runs `cargo clippy + cargo test -p lvqr-archive
   --features c2pa`. Separate job rather than a matrix
   cell on the existing `test` job so macOS CI time
   does not grow by ~2 minutes (c2pa-rs pulls ~20
   transitive crates; all pure-Rust with our
   default-features-off config).

2. **Session 91 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 1 | `sign_asset_bytes_reports_c2pa_error_on_missing_cert_file` in `crates/lvqr-archive/tests/c2pa_sign.rs` | ok (feature-gated; runs on the `archive-c2pa` CI job + locally with `--features c2pa`) |
| 0 (ignored) | `sign_asset_bytes_emits_non_empty_c2pa_manifest_for_minimal_jpeg` | `#[ignore]`'d pending session B's cert fixture |

Workspace totals on macOS: **739** passed, 0 failed,
1 ignored. Feature-gated c2pa test does not count
toward the default-feature workspace total; it adds
+1 passed / +1 ignored when the `c2pa` feature is on.

### Ground truth (session 91 close)

* **Head**: `1c34428` (feat) on `main` before this
  close-doc commit lands; after it lands local main
  is 6 commits ahead of `origin/main` (sessions 89
  feat + close, 90 feat + close, 91 feat + close).
  Verify via `git log --oneline origin/main..main`
  before any push. Do NOT push without direct user
  instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS (default features).
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  c2pa --all-targets -- -D warnings` clean on macOS.
  `cargo test -p lvqr-archive --features c2pa` green
  (26 lib + 1 c2pa_sign + 1 ignored).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 / 89 / 90 |
| 4.3 | C2PA signed media | **A DONE**, B pending | 91 (A) / 92 (B) |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

### Session 92 entry point

**Tier 4 item 4.3 session B: cert fixture +
finalize-asset construction + admin verify route +
E2E.** Absorbed scope from the session-84 plan's
session B + session A's deferred items per the
session-91 re-scope.

Deliverables per the refreshed
`tracking/TIER_4_PLAN.md` section 4.3 row B:

(a) **Cert-chain fixture** so the happy-path
`c2pa_sign::sign_asset_bytes_emits_non_empty_c2pa_
manifest_for_minimal_jpeg` test unignores. Pick one
of three paths documented in the test's doc comment:

  * rcgen with full extension control (explicit AKI/
    SKI, basic-constraints criticality, explicit
    validity window). Requires digging into which of
    the ~8 failure branches in
    `CertificateProfileError::InvalidCertificate` is
    tripping. Enable c2pa-rs's `validation_log` or
    build a scratch binary that prints the log to
    debug.
  * Vendored test CA + end-entity under
    `crates/lvqr-archive/tests/fixtures/c2pa/` with a
    far-future `notAfter` (2099-era) and a README
    noting the expiry. Cleanest long-term: removes
    the rcgen dev-dep for this test entirely and
    removes fixture-construction flakiness.
  * `CertificateTrustPolicy::passthrough()` behind a
    new `c2pa-test-bypass-cert-check` feature. Lets
    the test run end-to-end without production-grade
    PKI. Caveat: the primitive signs with a trust-
    bypassed policy, so the test no longer validates
    that the cert profile is compliant -- the
    primitive may let bad certs through at sign time
    in production if the feature leaks. Mark the
    feature loudly.

(b) **Finalize-asset construction** in
`lvqr-archive` + the CLI drain task. Three moving
pieces:

  * Persist init bytes to disk at first-write time.
    Today `FragmentBroadcaster::meta()` holds them
    in memory only. Options: write once when the
    first segment lands, at
    `<archive_dir>/<broadcast>/<track>/init.mp4`;
    or generalise the on-disk layout to include a
    `metadata.json` sidecar per `(broadcast,
    track)` with the init bytes base64-encoded in
    it. Decide + document in B's feat commit.
  * Broadcast-end lifecycle hook on
    `FragmentBroadcasterRegistry`. Currently the
    registry exposes `on_entry_created` only; add a
    matching `on_entry_removed` (or a more general
    `LifecycleObserver`) so the drain-task-
    termination path can notify listeners. This is
    a shared primitive -- future sessions (4.4
    federation, 4.5 AI agents) may also want to
    react to broadcast-end events.
  * Segment-concat helper in `lvqr-archive` that
    produces the bytes to feed to
    `sign_asset_bytes`. Walks the redb index for
    the broadcast + track, reads segments in
    start_dts order, concatenates with the init
    bytes, returns a `Vec<u8>`. At today's archive
    segment sizes (<= 1 MiB) the in-memory buffer
    is fine; if that ever grows too large we swap
    to a streaming `impl Read + Seek`.

(c) **`GET /playback/verify/{broadcast}`** admin
route in `lvqr-cli`. Reads the signed asset +
sidecar manifest from disk, calls
`c2pa::Reader::from_manifest_data_and_stream`,
returns a JSON object `{ signer: String, signed_at:
Option<DateTime>, valid: bool, errors: Vec<String>
}`. Auth per existing `/admin` routes (admin
token).

(d) **E2E test** at
`crates/lvqr-cli/tests/c2pa_verify_e2e.rs`. Starts
a `TestServer` with `C2paConfig` pointed at the
session-B cert fixture; publishes one RTMP
broadcast; drops the publisher to trigger
finalize; hits `GET /playback/verify/{broadcast}`
and asserts the JSON has `valid: true` + the
expected signer.

Expected scope: ~500-800 LOC (cert fixture + three
archive changes + CLI route + E2E test + docs
section). Biggest risk: the lifecycle-hook addition
to `FragmentBroadcasterRegistry` is a load-bearing
primitive that future items will also consume, so
the API shape is worth a short design discussion
before coding. Second risk: the cert-fixture branch
identification may still be non-obvious even with
validation_log enabled; budget 1-2 hours for that
alone.

Pre-session checklist:

- Read `tracking/TIER_4_PLAN.md` section 4.3 top-to-
  bottom (now accurate post-session-91 refresh).
- Run `cargo test -p lvqr-archive --features c2pa
  --test c2pa_sign -- --ignored --nocapture` and
  read the full c2pa error output -- that narrows
  which profile branch is tripping before any code
  changes.
- Decide cert-fixture path (rcgen / vendored /
  passthrough feature) before coding the verify
  route; the route's test depends on the fixture
  choice.
- Decide finalize-asset layout (flat `init.mp4` vs.
  `metadata.json` sidecar) and document in the feat
  commit.

## Session 90 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session B: criterion bench +
   deployment operator doc** (`bbe2757`). Last piece of
   item 4.1 after A1 extracted the writer (session 88)
   and A2 added the feature-gated tokio-uring path
   (session 89). The caller-facing API
   (`write_segment(archive_dir, broadcast, track, seq,
   payload) -> Result<PathBuf, ArchiveError>`) is
   unchanged; B is purely measurement + documentation.

   `crates/lvqr-archive/benches/io_uring_vs_std.rs` (~95
   LOC). criterion 0.5, parameterised on segment size
   across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` -- span
   chosen to cover the production fragment distribution
   (AAC AU through high-bitrate keyframe). Uses
   `BenchmarkId::from_parameter` + `Throughput::Bytes`
   so criterion reports per-variant throughput + latency.
   `measurement_time = 2s`, `sample_size = 30` caps a
   full run at ~8 s wall + ~1 GB of tempdir writes on
   the top variant; operators raise the cap from the CLI
   when they want tighter CIs.

   The harness does not cfg-gate itself. `write_segment`
   handles path selection internally, so the same bench
   file exercises std::fs on macOS + Windows (smoke test
   for harness health) and the tokio-uring path on
   Linux with `--features io-uring`. The std-vs-io-uring
   comparison is criterion's saved-baseline workflow
   (`--save-baseline std` + `--baseline std`), which is
   called out in the docs section verbatim.

   One TempDir per variant; seq counter rolls forward
   per iter so writes land on distinct files (matches
   the production monotonic-seq contract). `TMPDIR=
   /dev/shm` is explicitly marked anti-pattern in the
   bench doc-comment: tmpfs bypasses the block-device
   IO scheduler and hides the very effect the bench is
   measuring.

   `docs/deployment.md` gains a new 153-line "Archive:
   `io_uring` write backend (Linux-only)" section
   between "Upgrade strategy" and "Firewall hardening
   checklist". Covers when to enable (Linux + kernel
   5.6 + non-seccomp-restricted runtime; not for
   bursty-small workloads), how to enable (rebuild with
   `--features lvqr-archive/io-uring`; compile-time
   only, no runtime flag), how to measure (the criterion
   saved-baseline workflow with TMPDIR guidance), how
   to interpret (throughput delta + p99 on 256 KiB + 1
   MiB is the enable signal; 4 KiB regression means
   leave it off until session-B-scope follow-up
   promotes the writer to option (b)), the exact
   `OnceLock` cold-start `tracing::warn!` operator
   runbook (seccomp profile check, LimitMEMLOCK,
   gVisor/Kata carve-outs), and caveats (
   `create_dir_all` stays on std::fs, reader path stays
   on `tokio::fs`, ordering contract unchanged).

   **No Linux io_uring numbers committed.** The plan
   said "cite the numbers" but numbers captured on one
   machine are not portable to another (different CPUs,
   kernels, block devices yield materially different
   results). Committing numbers from this macOS dev box
   would misrepresent Linux production performance;
   committing numbers from a specific cloud instance
   would misrepresent self-hosted + bare-metal
   performance. The docs section drives the
   capture-your-own workflow instead. macOS smoke-run
   numbers (4 KiB: ~79 us; 1 MiB: ~940 us / ~1 GiB/s
   throughput) are noted in the feat commit message as
   evidence the harness is healthy end-to-end; they are
   not quoted in operator-facing docs.

   Plan refresh: section 4.1 header flipped to
   `**COMPLETE (sessions 88-90)**`; session B row
   flipped to `**DONE (session 90)**`. Opportunistic
   hygiene: the inline session-decomposition table for
   4.3 was still numbered 90/91 from before session 88
   split 4.1 into three sub-sessions; corrected to
   91/92 so the next item starts from a consistent
   baseline.

2. **Session 90 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 0 | Benches do not add test count. The bench harness was smoke-run on macOS with `--measurement-time 1 --sample-size 10 --warm-up-time 1`; all four segment-size variants produced plausible numbers. |

Total workspace tests on macOS: **739**, unchanged
from session 89. `cargo bench -p lvqr-archive --no-run`
compiles clean; `cargo clippy --workspace --all-targets
--benches -- -D warnings` includes the new bench in
scope and is clean.

### Ground truth (session 90 close)

* **Head**: `bbe2757` (feat) on `main` before this
  close-doc commit lands; after it lands local main is
  4 commits ahead of `origin/main` (session 89 feat +
  session 89 close + session 90 feat + session 90
  close). Verify via `git log --oneline
  origin/main..main` before any push. Do NOT push
  without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo bench -p lvqr-archive --no-run`
  compiles clean.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **COMPLETE** | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91 (A) / 92 (B) |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

Three Tier 4 items are now known-state (4.1 DONE, 4.2
DONE, 4.3 PLANNED with a known entry point). Tier 4
budget is unchanged at 27 sessions (85-111); the 4.1
extension from 2 to 3 sessions absorbed cleanly into
the tier-wide buffer at session 88's replan.

### Session 91 entry point

**Tier 4 item 4.3 session A: C2PA finalize-time
signing hook in `lvqr-archive`.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.3:

1. Add `c2pa-rs` to workspace deps (pin a specific 0.x
   version; `c2pa-rs` is pre-1.0 so any minor upgrade
   gets its own session). `tracking/TIER_4_PLAN.md`'s
   "Dependencies to pin" table at the bottom of the
   file has the target-version placeholder.
2. `lvqr-archive` gains a `C2paConfig` struct:
   `signing_cert_path`, `private_key_path`,
   `assertion_creator`. The config is optional at the
   crate boundary; when `None`, archive finalize
   behaves exactly as it does today (no signing, no
   manifest emission).
3. On `finalize()` (broadcaster disconnect), the
   archive emits a C2PA manifest asserting authorship
   + the SHA-256 of the finalized MP4 bytes. The
   manifest lives adjacent to the finalized file --
   layout decision up to session A, but
   `<archive_dir>/<broadcast>/<track>/manifest.c2pa`
   is the obvious starting point.
4. Integration test: `cargo test -p lvqr-archive
   --test c2pa_sign` hits a fixture cert + key pair
   (bundle in `crates/lvqr-archive/tests/fixtures/`),
   exercises the sign path, reads the manifest back
   via `c2pa-rs`'s reader, and asserts the author +
   content hash.
5. Anti-scope for A: no admin verify route (that is
   session B), no operator-supplied PKI (MVP uses
   `c2pa-rs` bundled Adobe test CA), no live
   signed-as-you-go manifests (file-at-rest only,
   covers the legal-discovery / broadcast-archive /
   journalism use cases the plan names).

Expected scope: ~250-400 LOC (C2paConfig struct +
finalize hook + fixture cert/key + integration test +
`docs/security.md` or similar pointer section; plus a
workspace dep pin). Biggest risk: the `c2pa-rs` API is
still pre-1.0 and may require an adapter if the shape
does not match the plan's mental model; if so,
session A surfaces that + the adapter is worth
carrying into session B as shared infrastructure.

Pre-session checklist:

- Read `tracking/TIER_4_PLAN.md` section 4.3 top-to-
  bottom. It is short (the whole section is ~40
  lines); no staleness risk comparable to 4.1's
  session-88 replan but worth confirming.
- Check `c2pa-rs` on crates.io for the current 0.x
  version. If it is a large jump from whatever the
  plan targeted, pin to the tested-compatible version
  and note the upgrade as follow-up work.
- Decide on the manifest-on-disk layout before coding
  (flat `manifest.c2pa` next to the final MP4, vs.
  embedded in a sidecar JSON, vs. manifested into the
  MP4 bytes themselves). The plan does not prescribe;
  pick + document in the feat commit.

## Session 89 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session A2: feature-gated
   tokio-uring write path** (`8c71f8c`). One-file body
   swap inside `lvqr_archive::writer` per the A1
   contract. Cross-crate call shape unchanged:
   `lvqr_cli::archive::BroadcasterArchiveIndexer::drain`
   still calls `write_segment(archive_dir, broadcast,
   track, seq, payload)` inside `tokio::task::
   spawn_blocking` and records the returned `PathBuf`
   on the matching `SegmentRef::path`. The io-uring
   path is invisible to callers.

   `Cargo.toml` (workspace) gains a
   `tokio-uring = "0.5"` pin next to the Tier 4 4.2
   `wasmtime` + `notify` pins. Declared once at the
   workspace level so the version is a single-file
   bump. `crates/lvqr-archive/Cargo.toml` pulls it in
   only under `[target.'cfg(target_os = "linux")'.
   dependencies]` with `optional = true`, and a new
   default-off `io-uring` feature activates it via
   `dep:tokio-uring`. macOS + Windows builds never
   resolve or compile tokio-uring; the feature is
   accepted as a no-op on non-Linux because the
   runtime code paths are gated
   `cfg(all(target_os = "linux", feature = "io-uring"))`.

   `crates/lvqr-archive/src/writer.rs`:
   `write_segment`'s outer signature
   (`fn(archive_dir, broadcast, track, seq, payload)
   -> Result<PathBuf, ArchiveError>`) is unchanged.
   The body splits into `write_payload_std` (always
   present; wraps `std::fs::write`) and
   `write_payload_io_uring` (Linux + feature; wraps
   `tokio_uring::start` inside
   `std::panic::catch_unwind`). `create_dir_all` stays
   on `std::fs` because tokio-uring 0.5 exposes no
   mkdir primitive; the archive tree is amortised
   across thousands of segments per broadcast so the
   extra syscall is noise.

   Fallback design: tokio-uring 0.5's
   `tokio_uring::start` calls
   `runtime::Runtime::new(&builder()).unwrap()`
   internally, with no fallible variant on
   `Builder::start` either. `catch_unwind` is the only
   way to observe a kernel-side setup failure (kernel
   < 5.6, seccomp / sandbox without `io_uring_*`
   syscalls) without aborting the process. A
   process-global `static IO_URING_AVAILABLE:
   OnceLock<bool>` traps the first setup failure, emits
   a single `tracing::warn!`, and latches
   `std::fs::write` for the rest of the process.
   On-path `io::Error`s from `File::create` /
   `write_all_at` / `sync_all` / `close` after the
   runtime comes up surface as `ArchiveError::Io`
   without tripping the latch, so the next segment
   retries io_uring cleanly.

   New CI job `archive-io-uring` in
   `.github/workflows/ci.yml`: `cargo clippy -p
   lvqr-archive --features io-uring --all-targets --
   -D warnings` + `cargo test -p lvqr-archive
   --features io-uring` on `ubuntu-latest`. Separate
   job rather than a matrix cell on the existing
   `test` job so macOS CI time does not grow. The
   existing ubuntu + macos matrix on the default
   feature path is unchanged.

   Plan refresh (`tracking/TIER_4_PLAN.md` section
   4.1): A2 row flipped to **DONE (session 89)** with
   the shipped-option note. Risks section gains a
   bullet documenting the `tokio_uring::start`
   panic-on-setup nuance so session B knows the
   `catch_unwind` is deliberate and not a bug.

2. **Session 89 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 1 | `writer::tests::write_segment_io_uring_matches_std_bytes` in `lvqr-archive/src/writer.rs` | cfg-gated on `all(target_os = "linux", feature = "io-uring")`; runs on the new `archive-io-uring` CI job only. Asserts byte-identity vs. the payload + that the OnceLock fallback latch did NOT trip (a trip on a recent kernel signals an environmental problem, not a code bug). |

Total workspace tests on macOS: **739** (unchanged
from session 88; the io-uring test is cfg-gated out
locally). The Linux `archive-io-uring` job adds one
additional test to the Linux-specific count.

### Ground truth (session 89 close)

* **Head**: `8c71f8c` (feat) on `main` before this
  close-doc commit lands; after both commits local
  main is 2 commits ahead of `origin/main` at session
  89 close. Verify via
  `git log --oneline origin/main..main` before any
  push. Do NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored on
  macOS.
* **CI gates locally clean**: `cargo fmt --all --
  --check`, `cargo clippy --workspace --all-targets
  --benches -- -D warnings`, `cargo test --workspace`
  all green. `cargo clippy -p lvqr-archive --features
  io-uring --all-targets -- -D warnings` also green
  on macOS (the feature is a compile-time no-op on
  non-Linux so clippy is still meaningful cover for
  the std path under the feature flag).
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **A1 + A2 DONE**, B pending | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91-92 |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

### Runtime-integration findings (for session 90 B)

Per the plan note, A2 ships option (a) (per-segment
`tokio_uring::start` inside `spawn_blocking`) and
leaves option (b) (persistent current-thread runtime
pinned to a dedicated writer thread) for B to decide
based on criterion numbers. A few observations the
bench should carry forward:

* **Per-call runtime setup cost is the variable to
  measure.** Each `tokio_uring::start` constructs a
  fresh io_uring submission queue + completion queue
  pair (default entries from
  `tokio_uring::builder()`). On a 4 KiB unit-test
  payload this is not visible but on a 64 KiB segment
  the setup may still dominate the actual write. The
  bench at `crates/lvqr-archive/benches/
  io_uring_vs_std.rs` should parameterise segment size
  across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` so the
  crossover point is visible.

* **`catch_unwind` is in the hot path.** Session B
  should measure the cost of the `AssertUnwindSafe`
  wrapper + the catch_unwind call itself, not just
  the io_uring submission. If the overhead is
  non-trivial, an alternative is to do the probe once
  via a dedicated "io_uring availability" check at
  startup, set the latch to the outcome, and skip the
  `catch_unwind` on every subsequent call. This is a
  follow-up for B's write-up, not an A2 change.

* **The OnceLock fallback has not been observed in
  test.** The new `write_segment_io_uring_matches_std_bytes`
  test asserts the latch is NOT `Some(false)` on a
  recent-kernel runner. If the Linux CI job ever
  reports a latch trip, it almost certainly means the
  GitHub Actions image dropped `io_uring_*` from the
  default seccomp profile (has happened historically
  with container runtimes) rather than a code bug.
  Document the failure mode in B's
  `docs/deployment.md` section so operators know what
  a cold-start `tracing::warn!` from lvqr-archive
  means in production.

* **`create_dir_all` staying on std::fs is a
  principled choice, not a shortcut.** tokio-uring 0.5
  has no mkdir / mkdirat primitive. The archive tree
  is `<root>/<broadcast>/<track>/` and segments live
  under the `<track>` leaf, so the tree-creation cost
  is O(broadcasts * tracks) while segment writes are
  O(broadcasts * tracks * segments_per_track); for any
  DVR window longer than a few seconds the mkdir cost
  is negligible. If `io_uring_mkdirat` lands upstream
  this can be revisited, but it is explicitly
  anti-scope for session B.

### Session 90 entry point

**Tier 4 item 4.1 session B: criterion bench + docs.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.1
session B:

1. New bench `crates/lvqr-archive/benches/
   io_uring_vs_std.rs` under criterion. Compare
   `write_segment` throughput (MB/s) + p99 latency
   between the std::fs body and the io-uring body on
   a 1-hour synthetic broadcast. Parameterise segment
   size across `[4 KiB, 64 KiB, 256 KiB, 1 MiB]` so
   the crossover point is visible. Run via
   `cargo bench -p lvqr-archive --features io-uring`
   on Linux (macOS cannot exercise io_uring; the
   bench file needs a `cfg(all(target_os = "linux",
   feature = "io-uring"))` guard on its bench
   harness so macOS `cargo bench --workspace` does
   not fail).
2. `docs/deployment.md` gains a "when to enable the
   io_uring backend" section citing the bench
   numbers. Include the OnceLock fallback failure
   mode so operators recognise the cold-start
   `tracing::warn!`.
3. If the bench shows the per-segment
   `tokio_uring::start` setup cost dominates writes
   on small segments, plan-and-land option (b)
   (persistent current-thread runtime on a dedicated
   writer thread) as a session-B extension or a new
   session C. Leave it out of session B's first
   commit until the numbers force it.

Expected scope: ~250-400 LOC (bench + docs section +
any small refactors the bench surfaces). Biggest risk:
the bench result may show io-uring is net-negative on
small segments, in which case the default-off feature
is the right ship state and the docs section needs to
be honest about it.

## Session 88 close (2026-04-18)

### What shipped

1. **Tier 4 item 4.1 session A1: archive writer extraction +
   plan refresh** (`ec7ef01`). Pure refactor, no behavior
   change.

   New module `crates/lvqr-archive/src/writer.rs` (~170 LOC
   including 6 unit tests). Exposes
   `lvqr_archive::writer::write_segment(archive_dir, broadcast,
   track, seq, payload) -> Result<PathBuf, ArchiveError>` and
   `segment_path(archive_dir, broadcast, track, seq) -> PathBuf`,
   plus a private `SEGMENT_FILENAME_FMT_WIDTH = 8` constant that
   documents the canonical `<seq:08>.m4s` filename format.
   `write_segment` is synchronous (matches the previous
   `std::fs::create_dir_all` + `std::fs::write` behavior) and
   returns the resulting `PathBuf` on success so callers can
   record it on the matching `SegmentRef::path`. New
   `ArchiveError::Io(String)` variant.

   `lvqr-cli/src/archive.rs` refactored to call
   `lvqr_archive::writer::write_segment` from inside the existing
   `tokio::task::spawn_blocking` block. The caller-side
   `BroadcasterArchiveIndexer::segment_path` helper is deleted
   in favor of the crate-owned one. Behavior is unchanged: same
   layout, same sequence numbering, same UTF-8 path check before
   recording into redb, same fail-warn semantics on write error.
   `rtmp_archive_e2e` still green.

   Unit tests: segment path layout (broadcast/track/seq
   subdirs + 8-digit zero-pad), overflow past 8 digits is not
   truncated, `write_segment` creates missing parent dirs,
   `write_segment` is idempotent on the same `(broadcast,
   track, seq)` (overwrites the file), `write_segment` returns
   `ArchiveError::Io` when the archive root is a regular file
   instead of a directory.

   Crate doc (`lvqr-archive/src/lib.rs`) refreshed: the
   pre-session-59 comment claiming "Not a segment writer.
   That is in `lvqr-record`" was stale on both counts (the
   writer moved to `lvqr-cli` in session 59 and now lives in
   `lvqr-archive::writer`). Replaced with a "What this crate
   OWNS" block that calls out the index + the writer; the "NOT"
   block now only lists HTTP playback + transcoding +
   rotation.

2. **Plan refresh** (same commit as item 1).
   `tracking/TIER_4_PLAN.md` section 4.1 rewritten to reflect
   the session 59-60 architecture. Split the original session
   A into two sub-sessions:

   * **A1 (this session, DONE)**: writer extraction +
     `ArchiveError::Io` + plan refresh. No io-uring yet.
   * **A2 (session 89, pending)**: feature-gated `tokio-uring`
     path inside `lvqr_archive::writer::write_segment`.
     Linux-only. Runtime fallback on `tokio_uring::start`
     failure.

   The plan now documents the tokio-uring runtime-integration
   nuance the pre-session-88 plan was silent about: LVQR runs
   multi-thread tokio, but `tokio-uring` needs a current-thread
   runtime. Option (a) spin `tokio_uring::start` per segment
   inside `spawn_blocking`; option (b) pin a long-lived
   current-thread runtime to a dedicated writer thread. A2
   ships option (a); session B's bench decides whether (b)
   pays for itself.

3. **Session 88 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 6 | `writer::tests::*` in `lvqr-archive/src/writer.rs` | ok |

No new integration tests -- the refactor is pure substitution
and `rtmp_archive_e2e` + `playback_surface_honors_shared_auth`
already cover the cross-crate call path.

Total workspace tests: **739** (+6 from session 87's 733).

### Ground truth (session 88 close)

* **Head**: `ec7ef01` (refactor) on `main` before this
  close-doc commit lands; after it lands local main is
  several commits ahead of origin/main (session-87 feat +
  session-87 close + session-88 feat + this close doc all
  queued locally). Verify via
  `git log --oneline origin/main..main` before any push.
  Do NOT push without direct user instruction.
* **Tests**: **739** passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace
  --all-targets --benches -- -D warnings, test --workspace
  all green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | **A1 DONE**, A2 + B pending | 88 (A1) / 89 (A2) / 90 (B) |
| 4.3 | C2PA signed media | PLANNED | 91-92 |
| 4.8 | One-token-all-protocols | PLANNED | 93-94 |
| 4.5 | In-process AI agents | PLANNED | 95-98 |
| 4.4 | Cross-cluster federation | PLANNED | 99-101 |
| 4.6 | Server-side transcoding | PLANNED | 102-104 |
| 4.7 | Latency SLO scheduling | PLANNED | 105-106 |

Session numbering slipped by one because item 4.1 is now 3
sessions instead of 2; downstream item numbers shifted
accordingly. The plan-as-written budget (27 sessions total
for Tier 4) is unchanged because 4.1's extension comes out
of the original 3-session buffer.

### Session 89 entry point

**Tier 4 item 4.1 session A2: `io-uring` feature on
`lvqr_archive::writer::write_segment`.**

Deliverable per the refreshed
`tracking/TIER_4_PLAN.md` section 4.1 A2 row:

1. Add `tokio-uring = "0.5"` workspace dep, gated on
   `target_os = "linux"`. Pin the exact version.
2. Add `io-uring` feature to `lvqr-archive` (default off).
   When on + target is Linux, `write_segment` spins a
   short-lived `tokio_uring::start` inside its body and
   issues `tokio_uring::fs::File::create` +
   `write_all_at`. The caller stays unchanged; the
   `spawn_blocking` + sync-call shape survives.
3. Runtime fallback: if `tokio_uring::start` fails (kernel
   < 5.6, container sandbox without io_uring syscalls),
   log a `warn` once per process and fall back to the
   `std::fs` path for this write and all subsequent ones.
   Session 89 A2 uses a `std::sync::OnceLock<bool>` for
   the feature-disabled latch so the first failure pins
   the fallback state and subsequent calls skip the probe.
4. No new unit tests for the io-uring path on macOS; a
   `#[cfg(target_os = "linux")]` integration test runs on
   a GitHub Actions `ubuntu-latest` job in
   `.github/workflows/ci.yml` as a separate cell.

Expected scope: ~200-350 LOC (module changes + CI cell +
workspace dep pin). Risk: the per-segment
`tokio_uring::start` overhead may dwarf the io_uring win
on small (<100 KB) segments; session B's bench is the
forcing function that decides whether to promote to a
persistent current-thread runtime in a follow-up.

## Session 87 close (2026-04-18)

### What shipped (1 feat commit + 1 close doc commit)

1. **Tier 4 item 4.2 session C: WASM filter hot-reload**
   (`2fc8196`). Full writeup in the feat commit message;
   synopsis here.

   New module `crates/lvqr-wasm/src/reloader.rs` (~250
   LOC including 3 unit tests). `WasmFilterReloader::spawn(path,
   filter)` canonicalises `path`, watches the **parent
   directory** via `notify::recommended_watcher` (parent-dir
   watch is the portable best practice: macOS FSEvents and
   Linux inotify both deliver rename-into-place events cleanly
   when the target file is replaced atomically; watching the
   file itself loses events on atomic saves). A background
   worker thread drains the `notify::Event` mpsc, filters by
   canonicalised target path + `EventKind::Create|Modify|Any`,
   debounces for 50 ms (`DEFAULT_DEBOUNCE`), re-runs
   `WasmFilter::load` on the path, and calls
   `SharedFilter::replace` on success. A compile failure logs
   a `tracing::warn` and keeps the previous module live.

   Atomic semantics documented at the top of `reloader.rs`:
   `SharedFilter::replace` takes the same `Mutex` that every
   `FragmentFilter::apply` call holds, so in-flight applies
   finish on the OLD module and the very next apply observes
   the NEW module. No partial-state visibility.

   `Drop` ordering matters: sends the shutdown signal, **then
   drops the watcher** (which closes the `mpsc::Sender` in the
   notify callback and wakes the worker out of its blocking
   `recv()`), **then** `join()`s the worker. Without that
   ordering the join deadlocks. One design iteration: the
   first draft stored the watcher as a plain (non-`Option`)
   field and hung every reloader-bearing test's teardown for
   60+ seconds until the fix landed.

   Example filter:
   `crates/lvqr-wasm/examples/redact-keyframes.{wat,wasm}`.
   The `.wasm` is 82 bytes, byte-identical across rebuilds,
   returns `-1` from `on_fragment` so every fragment is
   dropped. Paired with a new
   `cargo run -p lvqr-wasm --example build_fixtures`
   helper that walks `examples/*.wat` and regenerates
   the sibling `.wasm` files via `wat::parse_str`, so future
   sessions do not need `wat2wasm` or `wasm-tools` on PATH to
   rebuild either fixture. `frame-counter.wasm` round-trips
   byte-identical through the new helper so the session-86
   fixture is unchanged on disk.

   CLI integration: `lvqr-cli::start` now spawns a
   `WasmFilterReloader` alongside
   `install_wasm_filter_bridge` whenever `--wasm-filter` is
   set. `ServerHandle` gets a new
   `_wasm_reloader: Option<WasmFilterReloader>` field held
   solely for its `Drop` side effect. No new public API on
   `ServerHandle`; the reloader surfaces indirectly through
   the existing `WasmFilterBridgeHandle` counters which
   operators already watch to verify a deployed filter.

   Integration test at
   `crates/lvqr-cli/tests/wasm_hot_reload.rs` (~350 LOC).
   Seeds a tempdir `filter.wasm` with a copy of
   `frame-counter.wasm`, starts a `TestServer` pointed at
   it, publishes a real RTMP broadcast (`live/hot-reload-
   before`) via the proven `rml_rtmp` handshake +
   `ClientSession` pattern, asserts tap observed at least
   one fragment with `dropped == 0`, drops the RTMP session,
   atomically-renames `redact-keyframes.wasm` over
   `filter.wasm`, sleeps 500 ms for the watcher, publishes a
   second broadcast (`live/hot-reload-after`), and polls for
   `fragments_dropped > 0` on the new broadcast with a 10 s
   budget. Total wall-clock: ~1 s on a warm-cache Apple
   Silicon run.

2. **Test-contract script comment refresh** (folded into
   `2fc8196`). `scripts/check_test_contract.sh` still
   reports `lvqr-wasm` integration + E2E slots as missing
   because the tests live cross-crate in
   `lvqr-cli/tests/wasm_{frame_counter,hot_reload}.rs`
   (accepted case-by-case per `tests/CONTRACT.md`). Updated
   the inline comment to reflect session-87 reality: both
   integration tests now exist, and the educational warnings
   will remain until a future session either moves the tests
   in-tree or extends the script with a per-crate integration
   exemption mechanism. Fuzz + conformance slots stay open
   pending a WASM trap-surface fuzzer.

3. **Session 87 close doc** (`b4c2263`).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 3 | `reloader::tests::*` in `lvqr-wasm/src/reloader.rs` | ok |
| 1 | `wasm_filter_hot_reload_flips_drop_behavior_mid_stream` in `lvqr-cli/tests/wasm_hot_reload.rs` | ok |

Total workspace tests: **733** (+4 from session 86's 729).

### Ground truth (session 87 close)

* **Head**: `2fc8196` (feat) on `main` before the close-doc
  commit (`b4c2263`) landed. After both commits: local
  main was 2 commits ahead of origin/main on session 87
  close. Session 88 added 2 more (feat `ec7ef01` + close
  `8f1be03`), bringing the count to 4 commits ahead at
  session 88 close.
* **Tests**: **733** passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace
  --all-targets --benches -- -D warnings, test --workspace
  all green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status (session 87 view)

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **COMPLETE** (A + B + C DONE) | 85 (A) / 86 (B) / 87 (C) |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 88 entry point

**Tier 4 item 4.1 session A: io_uring archive writes.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.1 session A:

1. Feature-gated `tokio-uring` path for init + media segment
   writes in `lvqr-archive`. Feature `io-uring` off by
   default; Linux-only. Wire through
   `IndexingFragmentObserver::write_all` so archive segments
   go through `tokio-uring::fs` when the feature is on and
   `tokio::fs` otherwise.
2. Graceful runtime fallback: if `tokio_uring::start` fails
   (kernel < 5.6, container without io_uring syscalls), log
   a `warn` and drop back to `tokio::fs` without propagating
   the error.
3. Gate macOS CI on the non-feature path; add a Linux-only
   `cargo test --features io-uring` job to
   `.github/workflows/ci.yml` as a separate cell, not a
   matrix.

Expected scope: ~250-400 lines; no new crate. Risk: tokio-
uring requires a current-thread runtime. The archive writer
already runs on its own per-broadcast task so this should be
compatible with `lvqr-cli::start`'s multi-thread runtime
without any flavor change, but verify at first attempt.

## Session 86 close (2026-04-17)

### What shipped (3 commits total)

1. **Hygiene sweep** (`67763d1`). HANDOFF.md rotated from
   11,734 lines (564 KB) down to 345 lines; sessions 83 back
   to 1 archived verbatim to
   `tracking/archive/HANDOFF-tier0-3.md`. Five legacy AUDIT
   docs (`AUDIT-2026-04-10.md`,
   `AUDIT-2026-04-13.md`, `AUDIT-INTERNAL-*`,
   `AUDIT-READINESS-*`, `notes-2026-04-10.md`) moved via `git
   mv` to `tracking/archive/` with a new
   `tracking/archive/README.md` mapping each file to its
   role. `lvqr-wasm` added to the 5-artifact contract
   IN_SCOPE list so the educational warnings for its missing
   fuzz + integration + conformance slots surface as the
   forcing function for sessions 86/87. README gets a "what
   is NOT shipped yet" block so a casual reader cannot miss
   the ROADMAP Tier 3 items TIER_3_PLAN scoped out
   (webhooks, DVR scrub UI, hot reload, captions + SCTE-35,
   stream key CRUD) plus all pending Tier 4 items. No code
   changes; test count unchanged at 724.

2. **Tier 4 item 4.2 session B: WASM observer + CLI + E2E**
   (`efca5ce`). Full writeup in the feat commit message;
   synopsis here.

   New module `crates/lvqr-wasm/src/observer.rs` (~230
   LOC). `WasmFilterBridgeHandle` is clonable, holds
   per-`(broadcast, track)` atomic counters (fragments_seen
   / kept / dropped) in a `DashMap`, and holds the per-
   broadcaster tokio tasks alive for the server lifetime.
   `install_wasm_filter_bridge(registry, filter) -> handle`
   registers an `on_entry_created` callback on the shared
   `FragmentBroadcasterRegistry`; each fresh broadcaster
   spawns one tokio task that subscribes, runs every
   fragment through `filter.apply`, increments counters, and
   fires `lvqr_wasm_fragments_total{outcome=keep|drop}`
   metrics.

   The tap is **read-only** in v1 (session-B scope
   narrowing). Drop returns update counters but the original
   fragment still flows to HLS / DASH / WHEP / MoQ / archive
   unchanged. Full stream-modifying pipelines are deferred
   to v1.1; the two clean design options (ingest-side filter
   wiring per protocol, or broadcaster-side interceptor
   inside `lvqr-fragment`) are documented at the top of
   `observer.rs` for whichever session picks it up.

   CLI + config surfaces:

   * `ServeConfig.wasm_filter: Option<PathBuf>` (loopback
     default `None`).
   * `--wasm-filter <path>` / `LVQR_WASM_FILTER` clap arg in
     `lvqr-cli`.
   * `ServerHandle.wasm_filter() -> Option<&WasmFilterBridgeHandle>`.
   * `TestServerConfig::with_wasm_filter(path)` +
     `TestServer::wasm_filter()` passthrough.

   `start()` loads + compiles the module via
   `WasmFilter::load` and installs the bridge BEFORE any
   ingest listener accepts traffic, so the very first
   fragment of the very first broadcast flows through the
   filter.

   Example filter: `crates/lvqr-wasm/examples/frame-counter.
   wat` + an 82-byte pre-compiled `frame-counter.wasm`. The
   filter is a no-op that returns the input length
   unchanged; the interesting behaviour is host-side
   counting.

   Integration test
   `crates/lvqr-cli/tests/wasm_frame_counter.rs` (~260
   LOC) publishes a real two-keyframe RTMP broadcast through
   a TestServer pointed at the committed .wasm and asserts
   the tap observed fragments on `live/frame-counter`, with
   zero drops and kept == seen > 0. No mocks, no stdout
   capture; reads straight off the bridge handle.

3. **Session 86 close doc** (this commit).

### Tests shipped

| # | Test | Passes? |
|---|---|---|
| 4 | `observer::tests::*` in `lvqr-wasm/src/observer.rs` | ok |
| 1 | `wasm_frame_counter_sees_every_ingested_fragment` in `lvqr-cli/tests/wasm_frame_counter.rs` | ok |

Total workspace tests: **729** (+5 from session 85's 724).

### Ground truth (session 86 close)

* **Head**: `efca5ce` on `main` (feat) before this close-doc
  commit lands. Local main was even with origin/main after
  the hygiene-sweep push (`67763d1`); this session adds two
  more commits on top. Do NOT push without direct user
  instruction.
* **Tests**: 729 passed, 0 failed, 1 ignored.
* **CI gates locally clean**: fmt, clippy workspace --all-
  targets --benches -- -D warnings, test --workspace all
  green.
* **Workspace**: 26 crates, unchanged.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **A + B DONE**, C pending | 85 (A) / 86 (B) / 87 (C) |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 87 entry point

**Tier 4 item 4.2 session C: hot reload + a second example
filter that actually drops.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2
session C:

1. `WasmFilter::load` keeps its current shape; add a new
   `WasmFilterReloader` that watches the .wasm path via
   `notify::RecommendedWatcher`, compiles the new module on
   change, and calls `SharedFilter::replace(new_filter)`
   (the replace method shipped in session A).
2. In-flight `apply` calls finish on the OLD module; the
   next fragment uses the new one. Document atomicity at
   the call boundary.
3. Second example filter at
   `crates/lvqr-wasm/examples/redact-keyframes.{wat,wasm}`
   that returns -1 on every call (drops every fragment).
   Committed pre-compiled alongside the existing
   frame-counter.
4. Integration test
   `crates/lvqr-cli/tests/wasm_hot_reload.rs` at
   ~200 LOC. Publishes RTMP, asserts the frame-counter
   tap sees fragments with dropped=0. Then copies
   redact-keyframes.wasm over the configured filter path.
   Gives the watcher a beat to notice. Publishes more
   RTMP. Asserts subsequent fragments increment the
   dropped counter.

Expected scope: ~300-400 lines. Risk: notify's file-watch
semantics differ across macOS (FSEvents) vs Linux
(inotify). The existing lvqr-archive recorder has similar
exposure and landed green; worst case we use polling mode
which costs a 100 ms latency.

Also bring session C: update
`scripts/check_test_contract.sh` if needed -- the
lvqr-wasm integration slot is now met by
`tests/wasm_frame_counter.rs` (via `lvqr-cli`); the
fuzz + conformance slots remain open until a future
session.

## Session 85 close (2026-04-17)

### What shipped (1 feat commit, +1414 / -14 lines)

### Plan-faithful vs roadmap-complete

Tier 3 closed against `tracking/TIER_3_PLAN.md`'s scope
(cluster plane + observability plane). It did NOT close every
item in `tracking/ROADMAP.md`'s broader Tier 3 list. The
deferred items are tracked here explicitly so nobody reading
"Tier 3 COMPLETE" expects surfaces that were scoped out:

* **3.2 DVR scrub UI** -- `/playback/*` admin routes ship the
  JSON + byte-serving data surface. A dedicated web UI is
  Tier 5 ecosystem scope.
* **3.3 Webhook + OAuth + HMAC signed URLs** -- not shipped.
  HS256 static JWT is the only dynamic auth today.
* **3.5 Hot config reload** -- not shipped.
* **3.6 Captions + SCTE-35** -- Tier 4 item 4.5 (whisper.cpp
  captions) lands the transcription path, but SCTE-35 ad
  insertion and a full WebVTT segmenter are not scoped for
  v1.
* **3.7 Stream-key lifecycle CRUD** -- not shipped; static
  keys only.

These would add ~7 calendar weeks if a deployment needs them.
None is blocked by a design unknown.

## Session 85 close (2026-04-17)

### What shipped (1 feat commit, +1414 / -14 lines)

1. **Tier 4 item 4.2 session A: lvqr-wasm scaffold** (`727151f`).
   First Tier 4 code landing per
   `tracking/TIER_4_PLAN.md` section 4.2.

   New workspace crate `crates/lvqr-wasm/` (workspace member
   #26, NOT the browser-facing `lvqr-wasm` deleted in
   0.4-session-44; this is a fresh server-side host).

   Surface:

   * `FragmentFilter` trait. One synchronous method:
     `apply(Fragment) -> Option<Fragment>`. `Some` keeps
     (possibly with a replaced payload), `None` drops.
   * `WasmFilter` concrete impl. Compiles a WASM module via
     `WasmFilter::load(path)` or `WasmFilter::from_bytes(&[u8])`.
     Creates a fresh `wasmtime::Store` per `apply` call so
     filters cannot accumulate state across fragments (LBD
     #10 anti-scope from the plan).
   * `SharedFilter` wrapper (`Arc<Mutex<Box<dyn
     FragmentFilter>>>`) for thread-safe observer installs;
     includes `replace()` so session C's hot-reload path can
     swap modules atomically.

   Host-to-guest ABI (intentionally minimal -- core WASM, not
   the component model):

   * Guest exports `memory` (1-page initial) and
     `on_fragment(ptr: i32, len: i32) -> i32`.
   * Host writes payload to offset 0 of memory, calls
     `on_fragment(0, payload_len)`.
   * Return value: negative -> drop; non-negative N -> keep
     the fragment, use the first N bytes of memory as the
     replacement payload. N = 0 is a legal keep-with-empty-
     payload, semantically distinct from drop.
   * One substantive design cycle: original draft used `0`
     for drop, which collided with the legitimate empty-
     payload case (the `empty_payload_roundtrips_unchanged`
     unit test caught it on first run). Switched to
     negative-means-drop before commit.

   Fail-open semantics: a module that fails to instantiate
   or traps at runtime logs a `tracing::warn` and passes the
   fragment through unchanged. A single misbehaving filter
   cannot take down the server.

   Metadata pass-through: `track_id`, `group_id`,
   `object_id`, `priority`, `dts`, `pts`, `duration`, `flags`
   pass through unchanged regardless of filter output.
   Session B / C broaden the host-function surface to cover
   metadata mutation; session A ships the simplest useful
   shape so the runtime, trait, test harness, and CLI wiring
   path can land without scope entanglement.

   Workspace deps pinned (new):

   * `wasmtime = "25", default-features = false,
     features = ["runtime", "cranelift"]` -- per
     TIER_4_PLAN's dependency-pin table. Component model +
     WASI 0.2 stable as of 25.0 but we use core WASM for
     now; the dep still covers session B+ needs.
   * `notify = "6"` -- pulled in now so session 87's
     hot-reload path has the import available without a
     second Cargo edit. The watcher is stubbed in session A.

   Tests:

   * 9 unit tests in `crates/lvqr-wasm/src/lib.rs` cover
     no-op passthrough, drop, truncate, missing-memory
     fallback, empty-payload roundtrip, `SharedFilter`
     clone + `replace`, invalid-bytes rejection, and the
     `path()` accessor.
   * 1 proptest at `tests/proptest_roundtrip.rs` (256 cases)
     asserts arbitrary `Fragment` (any metadata, 0-16 KiB
     payload) roundtrips through a no-op WASM module
     byte-for-byte. 16 KiB cap is deliberate for session A
     (full bound lands with session B's `FragmentObserver`
     wiring once linear-memory growth is exercised under
     production payload sizes).
   * Test fixtures are WAT snippets assembled via the `wat`
     dev-dep at test time; no pre-compiled `.wasm` fixtures
     in the repo, no external toolchain dependency.

### Why core WASM and not the component model

Scope narrowing, not a design pivot. The
single-export `on_fragment(ptr, len) -> i32` surface binds
with `wasmtime::TypedFunc` directly and lets session A ship
the trait + harness without dragging in `cargo-component` or
a wit-bindgen build step for test fixtures. Session B is the
right place to decide whether the component-model binding is
worth its boilerplate for a broader host surface (e.g. if we
want full metadata mutation, or a richer error channel).
`FragmentFilter` is the stable surface the rest of the
workspace depends on; the transport between `WasmFilter` and
the guest module is an implementation detail that can change
without churning `FragmentBroadcasterRegistry` call sites.

### Ground truth (session 85 close, pre-session-close-doc commit)

* **Head**: `727151f` on `main`. v0.4.0. Local main is **1
  commit ahead of origin/main**; after this session-close
  doc lands it will be 2 ahead. 3 other commits from
  sessions 82-84 that were already queued had been pushed
  at session 82's close (see `6d99bef`); only sessions 83-84
  commits were held. Post-session-83 the 2 unpushed
  (session-83 feat + session-83 doc) + session-84 doc were
  all still local; this session adds session-85 feat. After
  the session-close doc commit lands: **5 commits queued**
  (9666cd1, 755d320, 7fb8dfe, 727151f, and this close doc).
  Do NOT push without direct user instruction.
* **Tests**: 724 passed, 0 failed, 1 ignored. Delta from
  session 84 (which was planning-only): +10 (9 lib unit +
  1 proptest harness with 256 cases). Delta from session 83:
  +10.
* **Code**: +1414 / -14 net. Workspace `Cargo.toml` + `Cargo.lock`
  (wasmtime 25.0.3 + notify 6.1.1 + their transitives),
  `crates/lvqr-wasm/Cargo.toml`, `crates/lvqr-wasm/src/lib.rs`
  (441 lines), `crates/lvqr-wasm/tests/proptest_roundtrip.rs`
  (90 lines).
* **Workspace**: **26 crates** (+1: `lvqr-wasm`).
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets --benches -- -D
  warnings`, `cargo test --workspace` all green.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | **A DONE**, B/C pending | 85 / 86 / 87 |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 86 entry point

**Tier 4 item 4.2 session B: WasmFragmentObserver + CLI
wiring + RTMP E2E.**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2 session B:

1. New `WasmFragmentObserver` in `lvqr-wasm` that
   implements `lvqr_fragment::broadcaster::FragmentObserver`
   (or the equivalent observer trait used by
   `FragmentBroadcasterRegistry`). On each fragment it calls
   the `SharedFilter::apply` path and forwards the result;
   drops are sinks, not errors.
2. `lvqr-cli` gains `--wasm-filter <path>` (env
   `LVQR_WASM_FILTER`). When set, `start()` loads the
   module via `WasmFilter::load`, wraps in `SharedFilter`,
   and installs the observer on the shared
   `FragmentBroadcasterRegistry` before any ingest listener
   starts accepting traffic.
3. First example filter at
   `crates/lvqr-wasm/examples/frame-counter/`. A hand-rolled
   WAT (or a minimal Rust WASM crate if simpler) that counts
   invocations and writes to WASI stderr every 100th call.
   Committed as source + pre-compiled `.wasm` under
   `examples/frame-counter.wasm`.
4. Integration test at
   `crates/lvqr-cli/tests/wasm_frame_counter.rs`. Publishes
   real RTMP through `TestServer` with `--wasm-filter=<path>`,
   asserts stderr (or a capture hook) contains the counter
   log, asserts the fragment pipeline still reaches
   downstream egress (i.e. HLS playlist shows up with the
   expected segments).

Expected scope: ~400-600 lines. Biggest risk is WASI stderr
capture in the test harness; if that proves flaky, the
example filter writes to a host-call side channel and the
test observes the count directly.

## Session 84 close (2026-04-17)

### What shipped (1 docs commit, +620 / -1 lines across HANDOFF + TIER_4_PLAN)

Planning session only; no code changes. Wrote
`tracking/TIER_4_PLAN.md` to bound Tier 4 scope before the
first implementation session, per ROADMAP load-bearing
decision #10 (\"every Tier 4 item gets a one-page MVP spec
before work starts\").

The plan covers all 8 Tier 4 items from ROADMAP, each with a
1-page section that includes:

1. Scope (what lands)
2. Anti-scope (explicit rejections)
3. API sketch (where relevant)
4. Session decomposition (2-3 sessions per item, numbered 85
   through 105)
5. Risks + mitigations

Execution order prioritises moat value per week of work,
dependency ordering, and "public demo" items first so the M4
marketing milestone lands on schedule:

1. 4.2 WASM per-fragment filters (3 weeks, sessions 85-87)
2. 4.1 io_uring archive writes (2 weeks, sessions 88-89)
3. 4.3 C2PA signed media (1 week, sessions 90-91)
4. 4.8 One-token-all-protocols (1 week, sessions 92-93)
5. 4.5 In-process AI agents / whisper.cpp (3 weeks, 94-97)
6. 4.4 Cross-cluster federation (2 weeks, 98-100)
7. 4.6 Server-side transcoding (2 weeks, 101-103)
8. 4.7 Latency SLO scheduling (1 week, 104-105)

Total: ~27 working sessions including 3-session buffer.
Budget: sessions 85 through 111. At 10-15 sessions / calendar
week, **~10-12 focused calendar weeks** for all of Tier 4.

Plan includes explicit non-goals: no browser WASM target, no
multi-filter pipelines, no SIP, no room-composite egress, no
live-signed C2PA streams, no GPU WASM, no admission control on
SLO breach, no OAuth2 / JWKS.

Three open questions deferred to the session that lands the
affected item:
* C2PA default `assertion_creator` string (proposal:
  `urn:lvqr:node/<node_id>`)
* Federation link auth layer (proposal: JWT bearer via item
  4.8's normaliser, which lands BEFORE federation)
* WASM filter audio handling (proposal: audio passthrough
  untouched in v1)

Five resolved questions (answered in the plan itself):
* WASM runtime = wasmtime (not wasmer, not wasmi)
* AI agent trait runs synchronously on the fragment hot path
  via `&mut self + &Fragment -> ()`; expensive work buffers
  internally
* Federation auth = JWT via item 4.8 (do not invent a new
  layer)
* Transcoding output = new broadcast, not new track on
  source broadcast
* SLO metric is server-side only in v1; true glass-to-glass
  lands in Tier 5 SDKs

### Ground truth (session 84 close, pre-session-close-doc commit)

* **Head**: `755d320` on `main`. v0.4.0. Local main and
  origin/main are **EVEN** at the session-83 close
  (`755d320`) as of this session's start; after this doc
  commit lands, local will be 1 commit ahead. Do NOT push
  without direct user instruction.
* **Tests**: 714 passed, 0 failed, 1 ignored.
  Unchanged from session 83 close (no code landed this
  session).
* **Code**: planning-only. `tracking/TIER_4_PLAN.md` (~620
  lines) and this close block on `tracking/HANDOFF.md`.
* **Workspace**: 25 crates, unchanged.
* **CI gates locally clean**: no rebuild needed; session 83
  close state stands.

### Tier 3 final state (unchanged from session 83 close)

All 13 sessions DONE. Cluster plane (71-79) + observability
plane (80-83) closed. LVQR is a multi-node live video server
with turnkey OTLP telemetry.

### Tier 4 execution status

| # | Item | Status | Sessions |
|---|---|---|---|
| 4.2 | WASM per-fragment filters | PLANNED, next up | 85-87 |
| 4.1 | io_uring archive writes | PLANNED | 88-89 |
| 4.3 | C2PA signed media | PLANNED | 90-91 |
| 4.8 | One-token-all-protocols | PLANNED | 92-93 |
| 4.5 | In-process AI agents | PLANNED | 94-97 |
| 4.4 | Cross-cluster federation | PLANNED | 98-100 |
| 4.6 | Server-side transcoding | PLANNED | 101-103 |
| 4.7 | Latency SLO scheduling | PLANNED | 104-105 |

### Session 85 entry point

**Tier 4 item 4.2 -- WASM per-fragment filters (session A of
3).**

Deliverable per `tracking/TIER_4_PLAN.md` section 4.2 session A:

1. New crate `crates/lvqr-wasm/`. NOT the deleted
   browser-facing `lvqr-wasm` referenced in the post-0.4.0
   removal block; this is a fresh server-side crate for
   `wasmtime`-hosted fragment filters.
2. Pin `wasmtime = "25"` as a workspace dep. Component
   model + WASI 0.2 are stable in 25.0. Pin `notify = "6"`
   for the session-87 hot-reload path (added now so
   session A has the import available but the code path is
   stubbed).
3. Define a `FragmentFilter` trait plus one concrete impl
   `WasmFilter` that loads a WASM component from disk and
   exposes one host call: `on-fragment(fragment) -> option<fragment>`.
   Matches the `lvqr:filter@0.1.0` WIT interface documented
   in the plan.
4. One proptest under `crates/lvqr-wasm/tests/proptest_roundtrip.rs`
   that pushes arbitrary `lvqr_fragment::Fragment` values
   through a no-op filter (a WASM component that returns
   input unchanged) and asserts bytewise equality on the
   payload.
5. Skeletons for fuzz + integration + E2E + conformance
   slots per the 5-artifact contract. These can be
   educational-warning level in session A; session B closes
   them.

Expected scope: ~300-500 lines split between
`crates/lvqr-wasm/src/lib.rs` (~200 LOC),
`crates/lvqr-wasm/Cargo.toml`, a minimal WASM component
fixture under `crates/lvqr-wasm/tests/fixtures/` (can be
compiled-in-advance WASM bytes committed to the repo), and
the proptest harness. No CLI wiring in session A; that comes
in session B.

Risk to flag on entry: wasmtime 25's component-model host
binding generator has a fair amount of boilerplate. If the
generated code exceeds ~300 LOC per host call, we use the
lower-level `Linker::func_wrap` API instead of the WIT
bindgen macro; session A picks whichever ships green first.

## Archived session blocks

Sessions 83 back to 1 live in
[`tracking/archive/HANDOFF-tier0-3.md`](archive/HANDOFF-tier0-3.md).

Rotation happened at session 86 during the post-Tier-3 hygiene
sweep. Live HANDOFF now holds only Tier 4 session blocks
(session 84 onward); historical context for Tier 0 through
Tier 3 stays on disk but outside the default read path so
fresh sessions do not pay the full ~560 KB context load on
every HANDOFF.md open.

The rotation is lossless. Every session close from 1 through
83 is preserved verbatim in the archive file; this live
HANDOFF is the authoritative source going forward.
