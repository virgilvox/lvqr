# LVQR Internal Code Audit -- 2026-04-13

This is the companion to `AUDIT-2026-04-13.md` (competitive audit). That
document compares LVQR to the field. This one audits LVQR against itself:
what code is rotten, what is dead, what is incomplete, and what is
load-bearing but under-tested.

Method: two parallel deep reads by Explore sub-agents covering every crate,
followed by manual verification of every critical claim against the actual
source. Findings below are only included after verification; agents
hallucinate and I do not trust their citations without checking.

## Summary

Three real bugs, two security hardening targets, two crates of dead code,
one unwired feature-complete subsystem, and several documentation gaps.
Nothing here is a five-alarm fire. Most of it is the normal debt of an
early-stage server that has not yet had a pre-production review.

Fix priority ranking, highest first:

1. **FIX**: `lvqr-relay::parse_url_token` does no validation on the
   broadcast path extracted from the MoQ session URL. Hardening target.
2. **FIX**: `lvqr-mesh::reassign_peer` leaves stale child references in
   the old parent. Latent bug; triggers only on live rebalance.
3. **WIRE**: `lvqr-auth::JwtAuthProvider` is feature-complete with unit
   tests but has zero consumers outside its own crate. CLI has no hook.
4. **DECIDE**: `lvqr-core::{Registry, RingBuffer, GopCache}` are tested,
   benchmarked, and entirely unused by any production code path.
5. **DELETE**: `lvqr-wasm` is explicitly marked deprecated in its own
   lib.rs header; `on_frame_callback` is set by JS callers but never
   invoked. Archive or remove.
6. **HARDEN**: admin `/metrics` endpoint bypasses auth middleware (intentional
   for Prometheus scraping, but should be documented as a security boundary).
7. **HARDEN**: `CorsLayer::permissive()` is applied to every axum router
   in lvqr-cli. Defense in depth says restrict to specific origins.
8. **DOCUMENT**: no auth-failure metrics, no rate limiting on any auth path.
   Brute-force attacks against admin token or subscribe token are invisible.

## Confirmed Bugs

### 1. Path traversal in broadcast name (lvqr-relay)

**File**: `crates/lvqr-relay/src/server.rs:208-218`

```rust
fn parse_url_token(url: Option<&url::Url>) -> (Option<String>, String) {
    let Some(url) = url else {
        return (None, String::new());
    };
    let broadcast = url.path().trim_start_matches('/').to_string();
    let token = url
        .query_pairs()
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned());
    (token, broadcast)
}
```

The broadcast string is extracted from the raw URL path with a single
`trim_start_matches('/')` and no further validation. It is then passed
straight to `self.auth.check(&AuthContext::Subscribe { ... broadcast })`.
An attacker can send a broadcast path containing `../`, backslashes, or
control characters.

**Real impact**: not a filesystem escape today (moq-lite uses the broadcast
string as an opaque key), but:

1. **Log injection**: the broadcast is logged at multiple tracing sites;
   newlines or control characters in the broadcast name corrupt structured
   logs.
2. **Auth provider confusion**: if a future auth provider (webhook, OAuth
   scope match, signed URL) treats the broadcast as a structured path,
   `../admin` could escape from a permission prefix.
3. **Recorder collision**: when a broadcast eventually flows into the
   recorder via the EventBus, `BroadcastRecorder::sanitize_name` catches
   the worst cases but a malformed broadcast can still hit the event bus
   subscribers in this bad state.

**Severity**: MEDIUM. Fix with a strict validator that rejects any
non-empty broadcast name that does not match `^[A-Za-z0-9/_\-.]+$` or
that exceeds 255 bytes. Empty names must still be accepted because MoQ
clients legitimately connect to the relay root URL and select broadcasts
via SUBSCRIBE protocol messages; the integration tests for lvqr-relay
rely on this and will fail fast if the validator rejects empty strings.

### 2. reassign_peer leaves stale child references (lvqr-mesh)

