# LVQR Test Audit -- 2026-04-28

Workspace test surface at audit time:
- **1,295 Rust `#[test]` declarations** across **217 files** (unit
  tests in `src/*.rs` `#[cfg(test)]` modules + integration tests in
  `crates/*/tests/`)
- 11 Vitest specs on the JS SDK packages
- 5 Playwright e2e specs
- 1 Python pytest module

Four parallel `Explore` agents audited the suite by domain
(security-critical, wire-format, hot-path, integration / cluster).
Each surfaced ~10-15 specific shallow tests with file:line cites.
Total surface: **~50 distinct findings**.

This document is the remaining backlog after the 2026-04-28 commit
hardened the highest-priority items. Each entry is grouped by
priority + has a one-line "why shallow" + "what real test should
look like" summary so a follow-up session can pick any row and
make progress.

---

## Already shipped in this audit cycle

| What | Where |
|---|---|
| **JWT expiration enforcement** (`exp = past` rejected for admin / subscribe / publish) | `crates/lvqr-auth/src/jwt_provider.rs::tests::expired_token_is_rejected_*` |
| **JWT wrong-secret rejection** (token signed with attacker key denied) | `crates/lvqr-auth/src/jwt_provider.rs::tests::token_signed_with_wrong_secret_is_rejected` |
| **JWT tampered-payload rejection** (mutating one base64url byte fails HMAC verify) | `crates/lvqr-auth/src/jwt_provider.rs::tests::token_with_tampered_payload_is_rejected` |
| **Stream-key real-time TTL expiry** (mint ttl=1, sleep 1.5s, assert lookup misses) | `crates/lvqr-auth/src/stream_key_store.rs::tests::mint_with_one_second_ttl_actually_expires_via_real_time` |
| **Stream-key TTL math window** (mint ttl=60, assert expires_at in [now+60, after+60]) | `crates/lvqr-auth/src/stream_key_store.rs::tests::mint_with_ttl_seconds_sets_expires_at_within_window` |
| **SCTE-35 mutated-byte proptest** (256 cases of single-byte XOR mutation; assert no panic + CRC consistency on accept) | `crates/lvqr-codec/src/scte35.rs::tests::parse_handles_arbitrary_byte_mutations_without_panic` |
| **HW encoder pipeline_str round-trip** (catches copy-paste swap of nvh264enc <-> vah264enc <-> qsvh264enc) | `crates/lvqr-transcode/src/{nvenc,vaapi,qsv,videotoolbox}.rs::tests::pipeline_str_uses_*_with_documented_property_mapping` |

---

## Priority 1 -- Security boundaries still under-covered

### P1.1 -- JWKS happy-path-only test surface

`crates/lvqr-auth/src/jwks_provider.rs:449-470` --
`happy_path_accepts_signed_ed25519_token` only exercises a static
JWKS endpoint with one key. No coverage for:

- Token expiration enforcement (already fixed in the symmetric
  HS256 path; mirror the same shape here).
- Mismatched issuer (tokens with `iss` not in the configured
  allowlist).
- Key rotation: start with kid-1 in JWKS, swap to kid-2; assert
  tokens signed with kid-1 are rejected after the refresh.
- Algorithm downgrade: token with `alg=none` or `alg=HS256` (using
  the public key as the HMAC secret) -- must be rejected.

### P1.2 -- HotReloadAuthProvider race-correctness

`crates/lvqr-auth/src/hot_reload_provider.rs:144-170` --
`check_holds_guard_for_duration_of_call` proves the provider does
not deadlock under concurrent swap + check, but does not prove the
swap is **observable**. After a swap from provider-A to provider-B,
at least some subsequent checks must reflect provider-B's policy
(otherwise the swap was a no-op).

**Real test**: install provider A (admin_token="v1"), spawn 50
concurrent checks while atomically swapping to provider B
(admin_token="v2"); assert the post-swap call distribution shows
v1-tokens rejected on >0% of checks (proving the swap took effect
for at least some).

### P1.3 -- Webhook auth bad-JSON deny

`crates/lvqr-auth/src/webhook_provider.rs` -- no test that a
webhook returning `200 OK` with malformed JSON denies the request
(rather than crashing or accepting). The TTL cache also lacks a
"cached deny survives N seconds, then re-fetched" test.

### P1.4 -- C2PA tamper detection round-trip

`crates/lvqr-archive/tests/c2pa_sign.rs:75-97` -- only asserts the
signed manifest is non-empty. No test that signs an asset, mutates
one byte in the signed bytes, and verifies the resulting manifest
fails validation. This is the load-bearing claim of the C2PA
surface (the verify endpoint exists but isn't exercised against
known-bad inputs).

