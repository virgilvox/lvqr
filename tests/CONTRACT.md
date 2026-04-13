# 5-Artifact Test Contract

Every new protocol, parser, or format feature in LVQR must ship with all five
of the artifacts below. This contract is enforced as an educational warning
during Tier 1 and becomes a hard CI gate starting Tier 2 per the roadmap
(`tracking/ROADMAP.md`, decision 9).

## The five artifacts

| # | Artifact | Tool | Canonical location |
|---|---|---|---|
| 1 | Property test | `proptest` | `<crate>/tests/proptest_*.rs` or a co-located `mod tests` |
| 2 | Fuzz target | `cargo-fuzz` + `arbitrary` | `<crate>/fuzz/fuzz_targets/<name>.rs` |
| 3 | Integration test | real network via `lvqr-test-utils::TestServer` | `<crate>/tests/integration_*.rs` |
| 4 | End-to-end test | `playwright` (browser) or `tokio-tungstenite` / `rml_rtmp` (headless) | `<crate>/tests/*_e2e.rs` or `tests/e2e/<feature>.spec.ts` |
| 5 | Conformance check | external validator (`ffprobe`, `mediastreamvalidator`, DASH-IF tool, WHIP reference client) | `<crate>/tests/*_conformance.rs` or `lvqr-conformance::ValidatorResult` |

Golden file regressions count as the conformance slot for hand-rolled
writers like the current fMP4 box writer, until the equivalent external
validator is wired in.

## Which crates are in scope

The contract applies to every crate under `crates/lvqr-{ingest,whip,whep,hls,dash,srt,rtsp,codec,cmaf,archive,moq,fragment,record}`. Pure library crates without a wire format or parser are exempt.

Crates currently in scope and their 5-artifact status as of 2026-04-13:

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest (FLV, fMP4, RTMP) | yes (`tests/proptest_parsers.rs`) | no | yes (`tests/rtmp_bridge_integration.rs`) | yes (`../lvqr-cli/tests/rtmp_ws_e2e.rs`) | golden only (`tests/golden_fmp4.rs`) |
| lvqr-relay | no | no | yes (`tests/relay_integration.rs`) | partial (via lvqr-cli) | no |
| lvqr-core | partial (ringbuf, gop) | no | no | n/a | n/a |
| lvqr-auth | no | no | no | n/a | n/a |
| lvqr-record | no | no | no | no | no |

Gaps relative to the contract are already tracked in the Tier 1 work list.
The immediate priorities are:

1. `cargo-fuzz` target for `parse_video_tag` and `parse_audio_tag` (reuses
   the proptest strategies from `tests/proptest_parsers.rs`, promoted to
   a separate fuzz crate under `crates/lvqr-ingest/fuzz/`).
2. ffprobe-backed conformance wrapper for the fMP4 writer, using the
   helper in `lvqr-test-utils::ffprobe_bytes`.
3. `mediastreamvalidator` wrapper for LL-HLS output once Tier 2.5 is
   underway, wired through `lvqr-conformance::ValidatorResult`.

## Enforcement

Tier 1 (now): educational. New crates that land without all five should
get a PR comment pointing at this document. Existing crates catch up
incrementally via the Tier 1 test-infra backlog.

Tier 2 (soon): CI script under `.github/workflows/` greps the diff for
new modules under the in-scope crates and fails the build if any of the
five artifacts is missing. Educational at first, hard-fail once the
backlog is cleared.

## Rationale

The contract exists because the v0.4 audit found that half of the tests
we shipped were theatrical (helper functions tested in isolation, mock
object-safety assertions, dead-code round-trips). The five artifacts each
cover a different failure mode:

- Property tests catch "the parser panics on arbitrary input".
- Fuzz targets catch "the parser panics on inputs the strategy did not
  cover", especially with the nightly long-run.
- Integration tests catch "the parser works but the subsystem it lives
  in cannot drive real network I/O".
- E2E tests catch "every subsystem works in isolation but they do not
  compose".
- Conformance checks catch "our output is structurally plausible but
  does not match the spec the client side expects".

Any one artifact on its own is insufficient. Requiring all five keeps the
test surface honest.