**File**: `crates/lvqr-mesh/src/coordinator.rs:166-206`

```rust
pub fn reassign_peer(&self, id: &str) -> Result<PeerAssignment, MeshError> {
    if !self.peers.contains_key(id) { ... }
    let parent = self.find_best_parent()?;
    let parent_depth = self.peers.get(&parent).map(|p| p.depth).unwrap_or(0);
    let assignment = {
        let Some(mut entry) = self.peers.get_mut(id) else { ... };
        entry.parent = Some(parent.clone());     // overwrites old parent
        entry.depth = parent_depth + 1;
        PeerAssignment { ... }
    };
    if let Some(mut parent_entry) = self.peers.get_mut(&parent) {
        parent_entry.children.push(id.to_string());  // adds to new parent
    }
    // MISSING: remove `id` from the old parent's children list
    Ok(assignment)
}
```

When `reassign_peer` runs, it overwrites `entry.parent` with the new
parent and appends the peer to the new parent's children. The old
parent's children list is never touched, leaving a stale reference that
points to a peer whose `parent` field no longer matches.

**Why this does not crash today**: `reassign_peer` is only called from
lvqr-cli after `remove_peer` returns a list of orphans. `remove_peer`
deletes the old parent entirely, so the dangling reference evaporates.
The bug is latent and will trigger the moment anyone adds a live
rebalance code path (e.g., moving a peer from an overloaded parent to an
underloaded one without the intermediate removal).

**Severity**: HIGH for correctness once the mesh sees real use; LOW
today. Fix defensively by reading the old parent out of `entry.parent`
before overwriting, then retaining the child list of the old parent.

### 3. Heartbeat test asserts nothing meaningful (lvqr-mesh)

**File**: `crates/lvqr-mesh/src/coordinator.rs:483-503`

```rust
let coord = MeshCoordinator::new(MeshConfig {
    heartbeat_timeout_secs: 0, // instant timeout for testing
    ...
});
coord.add_peer("peer-1".into(), ...);
std::thread::sleep(Duration::from_millis(10));
let dead = coord.find_dead_peers();
assert!(dead.contains(&"peer-1".to_string()));
coord.heartbeat("peer-1");
// But with 0s timeout it's still dead immediately... so this test
// really just validates the mechanism works
```

The test sets the timeout to 0 seconds so every peer is always considered
dead. Calling `heartbeat` after `find_dead_peers` does nothing observable.
The trailing comment admits the test does not assert what its name
implies. This is the exact theatrical-test anti-pattern that the Tier 0
audit called out.

**Severity**: LOW (test theater only). Fix by using a 1-second timeout
and asserting that a peer that just called `heartbeat` does not appear in
`find_dead_peers` until the timeout actually elapses.

## Dead Code Inventory

### lvqr-core::Registry, RingBuffer, GopCache

Grep result: these types appear only in `crates/lvqr-core/src/` (their
own implementations and tests), `crates/lvqr-core/benches/` (fanout and
ringbuffer benchmarks), and `crates/lvqr-test-utils/src/lib.rs` (a
`TestPublisher` helper that is itself unused by any shipped test).

Zero production call sites. The relay uses `moq_lite::OriginProducer`
directly for every fanout.

The lvqr-core lib.rs docstring already admits this: "the relay uses
moq-lite's `OriginProducer` for all track routing and fan-out. The
structures here exist for: shared type definitions ... and future use".

**Options**:

- Keep as a scaffolded fallback for a future WS fMP4 path, accepting the
  test-coverage debt.
- Move to a `fallback-fanout` feature and delete from the default build.
- Delete outright. The Unified Fragment Model in Tier 2.1 will replace
  them anyway.

**Recommendation**: delete outright in the same PR that introduces
`lvqr-fragment`. Removing them now costs us a feature we are not using;
keeping them extends lifetime support for code that is about to be
rewritten.

### lvqr-wasm (entire crate)

