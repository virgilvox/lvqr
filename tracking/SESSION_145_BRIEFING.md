# Session 145 Briefing -- Workspace 0.4.1 release + republish all 26 publishable Rust crates

**Date kick-off**: 2026-04-24
**Predecessor**: Session 144 (per-peer mesh capacity advertisement).
With 144's close, the four mesh-data-plane README rows are all
shipped; sessions 141-144 source changes (per-peer offload reporting
in 141, ICE-config + TURN deploy recipe in 143, per-peer capacity in
144) live on `origin/main` at `9f5bded` but NONE of them have reached
crates.io yet -- the Rust workspace last published at `0.4.0` on
2026-04-16. Session 144 also claimed `lvqr-transcode` for the first
time at `0.4.0`, which means **all 26 publishable Rust crate names
are now claimed on crates.io** and a workspace-wide bump is
mechanically possible.

This is a release-only session. No new features. No source changes
beyond version strings, the audit pre-flight, and the post-publish
README flip.

## Goal

Cut workspace `0.4.1` and republish every publishable crate so the
`origin/main` source tree is reachable from `cargo install`. Land
the audit fix that just started failing CI as a separate pre-flight
commit so the publish events fire from a green tree.

After this session:

* `lvqr-cli 0.4.1` on crates.io contains sessions 141 + 143 + 144's
  source.
* `cargo install --locked lvqr-cli` from a fresh cache resolves and
  builds cleanly through Tier 0 -> Tier 5.
* README's Rust SDK row reads `0.4.1` instead of `0.4.0`.
* Origin `main` CI is green (Supply-chain audit included).

## Open question -- 25 vs 26 (resolve before any source touch)

The session-145 kickoff prompt says "republish all **25** already-
claimed Rust crates" but the tier list inside the same prompt
contains **26** crates (the difference: `lvqr-transcode` appears in
Tier 2). Reconciling:

* **Pre-session-144**: 25 crates were on crates.io at 0.4.0.
* **Session 144**: `lvqr-transcode 0.4.0` was first-published,
  bringing the total to 26.
* **Today (pre-145)**: all 26 publishable crates have a 0.4.0 entry
  on crates.io.

Because every member crate uses `version.workspace = true`, the
workspace.package.version is shared. Bumping it to `0.4.1` will
auto-bump `lvqr-transcode` along with the other 25; there is no
clean "keep one crate at 0.4.0" path short of un-inheriting that
crate's version (a mess we should not introduce for a one-day-old
publish).

**Recommendation: bump and republish all 26.** The 26th publish costs
one extra `cargo publish` run with no source diff vs `0.4.0`; the
benefit is workspace-version coherence and a single release narrative.
The user should confirm before any version string moves.

## Pre-flight blocker -- Supply-chain audit is RED on `origin/main`