**Real test**: sign a small JPEG/MP4 fixture, write to disk, read
back with `c2pa::Reader`, mutate one byte in the asset, re-read
and assert validation fails with an integrity-violation reason.

### P1.5 -- Multi-key store revoke + fallback interaction

`crates/lvqr-auth/src/multi_key_provider.rs:170-184` -- tests scope
mismatch with a Noop fallback. Missing case: a stream-key with
token T, revoked, where the fallback (e.g., a JWT provider) would
recognise T independently. The current behaviour is "revoke wins"
because the fallback doesn't share the token; a regression that
made the multi-key provider ignore revoke when the fallback
allows would not be caught.

---

## Priority 2 -- Wire-format coverage gaps

### P2.1 -- Zero external validators on the merge gate

The HLS conformance harness at
`crates/lvqr-hls/tests/conformance_manifest.rs:32-125` soft-skips
when Apple's `mediastreamvalidator` is missing -- which is the
default state on every CI runner. The DASH equivalent
(`crates/lvqr-dash/tests/golden_mpd.rs:68-75`) compares against
hardcoded golden strings instead of any external validator.

**Recommendation**: install GPAC's `MP4Box` on the conformance CI
lane (apt-installable on Ubuntu) and use it to validate the MPD
schema + DASH-IF interoperability. For HLS, run the rendered
playlist through `hls.js`'s parser via a Node script in the SDK
test harness.

### P2.2 -- DASH MPD string-equality "golden" tests

`crates/lvqr-dash/tests/golden_mpd.rs` -- compares against a
hardcoded XML string. Brittle (any whitespace change fails) and
proves nothing about the MPD's parseability by a real client. Many
DASH-IF compliance issues cannot be caught this way (e.g.,
`SegmentTemplate` math, codec attribute well-formedness).

**Real test**: parse the rendered XML with `quick-xml` or
`roxmltree`, validate the namespace, extract `AdaptationSet` count
+ codec strings + presentationDelay, assert structure matches
expected. Then re-render and round-trip.

### P2.3 -- HLS playlist tests are string-prefix only

`crates/lvqr-hls/src/master.rs:256-263` -- asserts
`body.starts_with("#EXTM3U\n#EXT-X-VERSION:9")`. Never validates
that EXT-X-MEDIA + EXT-X-STREAM-INF entries are well-formed, that
URIs are on new lines, or that DATERANGE hex is uppercase per the
HLS spec.

**Real test**: re-parse the rendered playlist with the m3u8
crate (or equivalent), extract tags by type, assert the model
round-trips cleanly. Add a proptest that drives random rendition
sets and asserts every emitted playlist parses + every tag
attribute conforms to the HLS draft RFC 8216bis.

### P2.4 -- RTMP onCuePoint scte35-bin64 happy-path-only

`crates/lvqr-ingest/tests/scte35_rtmp_oncuepoint_e2e.rs:36-76` --
only valid base64-encoded SCTE-35. No coverage for:

- Truncated section bytes
- Invalid CRC at the publisher side
- Oversized data (>4 KB; should trip a guard)
- Non-base64 characters in the AMF0 string
- Replay (same `splice_event_id` twice in flight)

### P2.5 -- SRT MPEG-TS PMT 0x86 reassembly under fragmentation

`crates/lvqr-srt/src/ingest.rs::tests` (added in session 160) --
the proptest harnesses cover `split_annex_b` / `annex_b_to_avcc` /
`annex_b_to_hvcc` on arbitrary byte slices. The SCTE-35
private-section reassembly across TS packet boundaries is covered
only by the positive `scte35_section_with_valid_crc_publishes_*`
test.

**Real test**: proptest that generates a fragmented section
(across N packets where N > 1), interleaves with non-SCTE PMT
streams, asserts reassembly produces the same bytes as the
contiguous reference. Add a malformed case where the section
length declared on the wire exceeds the actual byte count.

### P2.6 -- WHIP/WHEP str0m bridge wire-shape

WHIP/WHEP tests rely on str0m's own test harness; the LVQR-side
bridges (`crates/lvqr-whip/src/bridge.rs`,
`crates/lvqr-whep/src/lib.rs`) are not exercised against
adversarial SDP offers (oversized fields, malformed transport
attributes, ICE-trickle race conditions for inbound).

---

## Priority 3 -- Hot-path correctness