Already marked `# DEPRECATED` in its own lib.rs header (verified at
lines 1-19). The header says: "The TypeScript client implements the
full MoQ-Lite protocol (this WASM crate only exposed raw WebTransport
stream helpers and never wired up the data callbacks)."

`LvqrClient::on_frame()` stores a callback that is never invoked.
`LvqrClient::read_stream()` reads bytes but does not pass them to the
stored callback. The crate compiles, ships to crates.io, and does
nothing.

**Recommendation**: delete in v0.5. For v0.4, leave the deprecation
notice in place and remove `lvqr-wasm` from the default `Cargo.toml`
`full` feature if it is listed there. (It is not; verified.)

## Feature-Complete But Unwired

### lvqr-auth::JwtAuthProvider

Grep result: `JwtAuthProvider` appears only in its own implementation
file and the crate-level `lib.rs` re-export. Zero consumers.

The implementation is correct (verified: `jsonwebtoken::decode` with
`Validation::new(Algorithm::HS256)`, token extraction from the
`AuthContext`, scope mapping from claims). Unit tests cover happy path
and expired tokens.

**Gap**: `lvqr-cli::serve` only ever builds a `NoopAuthProvider` or a
`StaticAuthProvider`. There is no CLI flag or env var hook that
constructs a `JwtAuthProvider`. Operators who want JWT auth today
cannot enable it without modifying `main.rs`.

**Fix**: add a `--jwt-secret` flag (plus `LVQR_JWT_SECRET` env) that,
when set, instantiates `JwtAuthProvider` as the authoritative provider.
This is a 30-line change, lands this session.

## Security Hardening (Not Bugs, But Flagged)

### Admin metrics endpoint is unauthenticated

**File**: `crates/lvqr-admin/src/routes.rs:109-113`

The `/metrics` route is attached to the outer router and merged with
the `api_routes` subrouter that carries the auth middleware. Prometheus
scraping needs this, so it is intentional. The risk: metric label
cardinality can leak stream names, subscriber counts, or path-based
identifiers that an operator may consider sensitive.

**Recommendation**: document this as an explicit security boundary and
recommend firewalling `/metrics` at the reverse-proxy layer. Consider
adding an optional `--metrics-token` flag in Tier 3 when the
observability work lands, defaulting to unauthenticated.

### CORS permissive applied to every axum router

**File**: `crates/lvqr-cli/src/main.rs` (the `CorsLayer::permissive()`
call in `serve`)

`CorsLayer::permissive()` reflects any origin. With bearer tokens in
headers (not cookies) this is not a direct auth bypass because the
browser will not send the token to a cross-origin request unless the
attacker site explicitly attaches it. But it violates defense-in-depth
and means a malicious page can trigger OPTIONS preflights against the
admin API, use the admin API from dev tools, and retrieve stats that
should be internal.

**Recommendation**: replace with a restrictive default that allows only
the configured admin origin plus localhost for development. Tier 3
hardening.

### No auth failure metrics

Grep for `lvqr_auth_failures_total`: fires in `lvqr-cli` for WS ingest
and subscribe, and in `lvqr-relay` for MoQ session rejects. Good.

But the admin middleware at `lvqr-admin/src/routes.rs:149-163` logs the
denial at `debug` level and does not emit a metric. Brute-force attempts
against the admin token are invisible to Prometheus scrapers.

**Recommendation**: emit `lvqr_auth_failures_total{entry="admin"}` from
the admin middleware on every `AuthDecision::Deny`.

### No rate limiting anywhere

Every auth surface accepts unbounded request volume. RTMP publish
handshake, WS ingest/subscribe upgrade, MoQ session accept, admin API
-- all of them will happily validate a million per-second auth attempts
with no backoff.

**Recommendation**: Tier 3 work. Add a small `tower::limit::RateLimit`
layer around the admin router and a per-IP accept budget on the WS and
MoQ paths. Not blocking for v0.4.

### lvqr-signal has no input validation

**File**: `crates/lvqr-signal/src/signaling.rs:105-124` and elsewhere.

