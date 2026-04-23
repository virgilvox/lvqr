# Tier 4 demos

Scripted showcases for the Tier 4 programmable data plane --
WASM per-fragment filters, whisper.cpp live captions, server-side
ABR transcoding, DVR archive finalize, and C2PA provenance.

Each demo runs a single self-contained `lvqr serve` process on
non-default ports against a scratch directory, publishes a
synthetic stream into it, then asserts the relevant Tier 4
surfaces actually ran end to end. Demos clean up after themselves
on exit.

## Demos

### `demo-01.sh` -- full-chain showcase

Boots an `lvqr serve` with:

- WASM per-fragment filter (`--wasm-filter`) using the in-repo
  `frame-counter.wasm` fixture.
- Whisper live captions (`--whisper-model`), conditional on
  `LVQR_WHISPER_MODEL` being set.
- Software ABR transcode ladder (`--transcode-rendition`) with
  720p + 480p + 240p renditions.
- On-disk DVR archive (`--archive-dir`) that finalizes each
  track on publisher disconnect.

A 20-second synthetic publish runs via ffmpeg (colour bars +
sine wave), and the script polls the HLS master playlist until
all four ABR rungs are advertised. The expected runtime is
~30 seconds on a workstation (20 s of ffmpeg publish + ~10 s of
boot + finalize).

Run it from the repo root:

```bash
./examples/tier4-demos/demo-01.sh
```

Success criteria printed in the final summary block:

- **hls variants**: `4 advertised` (source + 720p + 480p + 240p).
- **wasm tap keep**: non-zero fragment count.
- **archive video**: a finalized MP4 per track under the scratch
  archive directory.
- **captions**: `playlist: 200` when `LVQR_WHISPER_MODEL` is set.

The script exits non-zero if the ABR ladder or archive assertion
fails.

## Prerequisites

All demos share a common prereq set. The scripts fail fast when
any is missing with a pointer back here.

### 1. An `lvqr` binary with the Tier 4 feature set

```bash
cargo build --release -p lvqr-cli --features full
```

The `full` feature set enables `c2pa` + `whisper` + `transcode`
on top of the defaults. The script probes for
`--transcode-rendition` in `lvqr serve --help` and refuses to
proceed on an underfeatured binary.

The script also accepts a debug build at
`target/debug/lvqr`, or `lvqr` already on `$PATH`. Override the
choice explicitly via `LVQR_BIN`.

### 2. Runtime binaries

- `ffmpeg` (synthetic RTMP publisher).
- `curl` + `jq` (admin API probes and JSON parsing).
- `gst-launch-1.0` -- proxy for a working GStreamer install. The
  transcode feature loads GStreamer at runtime; without it the
  ABR rungs never materialize.

#### macOS

```bash
brew install ffmpeg curl jq gstreamer gst-plugins-base \
  gst-plugins-good gst-plugins-bad gst-plugins-ugly gst-libav
```

#### Debian / Ubuntu

```bash
sudo apt install ffmpeg curl jq \
  gstreamer1.0-tools \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-plugins-ugly \
  gstreamer1.0-libav
```

### 3. Optional: a whisper.cpp model

`demo-01.sh` enables the live-captions agent only when
`LVQR_WHISPER_MODEL` points at a real `ggml-*.bin` file. Without
it the rest of the demo still runs.

Download the smallest English-only model (~78 MB):

```bash
curl -L -o /tmp/ggml-tiny.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin
export LVQR_WHISPER_MODEL=/tmp/ggml-tiny.en.bin
```

Larger English or multilingual models from the same repo work
identically; the captions agent loads whatever ggml file the
env var points at.

The repo does not ship the model binary -- at ~78 MB it would
dominate every clone, and the canonical distribution lives on
Hugging Face.

## Environment knobs

All are optional.

| Variable | Effect |
|---|---|
| `LVQR_WHISPER_MODEL` | Path to a `ggml-*.bin`. Enables captions. |
| `LVQR_BIN` | Override the lvqr binary path. |
| `LVQR_DEMO_SCRATCH` | Override the scratch directory. When set, the scratch dir is retained after exit for inspection. |
| `LVQR_DEMO_DURATION` | Publish duration in seconds. Default 20. |
| `LVQR_DEMO_ADMIN_PORT` | Admin port. Default 18080. |
| `LVQR_DEMO_HLS_PORT` | HLS port. Default 18888. |
| `LVQR_DEMO_RTMP_PORT` | RTMP port. Default 11935. |
| `LVQR_DEMO_MOQ_PORT` | MoQ port. Default 14443. |

