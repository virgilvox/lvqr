# lvqr-conformance

Reference fixtures and conformance harnesses for LVQR. Not published to
crates.io (`publish = false`). Used as a dev-dependency by other crates.

## Fixture layout

```
fixtures/
  rtmp/          real FLV captures from OBS, ffmpeg, Larix, broadcast gear
  fmp4/          hand-vetted fMP4 init and media segments (golden files)
  hls/           generated playlists paired with Apple validator runs
  dash/          generated MPDs paired with DASH-IF validator runs
  moq/           captured MoQ wire messages for subgroups and catalogs
  edge-cases/    deliberate breakage: truncated tags, malformed boxes,
                 non-monotonic DTS, mid-stream codec changes, etc.
```

Each fixture has a sidecar `.toml` with provenance metadata:

```toml
source      = "OBS 30.0.2 on macOS 14.3"
codec       = "avc1.64001F (H.264 High 3.1)"
container   = "FLV"
duration_ms = 12400
notes       = "Webcam capture at 720p30, AAC-LC 44.1kHz stereo"
license     = "CC-BY-SA; captured by maintainer; safe to redistribute"
```

Fixtures larger than 1 MB are stored via Git LFS. Smaller fixtures are
committed directly. The corpus grows over time; the initial commit ships
an empty scaffold and a README pointing at this contract.

## External validators

The `ValidatorResult` API in `lib.rs` wraps external tooling runs so tests
can assert conformance without hard-failing CI on machines that lack the
tools. Current tools we plan to wrap:

- `ffprobe` for fMP4 and HLS structural sanity
- Apple `mediastreamvalidator` for HLS and LL-HLS spec compliance
- DASH-IF conformance tool for DASH MPD validation
- Pion WHIP / WHEP reference clients for WebRTC interop

Every validator wrapper returns `ValidatorResult::Skipped` when the tool
is not on PATH, so contributor laptops without the full toolchain still
run a useful subset of the test suite.

## Cross-implementation comparison harness

The audit dated 2026-04-13 promoted the MediaMTX comparison harness from a
late-Tier-1 add-on to a first-day CI requirement for Tier 2.5 (LL-HLS).
The harness feeds the same RTMP input into LVQR and MediaMTX, captures both
HLS playlist outputs, and structurally diffs them. Catches silent drift
from spec. Will land here once the LL-HLS egress path exists.