Peer IDs and track names are deserialized from untrusted JSON and
immediately logged plus used in message routing. No length bound, no
character set restriction, no rate limit on peer registrations. A
malicious client can open many WebSocket connections and register
arbitrarily many peers with arbitrary bytes, exhausting server memory
and polluting logs.

**Recommendation**: Tier 2 work. Add a `validate_peer_id` helper that
enforces `^[A-Za-z0-9_-]{1,64}$` and reject non-matching registrations.
Cap registrations per connection at 1.

## Architectural Observations

### lvqr-mesh is scaffolding, not a working relay

The `MeshCoordinator` is a pure topology tracker. It assigns peers to a
tree, distributes children, and reassigns orphans. That is the control
plane.

What it does not do: hand the `PeerAssignment` to anything that
actually establishes a WebRTC DataChannel or forwards media to a
parent. There is no code in the repository that reads
`assignment.parent` and opens a peer connection.

The mesh `CONNECTED` callback wires peer IDs to the coordinator, and
the signal server pushes `AssignParent` messages, but the actual media
forwarding is not implemented. The mesh offload percentage reported by
the admin API is therefore informational fiction: the percentage counts
how many peers are *supposed to* be served by other peers, not how
many actually are.

**Not a bug** -- this is scaffolding that was understood to be incomplete.
The audit dated 2026-04-13 (external) already lists "WebRTC mesh peer to
peer media relay" as "not started" under "What's Not Done".

**Recommendation**: add a comment at the top of `lvqr-mesh/src/lib.rs`
making it explicit that the crate is a topology planner and does not yet
drive real peer connections. Do not remove the crate; the topology
logic is correct and will be reused when the WebRTC mesh DataChannel
work lands in Tier 4.

### lvqr-record is tested only at the helper level

`record_track`, the async function that actually reads MoQ groups and
writes fMP4 segments to disk, has zero test coverage. The unit tests
at `recorder.rs:165-197` only exercise pure helpers (`looks_like_init`,
`track_prefix`, `sanitize_name`).

**Recommendation**: Tier 1 follow-up. Add an integration test that
spawns a `BroadcastRecorder` in a `tempfile::tempdir`, subscribes to a
synthesized MoQ broadcast with a known init + media frame pair, and
asserts the on-disk layout matches the documented structure.

### GopCache reads clone the entire Vec<Frame>

`crates/lvqr-core/src/gop.rs:79, 86-89, 95` -- every read clones a
`Gop` struct containing `Vec<Frame>`. For a 30fps 6-second GOP, that is
180 `Frame` clones per read. Frames use `Bytes` for payload, so the
bytes are cheap, but the Vec allocation is not.

Not a hot path (only on late-join), and `lvqr-core::GopCache` is itself
dead code per the inventory above. Flagging for completeness only.

### fMP4 esds descriptor uses single-byte length encoding

**File**: `crates/lvqr-ingest/src/remux/fmp4.rs:314-344`

The `esds` box writer emits MPEG-4 Elementary Stream descriptors with
single-byte length prefixes:

```rust
buf.put_u8(0x03);                         // ES_DescrTag
let asc_len = config.asc.len();
let decoder_config_len = 13 + 2 + asc_len;
let es_desc_len = 3 + 2 + decoder_config_len + 3;
buf.put_u8(es_desc_len as u8);            // length
```

MPEG-4 descriptor lengths are a variable-length encoding: byte with high
bit set means "more bytes follow". For ASC sizes up to ~113 bytes the
single-byte form works; above that the descriptor is malformed and
ffprobe will reject the output.

**Real impact today**: zero. LVQR only emits AAC-LC with 2-byte ASC
(`[0x12, 0x10]` in the tests), so ASC size is fixed at 2. A conformant
codec writer must handle >127 byte lengths for HE-AAC with SBR+PS or
xHE-AAC, but LVQR does not support those codecs today.

**Recommendation**: Tier 2.2 `lvqr-codec` crate will replace this
hand-rolled writer with `mp4-atom`. Flag in the audit and do not patch
the current writer; the fix lives in the crate that replaces it.