`cargo audit --deny warnings` (in `.github/workflows/audit.yml` and
also a step inside `ci.yml`'s `Supply Chain (cargo-audit)` job) is
failing on the latest commit `9f5bded`. The other seven CI jobs
(Test Contract, SDK tests, Feature matrix, MPEG-DASH Conformance,
LL-HLS Conformance, Tier 4 demos, CI) are green. The audit job
flagged **19 vulnerabilities + 6 denied warnings**, of which the
load-bearing entries break down as:

| Crate | RUSTSEC | Date | Pinned | Fix path |
|---|---|---|---|---|
| `rustls-webpki` | 2026-0098 | 2026-04-14 | 0.103.11 | bump to >=0.103.13 |
| `rustls-webpki` | 2026-0099 | 2026-04-14 | 0.103.11 | bump to >=0.103.13 |
| `rustls-webpki` | 2026-0104 | 2026-04-22 | 0.103.11 | bump to >=0.103.13 |
| `wasmtime` | 2026-0021 | 2026-02-24 | 25.0.3 | requires >=36/42/43 |
| `wasmtime` | 2026-0089 | 2026-04-09 | 25.0.3 | requires >=36/42/43 |
| `wasmtime` | 2026-0092 | 2026-04-09 | 25.0.3 | requires >=36/42/43 |
| `wasmtime` | 2026-0095 | 2026-04-09 | 25.0.3 | requires >=36/42/43 |
| `wasmtime` | 2025-0118 | 2025-11-11 | 25.0.3 | requires >=36/42/43 |
| `rsa` | 2023-0071 | 2023-11-22 | 0.9.10 | NO FIX AVAILABLE |

The `rustls-webpki` advisories are all dated 2026-04-14 to 2026-04-22
-- they appeared AFTER session 141-143's CI passes. The audit-DB
refresh on the most recent commit picked them up. The Cargo.lock
pins `rustls-webpki = 0.103.11`, three patch revs behind the fix.

**Why this blocks the release flow**: `cargo publish` itself does
NOT run `cargo audit`, so we *could* publish from a tree where CI
on `main` is red. We should not -- the published source would carry
a known-failing supply-chain check, and a routine `cargo install
--locked lvqr-cli` smoke could legitimately surface the same
warnings to operators.

### Decisions locked for the audit fix

1. **`rustls-webpki` -> upgrade in Cargo.lock only.** Run
   `cargo update -p rustls-webpki` to pull the latest 0.103.x. No
   `Cargo.toml` change needed (the workspace pin is on `rustls
   0.23`; `rustls-webpki` is transitive). Verify post-update lockfile
   carries 0.103.13 or newer for every entry. Three rows of
   `rustls-webpki` appear in the lockfile graph (different
   transitive paths); all three should resolve to the same fixed
   patch.

2. **`wasmtime 25` advisories -> ignore via `audit.toml`, do NOT
   upgrade.** The workspace.dependencies comment in `Cargo.toml`
   lines 168-172 reads "Any upgrade gets its own session per
   `TIER_4_PLAN.md` dependency-pin table." Bumping `wasmtime` from
   25 to 36/42/43 is a multi-major jump that the project explicitly
   gated behind a dedicated session. Do not touch it here.

   Add a project-root `audit.toml` (cargo-audit reads this from CWD
   by default) with each `wasmtime` advisory listed under
   `[advisories].ignore` and a one-line comment naming the deferred-
   upgrade session. Recommend a follow-up agent in 2-3 weeks to file
   the wasmtime upgrade as a planned Tier 4 maintenance session.

3. **`rsa` Marvin (RUSTSEC-2023-0071) -> ignore permanently.** The
   advisory itself states `Solution: No fixed upgrade is available!`.
   `rsa` is a transitive of `c2pa` (Tier 4 item 4.3 signed-media
   stack). The Marvin attack requires a chosen-ciphertext oracle
   against an in-process RSA decryption path; LVQR uses `rsa` only
   for c2pa manifest signing (signing, not decryption), so the
   advisory is not exploitable in our use. Add to `audit.toml`
   ignore with the rationale comment.

4. **Audit-fix is a separate commit.** Subject:
   `chore(audit): bump rustls-webpki + ignore wasmtime/rsa
   advisories`. Lands BEFORE the version-bump commit so the
   publish events fire from a green tree.

The 6 "denied warnings" in the audit output are unmaintained-crate
warnings (cargo-audit promotes them to errors under `--deny
warnings`); they appear in the output as the 6 entries beyond the
19 vulnerabilities. Resolve in the same `audit.toml` block where
applicable; if any are actively-maintained-but-stuck-on-old-tag
flags we should treat them as ignores with a clear "tracked"
comment, same shape as the wasmtime entry.

## Decisions locked for the release

### 1. Single workspace.package.version flip, NOT per-crate

Audit of every member crate's Cargo.toml (29 crates including the 3
`publish = false` ones) shows ALL of them use
`version.workspace = true`. There is no crate that pins its own
version inline, so the entire release reduces to:

* One line in `Cargo.toml` workspace.package: `version = "0.4.0"` ->
  `version = "0.4.1"`.
* 26 lines in `Cargo.toml` workspace.dependencies: every internal
  `lvqr-X = { version = "0.4.0", path = "..." }` entry's version
  string flips to `"0.4.1"`. The 3 `path`-only entries (lvqr-
  conformance, lvqr-test-utils, lvqr-soak; all `publish = false`)
  do not need to change because they have no `version = ...`.
* `cargo check` to regenerate Cargo.lock with the 26 internal-crate
  bumps recorded.

Per-crate flipping is anti-scope; would require un-inheriting from
the workspace, which the project does not currently do anywhere and
which would increase manifest churn for zero release-narrative
benefit.

