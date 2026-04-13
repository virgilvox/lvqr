# lvqr-ingest fuzz targets

libFuzzer-backed fuzz targets for the RTMP ingest parsers. Excluded from
the main workspace because `libfuzzer-sys` requires nightly rustc and
sanitizer instrumentation.

## Running

```bash
# one-shot run, any target
cargo +nightly fuzz run parse_video_tag
cargo +nightly fuzz run parse_audio_tag

# time-bounded (e.g. 60s per target in CI)
cargo +nightly fuzz run parse_video_tag -- -max_total_time=60
cargo +nightly fuzz run parse_audio_tag -- -max_total_time=60

# minimize corpus
cargo +nightly fuzz cmin parse_video_tag
```

## Corpus

Seed corpora live under `corpus/<target>/` and are not committed. Populate
them from the `lvqr-conformance` fixture set plus past crash repros:

```bash
mkdir -p corpus/parse_video_tag
cp ../../lvqr-conformance/fixtures/rtmp/*.flv-tag corpus/parse_video_tag/
```

## CI

The plan is to run each target for 60 seconds on every PR (educational
during Tier 1, gating from Tier 2) via a separate nightly GitHub Actions
job. Nightly cron runs each target for 15 minutes. Crash reproducers land
in `crates/lvqr-conformance/fixtures/edge-cases/<target>/` with a sidecar
`.toml` naming the original crash signature.