### P3.1 -- AVC / HEVC parsers proptest only for panic-freedom

`crates/lvqr-codec/tests/proptest_aac.rs:18-27` -- asserts
"successful parses have plausible sample rates" but never validates
that the parsed `frequency_index` matches the spec table or that
DSP parameters (frame length, redundancy) are correct.

`crates/lvqr-codec/tests/proptest_hevc.rs:20-24` --
`nal_type_from_any_u8_is_total` is a tautology (the conversion
function is total by construction).

**Real test**: assert each NAL type maps to the correct slice type
per ITU-T H.265 Table 7-1, and that only valid NAL types parse via
`parse_nal_unit` non-trivially (i.e., the parser doesn't silently
accept reserved values).

### P3.2 -- CMAF init segment magic-byte-only validation

`crates/lvqr-cmaf/src/init.rs::avc_init_segment_starts_with_ftyp_and_contains_moov`
only checks for "ftyp" and "moov" substrings in the rendered
bytes. Never validates that `moov` contains a sane `mvhd`,
`trak`, or `trun` atom; never round-trips through a parser.

**Real test**: decode the output with `mp4_atom::Decode` (already
in the workspace), assert `mvhd.timescale`, `trak[].tkhd.{width,
height}`, and `trun` sample count match the input.

### P3.3 -- CMAF segmenter ordering under contention

`crates/lvqr-cmaf/tests/integration_segmenter.rs:114-124` -- only
tests `next_chunk().await.is_none()` on an empty stream. No
coverage for:

- DTS-monotonic emission under concurrent push + drain (race-prone)
- Backpressure: what happens when the SampleStream is slower than
  the producer (current code assumption: drop or block?)

### P3.4 -- Fragment broadcaster lacks ring-buffer-lag test

`crates/lvqr-fragment/src/fragment.rs::fragment_new_preserves_fields`
is a compile-time tautology. The actual lifecycle test
(`broadcast -> subscribe -> drop`) doesn't include a slow consumer
that lags into ring-buffer eviction territory.

**Real test**: drive 100 fragments through a broadcaster with a
deliberately stalled consumer; assert the consumer either receives
a contiguous prefix and a clean "behind by N" signal, or the
contract surfaces eviction explicitly. Today the assertion would
catch a regression that silently drops middle fragments without
signaling.

### P3.5 -- Agent runner panic isolation untested

`crates/lvqr-agent/tests/integration_basic.rs:70-125` -- happy-path
lifecycle (start -> fragments -> stop). No test that an agent
panicking during `on_fragment` increments
`handle.panics()` and that subsequent fragments still reach OTHER
agents on the same broadcast.

**Real test**: factory returns an agent that panics on the second
fragment; assert `handle.panics() == 1`, assert the other agent
on the same broadcast received all fragments unaffected.

---

## Priority 4 -- Integration test hardening

### P4.1 -- Blind `tokio::time::sleep` in 30+ integration tests

The full list (file:line) of fixed-sleep call sites:

- `crates/lvqr-cli/tests/rtmp_dash_e2e.rs:123, 179, 195` (3 x
  500 ms before HTTP GET)
- `crates/lvqr-cli/tests/rtmp_archive_e2e.rs:78, 148, 340` (50 ms
  + 500 ms x2)
- `crates/lvqr-cli/tests/playback_signed_url_e2e.rs:81, 183, 298`
  (50 ms + 500 ms x2)
- `crates/lvqr-cli/tests/wasm_filter_admin_route.rs:51, 121` (50 ms
  + 400 ms)
- `crates/lvqr-cli/tests/srt_dash_e2e.rs:182` (1000 ms)
- `crates/lvqr-cli/tests/rtsp_hls_e2e.rs:159` (1500 ms)
- `crates/lvqr-cli/tests/wasm_frame_counter.rs:56, 129` (50 ms +
  400 ms)
- `crates/lvqr-cli/tests/transcode_ladder_e2e.rs:94, 275` (50 ms
  + 200 ms loop without deadline)
- `crates/lvqr-cli/tests/rtmp_whep_audio_e2e.rs:*`
- `crates/lvqr-rtsp/tests/play_integration.rs:625` (6 s)
- `crates/lvqr-test-utils/tests/moq_timing_e2e.rs:95` (2 s)
- `crates/lvqr-record/tests/record_*.rs` (2 x 2 s)
- `crates/lvqr-agent-whisper/tests/whisper_basic.rs:182` (10 s)

