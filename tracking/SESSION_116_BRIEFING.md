# Session 116 briefing -- phase B rows 115 or 116

Authored at the close of session 115 (head `675774d`) to tee up the next
session's pick between `PLAN_V1.1.md` rows 115 and 116. Both are
non-trivial design sessions that benefit from up-front briefing per the
`PLAN_V1.1.md` convention.

## Context

Phase B rows 113 (WHEP AAC-to-Opus transcoder) and 114 (WHIP->HLS +
SRT->DASH + RTMP->WHEP audio E2E tests) are SHIPPED on local `main` as
of session 115's close. 11 commits sit unpushed on `main` at head
`675774d`; `origin/main` remains at `2e50635`. Workspace tests **941 / 0
/ 1** on the default gate, 29 crates, Rust v0.4.0 on crates.io, npm
`@lvqr/core` + `@lvqr/player` at v0.3.1, PyPI `lvqr` at v0.3.1.

The two unshipped phase-B rows are:

| Row | Scope (from `PLAN_V1.1.md`) |
|---|---|
| 115 | Mesh data-plane step 2. Exercise the existing `@lvqr/core` `MeshPeer` client against the session 111-B server wiring. Add Playwright E2E with two browser peers. Flip `docs/mesh.md` from "topology planner only" to "topology planner + signaling wired; DataChannel media relay ready for end-to-end testing". |
| 116 | `examples/tier4-demos/` first public demo script. One polished scripted demo chaining WASM filter + Whisper captions + ABR transcode + archive + C2PA verify. Closes the Tier 4 exit criterion skipped when Tier 4 was marked COMPLETE. |

## Recommendation: start with row 115 (mesh Playwright)

Both rows are genuine phase-B work. Rationale for preferring row 115:

1. **Row 115 unblocks phase D.** Phase D (mesh data-plane completion,
   sessions 122-125) depends on having the two-browser E2E harness in
   place. Without row 115, phase D is harder to start. Row 116 is
   self-contained and does not block anything.

2. **Row 115 surfaces real bugs in a never-exercised code path.**
   `bindings/js/packages/core/src/mesh.ts` (267 LOC, shipped to npm at
   v0.3.1 since session 103) has zero tests against a live LVQR server.
   Session 111-B wired the Rust server-side half. A Playwright harness
   is the first moment the client-side DataChannel relay runs against
   real signaling. Bugs are likely and worth finding early.

3. **Row 116 is mostly glue code.** Every component (WASM filter,
   Whisper, transcode ladder, archive, C2PA verify) ships with an
   integration test in `crates/lvqr-cli/tests/`. The demo script
   chains them with shell / Node.js orchestration. Lower information
   density per hour spent; the orchestration itself does not stress-
   test any individual component.

4. **Playwright setup cost is one-time.** Once installed and wired into
   `bindings/js`, subsequent mesh / browser-side tests reuse the same
   harness. Row 116's effort does not create a reusable test framework.

Operators who prefer row 116 (e.g., marketing-grade demo takes priority
over engineering-grade coverage) can pick it with no penalty; both are
valid Plan B rows.

## Row 115 scoping decisions to lock in-briefing

### Two-peer topology

`MeshCoordinator::default()` has `root_peer_count = 30`, which means
both connecting peers are root peers (direct fanout from origin) until
peer 31 joins. For a two-peer test we need the SECOND peer to become a
CHILD of the first, not a second root. Fix: call
`TestServerConfig::with_mesh_root_peer_count(1)` (added in session
111-B1) so only one root peer is accepted; the second peer gets
`AssignParent(peer_1)` and opens a DataChannel to it.

### WebRTC transport

`@lvqr/core/src/mesh.ts::MeshPeer` uses `RTCPeerConnection` +
`RTCDataChannel`. Playwright's Chromium bundle supports WebRTC
natively. No TURN server needed on loopback; host candidates over
`127.0.0.1` suffice. If a two-browser test ever hits an ICE failure,
flip to `--disable-webrtc-hw-encoding` on the Chromium launch args.