### 2. Internal-dep version constraints flip atomically with the workspace bump

Same commit. If the workspace.package.version moved without the
workspace.dependencies version strings tracking it, `cargo publish`
would fail tier 1+ builds: a tier-1 crate at 0.4.1 would resolve
its tier-0 dep against the registry-published 0.4.0 (because the
constraint says `version = "0.4.0"`) which is an older snapshot of
the source -- and may not even have the new APIs the tier-1 crate
uses (e.g. `MeshCoordinator::add_peer` grew a `capacity` argument
in 144).

Atomic bump = both happen in the same commit. Subject:
`chore(release): bump workspace 0.4.0 -> 0.4.1`.

### 3. CHANGELOG -- skip for this release

`CHANGELOG.md` last had a real entry at `## [0.4.0] - 2026-04-16`,
followed by a `## Unreleased (post-0.4.0, through session 82 --
2026-04-17)` section that has been stale for 62 sessions. Folding
sessions 83-144 into changelog form is an editorial job of several
hours and is not a release blocker:

* `tracking/HANDOFF.md` carries the per-session narrative and is
  the authoritative source for what changed between releases.
* `git log` between the `0.4.0` tag (if it exists) and the upcoming
  `0.4.1` head is exact and machine-readable.
* No public consumer has asked for a changelog refresh in the
  HANDOFF window we have visibility into.

**Recommendation: replace the stale "## Unreleased" header with a
2-line `## [0.4.1] - 2026-04-2X` entry that says "republish to
include sessions 141-144 source -- per-peer mesh capacity, ICE
config + TURN, offload reporting; see `tracking/HANDOFF.md` sessions
83-144 for the full narrative." A future session can fold sessions
83-144 into the changelog as a dedicated docs sweep.**

This is the cheapest move that does not leave the changelog in a
worse state than it is today. If the user wants the full fold-in
done now, that is a session 146 docs-sweep candidate.

### 4. Republish order: Tier 0 -> Tier 5, ~30-60s between tiers

The tier list from the kickoff prompt matches the workspace's intra-
crate dep graph (verified by `lvqr-transcode/Cargo.toml`'s
`lvqr-fragment = { workspace = true }` placing transcode in Tier 2,
not Tier 0). Plan:

| Tier | Crates | Count |
|---|---|---|
| 0 | lvqr-archive, lvqr-auth, lvqr-codec, lvqr-core, lvqr-moq, lvqr-observability | 6 |
| 1 | lvqr-cluster, lvqr-fragment, lvqr-record, lvqr-relay, lvqr-signal | 5 |
| 2 | lvqr-admin, lvqr-agent, lvqr-cmaf, lvqr-mesh, lvqr-transcode, lvqr-wasm | 6 |
| 3 | lvqr-agent-whisper, lvqr-hls, lvqr-ingest | 3 |
| 4 | lvqr-dash, lvqr-rtsp, lvqr-srt, lvqr-whep, lvqr-whip | 5 |
| 5 | lvqr-cli | 1 |
| | **Total** | **26** |

Rules of the road:

* **Within a tier, publishes can fire in parallel** (no intra-tier
  dependency by definition). In practice serialize them inside a
  single shell loop -- crates.io rate-limits per-token publishes,
  and a parallel storm risks a 429.
* **Between tiers, sleep ~45 s** for crates.io's index to become
  consistent. The previous tier's crates need to be discoverable by
  the next tier's `cargo publish` resolver. Session 144's
  lvqr-transcode publish observed ~30 s of index lag; 45 s gives
  margin.
* **`cargo publish --no-verify` is OFF.** Default verify path runs a
  full build + test of the packaged tarball, which catches
  packaging errors (missing files, path-only deps, license-file-
  not-included) before the upload. Worth the extra time.
* **Per-tier failure handling**: any failure inside a tier STOPS
  the script and surfaces output to the user. Do not retry a failed
  publish without explicit approval; a partial-state recovery may
  need a Cargo.lock or manifest tweak that should not auto-apply.

### 5. Smoke verify after Tier 5

`cargo install --locked lvqr-cli --force` in a clean `target` (or
better, a clean `CARGO_HOME=/tmp/lvqr-smoke cargo install ...`).
Asserts:

* The full tree resolves from crates.io alone (no path overrides).
* The 0.4.1 publishes are all index-consistent against each other.
* `lvqr --version` reports 0.4.1.

If smoke fails, the failing-tier output names the culprit and we
loop back. Do not move to step 6 until smoke is green.

### 6. Post-publish README flip

`README.md` carries a Rust SDK row that today reads "lvqr-core ...
0.4.0 on crates.io" plus the matching CLI row. Flip both to 0.4.1.
Subject: `docs(release): flip Rust SDK rows to 0.4.1 on crates.io`.

This is a separate commit from the version-bump commit so the
release-cycle's history reads as: (1) audit fix, (2) version bump,
(3) post-publish docs flip. Each step's commit exists for ~minutes
on local before its successor lands.

### 7. Live smoke after the README flip is sufficient

The session 144 close already did per-feature live smoke via
`curl /api/v1/mesh`. Re-running it after the version bump would only
prove that the bump is non-functional (which it is); the
`cargo install --locked lvqr-cli` smoke is the load-bearing
end-to-end check.

## Anti-scope (explicit rejections for this session)

* **No new feature work.** No bug fixes either, except the audit
  pre-flight which is purely a Cargo.lock + audit.toml change. If a
  bug surfaces during the dry-run that is unrelated to the version
  bump, log it and ship the release; address in the next session.
* **No CHANGELOG fold-in.** Session-by-session 83-144 narrative
  stays in HANDOFF; CHANGELOG gets a 2-line 0.4.1 stub.
* **No npm or PyPI re-publishes.** Those landed at 0.3.2 in session
  144 and are unaffected by the Rust 0.4.0 -> 0.4.1 bump. The npm
  packages (`@lvqr/core`, `@lvqr/player`) and the Python `lvqr`
  package version their wire shapes independently from the Rust
  workspace.
* **No wasmtime upgrade.** Per the workspace.dependencies inline
  comment, that gets its own session.
* **No tag push.** The user has not requested a `v0.4.1` git tag in
  this kickoff; if they want one, that is a one-line aside after
  smoke is green.
* **No `cargo publish` execution without explicit approval per
  tier.** Dry-run output for Tier 0 must be reviewed before the
  live Tier 0 fires; live Tier 0 must be observed clean before
  Tier 1 starts.

## Execution order

Each step is gated. STOP at any step that surfaces an unexpected
diff and surface to the user.

1. **Author this briefing.** Now.

2. **Walk the user through the version-bump strategy + commit shape**
   above. Resolve the 25-vs-26 question. Get approval.

3. **Audit pre-flight commit.**
   * `cargo update -p rustls-webpki`. Verify Cargo.lock now has
     0.103.13+.
   * Create `audit.toml` at repo root listing wasmtime + rsa
     advisories under `[advisories].ignore` with rationale comments.
     Address any of the 6 unmaintained-crate warnings here too.
   * Run `cargo audit --deny warnings` locally. Should be clean.
   * Run `cargo check --workspace` to confirm Cargo.lock change
     compiles.
   * Commit: `chore(audit): bump rustls-webpki + ignore deferred
     wasmtime/rsa advisories`.
   * Surface diff to user. STOP for approval before push.

4. **Version-bump commit.**
   * Edit `Cargo.toml`: workspace.package.version
     `"0.4.0"` -> `"0.4.1"`. 26 internal-dep version strings
     `"0.4.0"` -> `"0.4.1"` in [workspace.dependencies].
   * Optional CHANGELOG stub (2-line 0.4.1 entry replacing the
     stale Unreleased header).
   * `cargo check --workspace` to refresh Cargo.lock.
   * `cargo fmt --all -- --check` and `cargo clippy --workspace
     --all-targets -- -D warnings` to confirm nothing else moved.
   * Commit: `chore(release): bump workspace 0.4.0 -> 0.4.1`.
   * Surface diff to user. STOP for approval before push.

5. **Push the audit-fix + version-bump commits.** User approval
   required. After push, watch the audit job + the rest of CI on
   the new HEAD; STOP if anything regresses.

6. **Tier 0 dry-run.**
   * For each Tier 0 crate: `cargo publish -p <crate> --dry-run
     --allow-dirty`. (`--allow-dirty` because Cargo.lock may
     register-touch during the dry; double-check before live.)
   * Surface aggregate output to user. STOP for approval.