Non-default ports keep the demo clear of a locally-running
`lvqr serve` on the zero-config defaults.

## On C2PA provenance

C2PA signing + verify is exposed on the CLI via
`--c2pa-signing-cert` + `--c2pa-signing-key` (plus
`--c2pa-signing-alg`, `--c2pa-assertion-creator`,
`--c2pa-trust-anchor`, `--c2pa-timestamp-authority`); see the
main [`README.md`](../../README.md#cli-reference) for the
full CLI-reference block and
`crates/lvqr-archive/src/provenance.rs` for the underlying
`C2paSignerSource` enum (on-disk PEMs and caller-supplied
signers).

The demo includes an opt-in for C2PA sign + verify via
`LVQR_DEMO_C2PA=1`:

```bash
LVQR_DEMO_C2PA=1 ./examples/tier4-demos/demo-01.sh
```

When enabled, the demo shells out to `openssl` to mint an
ephemeral CA + leaf + PKCS#8 key in the scratch dir, passes
the PEMs to `lvqr serve --c2pa-signing-cert` +
`--c2pa-signing-key`, and after the publish curls
`/playback/verify/live/demo` to print `valid` +
`validation_state` + `signer`. The openssl recipe is locked
into CI via `crates/lvqr-cli/tests/c2pa_cli_flags_e2e.rs`
(`openssl_generated_certkeyfiles_also_yields_valid_manifest`),
which mints the same material in-test and asserts
c2pa-rs accepts it end to end.

Two integration tests cover both operator-facing signer
paths end-to-end:

```bash
# on-disk CertKeyFiles flow (this session's addition):
cargo test -p lvqr-cli --features c2pa \
  --test c2pa_cli_flags_e2e

# programmatic Custom(Arc<dyn Signer>) flow via EphemeralSigner:
cargo test -p lvqr-cli --features c2pa \
  --test c2pa_verify_e2e
```

## Troubleshooting

### `lvqr binary missing --transcode-rendition flag`

The build does not include the `transcode` feature. Rebuild:

```bash
cargo build --release -p lvqr-cli --features full
```

### `master playlist never reached 4 variants`

The transcode pipeline failed to start. Common causes:

- GStreamer plugin set incomplete. Verify with
  `gst-inspect-1.0 x264enc` -- it must list the element.
- A previous run is still holding the HLS port. Check
  `lsof -i :18888`; kill stragglers or set `LVQR_DEMO_HLS_PORT`.
- The lvqr binary was built without the `transcode` feature. See
  above.

Inspect `scratch/lvqr.log` by setting `LVQR_DEMO_SCRATCH=/tmp/foo`
before invoking the script so the scratch dir is retained.

### `archive video track did not materialize`

The RTMP publisher disconnected but the drain task never saw
`next_fragment -> None`. Usually a symptom of the ffmpeg client
being killed abruptly (instead of exiting at the end of
`-t DURATION`); the demo always waits for ffmpeg to finish
its `-t` window so this is rare in practice.

### `WHISPER_MODEL points at '...' but the file does not exist`

`LVQR_WHISPER_MODEL` must be an absolute path to an existing
ggml file. See the download recipe above.

## What this covers

The file `tracking/TIER_4_PLAN.md` names a working
`examples/tier4-demos/` demo script as the Tier 4 exit criterion.
`demo-01.sh` is that script.

Coverage by Tier 4 item:

| Tier 4 item | Surface | Covered by demo-01 |
|---|---|---|
| 4.1 WASM filters | `--wasm-filter` + `lvqr_wasm_fragments_total` | yes |
| 4.2 io-uring archive | `--archive-dir` (Linux io-uring behind `io-uring` feature) | yes (archive finalize) |
| 4.3 C2PA provenance | `--c2pa-signing-cert` + siblings | yes, opt-in via `LVQR_DEMO_C2PA=1` |
| 4.4 Cross-cluster federation | `--cluster-listen` / `FederationLink` | out of scope for a single-node demo |
| 4.5 AI agents | `--whisper-model` + captions rendition | yes (when model is provided) |
| 4.6 ABR transcoding | `--transcode-rendition` + master playlist | yes |
| 4.7 Latency SLO | `/api/v1/slo` + Grafana alert pack | not exercised (no subscribers in this demo) |
| 4.8 One-token auth | `--subscribe-token` + `--jwt-secret` | out of scope (auth off for demo simplicity) |