### Media source for the test

Two options:

- **a) Use the origin's raw MoQ fanout.** The parent peer subscribes
  directly from the server's MoQ relay (via WebTransport). The child
  peer subscribes from the parent via DataChannel. Assert the child
  receives the same byte count the parent receives. Pro: exercises
  the real relay path. Con: requires a real broadcaster (e.g., a
  `fetch`-based synthetic injector on a helper test route).

- **b) Inject synthetic fragments server-side.** Use the existing
  `TestServer::fragment_registry()` accessor to push known bytes
  onto a track. Both peers subscribe; parent relays to child. Assert
  byte-for-byte match. Pro: minimal server-side moving parts. Con:
  does not exercise MoQ wire or WebTransport handshake on the parent
  side.

**Preferred: (a).** The point of this test is "end-to-end through the
relay, client-side parent/child forwarding happens transparently". (b)
skips half of that. (a) is harder but closer to the actual user story.

### Playwright setup

Add `@playwright/test` to `bindings/js/package.json` as a devDependency.
Configure `playwright.config.ts` with:

- `testDir: './tests/e2e/mesh'` or similar.
- `use: { headless: true, baseURL: process.env.LVQR_TEST_URL }`.
- One project entry for Chromium only (MeshPeer does not gate on
  Firefox quirks; keep the matrix small for the first test).
- `webServer` block to boot a `TestServer` process that exposes the
  relay + admin + signaling ports on fixed env-var-driven ports.

Keep the Playwright test count at one. Two browser contexts (peer_1,
peer_2) inside that single `test()`. Both contexts load a small HTML
shell from the admin server at `/static/mesh-peer-harness.html` (new
file to author; served via a new test-only axum route OR a file:// URL
Playwright loads directly).

### Server-side precedent

`crates/lvqr-cli/tests/mesh_ws_registration_e2e.rs` (session 111-B2) is
the closest Rust-side precedent. It exercises the `ws_relay_session`
registration path + `peer_assignment` leading-frame. Row 115 is the
client-side counterpart: two `MeshPeer` browsers register, the second
gets `AssignParent`, opens DataChannel, forwards media, child delivers
to consumer. Read that test first to understand the wire-shape side of
the contract.

### Scope limit

DO NOT attempt the full mesh data-plane checklist (actual-vs-intended
offload, per-peer capacity advertisement, TURN recipe, 3+ browser
Playwright). Those land in phase D (sessions 122-125) per
`PLAN_V1.1.md`. Row 115 is the "two browsers, happy path, proof of
client-side relay" slice.

## Row 116 scoping decisions (if chosen instead)

### Demo script shape

A single Bash script at `examples/tier4-demos/demo-01.sh` is the
simplest deliverable. It:

1. Boots `lvqr serve --features full` against a scratch dir.
2. Publishes an ffmpeg synthetic video via RTMP.
3. Attaches a pre-built WASM filter via a flag.
4. Enables whisper captions via `--whisper-model /path/to/model.bin`.
5. Enables the transcode ladder via `--transcode-rendition 720p,480p`.
6. Waits for the archive to finalize.
7. Verifies the C2PA signature via `/playback/verify/{broadcast}`.
8. Prints a summary + HLS master URL for browser preview.

All flags + code paths already ship; the demo is orchestration.

### Expected friction

- Whisper model download adds ~100 MB to the clone. Document a
  one-liner `curl` for the model; do not commit the binary.
- C2PA cert material needs a pre-generated test cert. Reuse
  `crates/lvqr-archive/tests/c2pa_verify_e2e.rs`'s fixture pattern.
- Transcode feature needs GStreamer at runtime. Document install
  recipe for macOS (brew) + Debian/Ubuntu (apt) in the demo README.

### README