7. **Tier 0 live publish.** User approval. `cargo publish -p
   <crate>` for each Tier 0 crate sequentially. Sleep ~45 s. Verify
   each via `cargo search lvqr-<crate>` showing 0.4.1.

8. **Tier 1 -> Tier 5.** Same shape as step 7 -- live publish
   each tier sequentially, ~45 s sleep between tiers, intra-tier
   serialized. STOP at any failed publish.

9. **Smoke verify.** `cargo install --locked lvqr-cli --force` in a
   clean `CARGO_HOME`. Confirm `lvqr --version` reports 0.4.1.

10. **README post-flip commit + push.** User approval before push.

11. **HANDOFF session 145 close block.** Documents what shipped.

## Risks + mitigations

* **A 0.4.0 crate's source has uncommitted local-only changes.**
  `git status --short` at brief-write shows only `?? .claude/`
  (Claude's session state, untracked, not in the workspace). No
  uncommitted Cargo manifest or `.rs` change. Fully clean.

* **A crate's dry-run packaging fails on missing license-file or
  README.** Session 144 verified this for lvqr-transcode (the most
  recently first-published). Older crates have shipped before at
  0.4.0 so their package metadata is known-good. Re-failure here
  would indicate a Cargo.toml regression that landed between 0.4.0
  and now -- low probability, would surface in dry-run.

* **crates.io rate limits.** The free-tier limit is "publishes per
  hour per token". 26 publishes spread across ~6 tiers with 45s
  inter-tier sleep is well under any documented limit. If a 429
  surfaces, sleep + retry per the retry-after header rather than
  ramming.

* **Publishing tier-1 before tier-0 indexes -> tier-1 fails.**
  Mitigated by the 45 s inter-tier sleep. If a tier-1 publish
  surfaces a `failed to find crate ... in registry` resolver error,
  sleep another minute and retry. Do not run with `--allow-dirty
  --offline` paths that bypass the registry resolver.

* **rustls-webpki update bumps a transitive higher than expected
  and breaks something.** Verify locally with `cargo build
  --workspace` before commit. If a build break surfaces, drop back
  to a precise pin via `cargo update -p rustls-webpki --precise
  0.103.13` and re-verify.

* **cargo-audit installation in CI takes 60 s+.** The current
  workflow installs from source; previously cached. Not a session
  blocker -- just affects CI walltime after the bump commit lands.

* **A consumer in `bindings/` or `examples/` has a hardcoded
  `0.4.0` reference that breaks when the workspace flips.** Spot-
  check: the bindings (`bindings/js`, `bindings/python`) and
  `examples/tier4-demos/` have their own version state independent
  of the Rust workspace; they should not reference Rust crate
  versions inline. Verify with a grep for `0.4.0` across the tree
  during step 4 -- if any matches, surface to the user.

## Ground truth (session 145 brief-write)

* **Head**: `9f5bded` on `main`. v0.4.0. Local main and `origin/main`
  are EVEN at this commit. Working tree clean except `?? .claude/`
  (untracked harness state).
* **Crates**: 29 workspace members; 26 publishable; 3
  `publish = false` (lvqr-conformance, lvqr-test-utils, lvqr-soak).
* **Version style**: 100% `version.workspace = true`; no per-crate
  version pins.
* **CI on `main`**: 7 jobs green, 1 job red (Supply-chain audit:
  19 vulnerabilities + 6 denied warnings, all fixable per section
  "Pre-flight blocker" above).
* **crates.io**: all 26 publishable crate names claimed at 0.4.0.
* **npm + PyPI**: `@lvqr/core@0.3.2`, `@lvqr/player@0.3.2`,
  `lvqr@0.3.2` (PyPI). Untouched by this session.
* **Tests**: 1043 Rust default-gate / 0 / 3 + 30 Python pytest + 11
  Vitest. Will not move during this session except possibly
  Cargo.lock-driven rebuilds (counts unchanged).

## Step 0 deliverable -- this briefing

Author at `tracking/SESSION_145_BRIEFING.md`. Get user's sign-off on
section "Open question -- 25 vs 26", section "Pre-flight blocker",
and section "Execution order" before any source touch.
