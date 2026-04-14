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

Crates currently in scope (per `scripts/check_test_contract.sh` IN_SCOPE
list) and their 5-artifact status as of 2026-04-13 (session 10 close):

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest (FLV, fMP4, RTMP) | yes (`tests/proptest_parsers.rs`) | yes (`fuzz/fuzz_targets/{parse_video_tag,parse_audio_tag}.rs`) | yes (`tests/rtmp_bridge_integration.rs`) | yes (`../lvqr-cli/tests/rtmp_ws_e2e.rs` plus `tests/e2e/test-app.spec.ts`) | golden + ffprobe (`tests/golden_fmp4.rs`) |
| lvqr-record | yes (`tests/proptest_recorder.rs`) | open (pure helpers already proptest-covered) | yes (`tests/record_integration.rs`) | workspace `tests/e2e/` | yes (`tests/record_conformance.rs`) |
| lvqr-moq | yes (`tests/proptest_facade.rs`) | open (pure value-type facade) | yes (`tests/integration_facade.rs`) | via `rtmp_ws_e2e` | n/a |
| lvqr-fragment | yes (`tests/proptest_fragment.rs`) | open (pure value type) | yes (`tests/integration_moq_sink.rs`) | via `rtmp_ws_e2e` | n/a |
| lvqr-codec | yes (`tests/proptest_{hevc,aac}.rs`) | yes (`fuzz/fuzz_targets/{parse_hevc_sps,parse_aac_asc,read_ue_v}.rs`) | yes (`tests/integration_codec.rs`) | via `rtmp_ws_e2e` | yes (`tests/conformance_codec.rs`; iterates the `lvqr-conformance` codec corpus including the kvazaar multi-sub-layer fixture) |
| lvqr-cmaf | yes (`tests/proptest_policy.rs`) | open (no parser attack surface; consumes trusted `Bytes`) | yes (`tests/integration_segmenter.rs`, `tests/integration_sample_segmenter.rs`, `tests/parity_avc_init.rs`, `tests/parity_avc_segment.rs`) | via `rtmp_ws_e2e` | yes (`tests/conformance_init.rs` + `tests/conformance_coalescer.rs`; ffprobe-validated AVC + HEVC + AAC init segments and ffprobe-validated AVC + AAC coalescer media segments) |
| lvqr-hls | yes (`tests/proptest_manifest.rs`) | open (no parser attack surface; renderer reads structured input) | yes (`tests/integration_builder.rs` + `tests/integration_server.rs` driving the axum router via `tower::ServiceExt::oneshot`) | via router oneshot; TCP loopback E2E lands with the `lvqr-cli` HLS composition | yes (`tests/conformance_manifest.rs`; Apple `mediastreamvalidator` soft-skip via `lvqr_test_utils::mediastreamvalidator_playlist`) |

Gaps relative to the contract are tracked in the Tier 1/2 work list.
Priorities closed in sessions 6 through 10:

* `mediastreamvalidator` soft-skip helper: landed session 8
  (`lvqr_test_utils::mediastreamvalidator_playlist`) and exercised
  by `lvqr-hls`'s `tests/conformance_manifest.rs`.
* Multi-sub-layer HEVC SPS fixture: landed session 7
  (`crates/lvqr-conformance/fixtures/codec/hevc-sps-kvazaar-main-320x240-gop8.{bin,toml}`)
  via kvazaar 2.3.2 with `--gop 8`. x265 refused to emit
  `sps_max_sub_layers_minus1 > 0` under every configuration tried
  in session 5.

Remaining immediate priorities:

1. Audio media-segment fixture for `lvqr-ingest` esds conformance
   tests to exercise >127-byte `AudioSpecificConfig` payloads
   through the full ffprobe round-trip. LVQR's own AAC writer
   already rejects unusual configs with a typed error, so this is
   a lower-priority hardening item for the hand-rolled path that
   will be retired behind a feature flag in session 11.
2. TCP-loopback E2E for `lvqr-hls` once the `lvqr-cli` HLS
   composition lands in session 11. The current router E2E runs
   over `tower::ServiceExt::oneshot` rather than a real TCP
   socket; upgrading it to TCP requires `lvqr-test-utils::TestServer`
   to grow an HLS bind address, which in turn requires the CLI
   serve path to compose `HlsServer::router`.

## Enforcement

Tier 1 (now): educational. `.github/workflows/contract.yml` runs
`scripts/check_test_contract.sh` on every PR and push with
`continue-on-error: true`. Missing slots surface as GitHub Actions
warning annotations on the affected crate's `Cargo.toml` so
contributors see them on the Checks tab without opening logs.

Tier 2 (soon): set `LVQR_CONTRACT_STRICT=1` in the workflow and remove
`continue-on-error`. The script exits non-zero on any missing slot in
strict mode. Specific per-crate E2E exemptions are declared through
`CONTRACT_E2E_EXEMPT_<crate_with_underscores>=1` environment variables
when a crate's E2E legitimately lives in another crate's test binary.

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