Each one races on macOS CI (the source of session 162's CI
saga). Replace with `wait_until(closure, deadline)` polling.
The codebase already has a healthy active-wait helper at
`crates/lvqr-cli/tests/cluster_redirect.rs:*` and the federation
test's `Instant::now() + CONNECT_TIMEOUT` pattern; mirror it.

**Recommendation**: a workspace-wide `lvqr_test_utils::wait_until`
helper (if not already present) so every integration test uses
the same polling shape.

### P4.2 -- Cluster ownership crash failover untested

`crates/lvqr-cluster/tests/ownership.rs` -- tests claim + lease
propagation, but no test for:

- Owner crashes (SIGKILL) -- does another node observe `owner ==
  None` after the lease TTL?
- Owner network-partitions -- same.
- Owner renews the lease in a long-running task -- does the
  renewer keep going for >1 TTL window, or does it drop out
  prematurely?

**Real test**: per the audit, an
`claim_survives_renewer_loop_over_ttl` test that drops the
`Claim` mid-loop and asserts another node sees `None` after the
TTL.

### P4.3 -- Mesh data plane: no Rust-side three-node forwarding test

The Playwright e2e suite exercises browser-to-browser DataChannel
forwarding (sessions 115 + 142). The Rust-side mesh tests cover
signal registration + topology assignment but never assert that a
frame published on the root peer reaches a depth-2 leaf via
relay-1. Today this is implicit (via Playwright); a Rust-side
deterministic version would catch regressions on the data path
without needing a browser.

### P4.4 -- SLO histogram percentile sanity

`crates/lvqr-cli/tests/slo_latency_e2e.rs:148-156` -- asserts
`p50 >= 150 && p99 >= p50`. Passes even if the histogram is
completely wrong (e.g., all samples are 0).

**Real test**: drive 8 fragments at known ingest times spaced 5 ms
apart. Compute expected p50 (~150 + 4*5 / 2 = ~160 ms) and assert
the route's reported p50 is within +/- 10 ms of expected.

### P4.5 -- Federation `forward_track` race on macOS

`crates/lvqr-cli/tests/federation_two_cluster.rs` -- known-broken
on `macos-latest` GitHub-hosted runners. The federation MoQ
session reaches `Connected` and the announcement propagates, but
every subsequent `next_group` returns `code=13` for the full
`PROPAGATION_TIMEOUT`. Linux + local macOS dev pass.

**Investigation owed**: instrument the
`lvqr_cluster::federation::forward_track` task with timing logs;
reproduce on a fresh macos-latest runner; identify whether the
issue is in moq-native's QUIC connect, the per-track subscribe,
or the LVQR-side forward loop. Filed against v1.2.

### P4.6 -- `LVQR_LIVE_RTMP_TESTS` env-gated tests

Several Playwright + Rust integration tests gate on
`LVQR_LIVE_RTMP_TESTS=1` to opt into ffmpeg-driven flows. CI sets
this on `mesh-e2e.yml` but not all workflows. Result: some flows
silently soft-skip in some lanes.

**Recommendation**: audit which workflows set the env var, ensure
every CI lane that should exercise the flow does so, and emit a
visible `[SKIP] missing LVQR_LIVE_RTMP_TESTS` log line when not
set so log analysis can count skipped vs passed cleanly.

---

## Aggregate posture

After the 2026-04-28 fixes, the workspace's security-critical
boundaries (JWT expiration, JWT signature verification, stream-key
TTL, SCTE-35 CRC mutation) have **adversarial proof tests**.
Wire-format and hot-path coverage is still largely happy-path-only
and would benefit from external validators (mediastreamvalidator,
GPAC MP4Box, dash.js) run against the rendered output rather than
hardcoded string comparisons.

**Recommended sequencing for follow-up sessions:**

1. **Wire-format conformance lift** (P2.1 + P2.2 + P2.3): install
   GPAC + run hls.js parser, gate at least one HLS + DASH test on
   real-tool acceptance. Highest leverage for "passes our tests,
   breaks real client" defense.
2. **Integration-test sleep migration** (P4.1): mechanical, large
   surface, big stability win on macOS CI.
3. **JWKS / webhook adversarial coverage** (P1.1 + P1.3): mirror
   the HS256 hardening shape onto the asymmetric / webhook paths.
4. **C2PA tamper round-trip** (P1.4): the headline provenance
   claim deserves a test against known-bad input.
5. **Cluster crash failover** (P4.2): the ownership lease is the
   one cluster-side correctness invariant we have not actually
   tested fail-recover behaviour for.

This document is the work backlog. Each entry is independently
actionable in a single session.