### patch_trun_data_offset is a walk, not a patch

**File**: `crates/lvqr-ingest/src/remux/fmp4.rs:499-527`

After writing the moof, the code computes the data_offset and then
scans the entire moof box for the trun to patch the placeholder. This
is wasteful -- the writer could have remembered the offset when it
wrote the placeholder (line 412-413 even captured it, then threw it
away with `let _ = data_offset_pos`). The walk is O(depth of box
hierarchy), so not catastrophic, but the captured offset is the more
obvious design.

**Severity**: cosmetic. Performance is not meaningful compared to the
base_dts and per-frame work elsewhere.

## Cross-Cutting Observations

- **StaticAuthProvider constant-time compare is correct** (verified:
  `static_provider.rs:57-66` compares length first, then XORs all
  bytes, then checks `diff == 0`). Not a timing oracle.
- **StaticAuthProvider `from_env` exists but is not called** anywhere;
  lvqr-cli manually reads each CLI arg/env var separately. Harmless
  duplication.
- **The relay's `run` method and the CLI's `accept_loop` path are
  parallel code** (`crates/lvqr-relay/src/server.rs:116-121` vs. the
  CLI-driven path at `main.rs:328-352`). The CLI path is the real one.
  `run` only exists for the library consumers of `RelayServer`.
- **No `todo!()`, `unimplemented!()`, or `panic!()` on user input** in
  any crate I audited. Every input path either returns an error or a
  fallback value. That is the single biggest thing the codebase has
  going for it.
- **28 test binaries, 2560 generated proptest cases, all green** as of
  this audit. The theatrical tests flagged in Tier 0 are gone. The
  only remaining theatrical test is the heartbeat one in lvqr-mesh.
- **No dependency graph cycles**. Every crate has a clean one-way
  dependency on the crates below it in the tier list from the
  roadmap.
- **No `#[allow(unused)]` in shipped code** except for the deprecation
  shim in `lvqr-wasm::LvqrClient.track`, which is there intentionally.
- **No `unsafe` blocks in any shipped crate**. LVQR is 100% safe Rust.

## Fix Plan for This Session

Landing in the same commit as this audit:

1. Validator for broadcast names in `lvqr-relay::parse_url_token`,
   plus a unit test that rejects traversal attempts.
2. Defensive old-parent cleanup in `lvqr-mesh::reassign_peer`, plus a
   regression test that exercises a live rebalance (reassign without
   remove) and asserts the old parent has no stale children.
3. `--jwt-secret` CLI flag wired to `JwtAuthProvider`.
4. Comment at the top of `lvqr-mesh/src/lib.rs` stating that the
   topology planner is not yet wired to real WebRTC forwarding.
5. Fix the heartbeat theatrical test (use 1s timeout, assert real
   behavior).

Deferred, tracked in this audit:

- Delete `lvqr-core::{Registry, RingBuffer, GopCache}` when
  `lvqr-fragment` lands in Tier 2.1.
- Delete `lvqr-wasm` in v0.5.
- Admin auth-failure metric. Tier 3.
- CORS restrict. Tier 3.
- Rate limits. Tier 3.
- lvqr-signal input validation. Tier 2.
- lvqr-record integration test. Tier 1 follow-up.
- fMP4 esds multi-byte descriptor length encoding. Handled by
  `lvqr-codec` in Tier 2.2 replacing the hand-rolled writer.

## Bottom Line

The foundation is not rotten. The confirmed bugs are narrow, the dead
code is clearly marked, and the one feature that is complete-but-unwired
(JWT) is a 30-line CLI fix. The biggest structural question -- whether
Registry/RingBuffer/GopCache should survive -- is already answered by
Tier 2.1 replacing them. Everything else is hardening that lives
comfortably inside Tier 2 or Tier 3.

The audit confirms the strategic bets from the external audit. Tier 2
remains the load-bearing call. Nothing in this internal review changes
the plan; it only adds five specific fixes to land before Tier 2 starts.