`examples/tier4-demos/README.md` should name the prereqs (GStreamer,
whisper model, test cert), the expected runtime (~90 s on a
workstation), and the success criteria (HLS master loads + C2PA verify
returns `"signed_ok": true`).

### Scope limit

Row 116 is ONE demo. Per `PLAN_V1.1.md`, further polished demos are
deferred to Phase D.

## Read first, in this order

Regardless of the pick:

1. `CLAUDE.md`. Project rules -- no Claude attribution, no emojis, no
   em-dashes, 120-col max, real ingest/egress in tests, edit in-repo
   only, no push without instruction, never skip hooks, never
   force-push main.
2. `tracking/HANDOFF.md` status header + "Session 116 entry point"
   block + the session 115 close block above it.
3. `tracking/PLAN_V1.1.md` -- the matching row (115 or 116) scope
   line + "How to kick off the next session" block.

**For row 115 (mesh Playwright)**:

4. `bindings/js/packages/core/src/mesh.ts` -- the `MeshPeer` client
   under test. Understand `AssignParent` handling, RTCPeerConnection
   lifecycle, DataChannel `onmessage` fanout.
5. `crates/lvqr-cli/tests/mesh_ws_registration_e2e.rs` -- Rust-side
   precedent for the server wiring. Understand the `ws-{counter}`
   peer_id allocation + leading `peer_assignment` JSON frame.
6. `docs/mesh.md` -- the doc to flip. Current text says "topology
   planner only"; session 115 close leaves it untouched.
7. `crates/lvqr-test-utils/src/test_server.rs` -- `with_mesh`,
   `with_mesh_root_peer_count`, `signal_url`, `mesh_coordinator`
   accessors that Playwright's `webServer` block exercises.

**For row 116 (tier4-demos)**:

4. `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` -- C2PA signing +
   verify wire shape + test cert fixture pattern.
5. `crates/lvqr-cli/tests/whisper_cli_e2e.rs` -- whisper model path
   wiring + captions HLS subtitle track assertion.
6. `crates/lvqr-cli/tests/transcode_ladder_e2e.rs` -- `--transcode-
   rendition` flag + master playlist composition.
7. `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` -- archive finalize +
   `/playback/*` route shape.
8. `README.md` -- Tier 4 exit criterion referenced at line ~260.

## Verification gates

- `cargo fmt --all --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo test --workspace` stays >= 941 / 0 / 1 on the default gate.
- Row 115 additionally: `pnpm --dir bindings/js playwright test`
  (or equivalent) passes with the new mesh-peer Playwright spec.
- Row 116 additionally: `examples/tier4-demos/demo-01.sh` runs to
  completion on a GStreamer + whisper-model-equipped workstation.

## After session 116

Write a "Session 116 close" block at the top of `tracking/HANDOFF.md`
immediately after the status header. Mark the chosen
`tracking/PLAN_V1.1.md` row (115 or 116) SHIPPED. Update the
`project_lvqr_status` auto-memory. Commit as a feat commit + a
close-doc commit (two commits). Push only if the user says so.

If the user does ask to push, re-verify `git log --oneline
origin/main..main` first so the unpushed 113 + 114 + 115 chain rides
along as a single batch.

## Absolute rules (copied from `CLAUDE.md` + the session 115 kickoff)

- Never add Claude as author, co-author, or contributor in git
  commits, files, or any other attribution (no `Co-Authored-By`
  trailers).
- No emojis in code, commit messages, or documentation.
- No em-dashes in prose.
- 120-column max in Rust.
- Real ingest and egress in integration tests (no
  `tower::ServiceExt::oneshot` shortcuts, no mocked sockets).
- Only edit files within this repository.
- Do NOT push to origin without a direct user instruction.
- Plan-vs-code refresh on any design deviation from `PLAN_V1.1.md`.
- Never skip git hooks (no `--no-verify`, no `--no-gpg-sign`).
- Never force-push main.
