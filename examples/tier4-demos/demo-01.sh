#!/usr/bin/env bash
#
# demo-01.sh -- Tier 4 programmable-data-plane showcase.
#
# Boots a single `lvqr serve` process with every Tier 4 surface
# enabled at once:
#
#   * WASM per-fragment filter (`--wasm-filter`) tapping every
#     fragment as it flows through the relay.
#   * Whisper live-captions agent (`--whisper-model`) producing an
#     HLS WebVTT subtitle rendition. Optional: if
#     $LVQR_WHISPER_MODEL is unset the captions wiring is skipped
#     and the rest of the demo still runs.
#   * Software ABR transcode ladder (`--transcode-rendition`) with
#     720p + 480p + 240p renditions emitted alongside the source.
#   * On-disk DVR archive (`--archive-dir`) producing a segment
#     index + finalized MP4 per track on publisher disconnect.
#
# An ffmpeg publisher feeds a synthetic colour-bars + sine test
# signal into the RTMP ingest for ~20 s. The script polls the
# admin + HLS surfaces during the publish, then asserts the
# master playlist carries all four ABR rungs and the archive
# finalized on disconnect.
#
# C2PA signing is wired today through the programmatic
# `ServeConfig.c2pa` API (see
# `crates/lvqr-archive/src/provenance.rs` for the signer-source
# enum and `crates/lvqr-cli/tests/c2pa_verify_e2e.rs` for the
# end-to-end sign + verify integration test). CLI-flag wiring for
# C2PA is on the phase-C roadmap; for now, operators who want
# signed archives call `lvqr_cli::start` programmatically with a
# `C2paConfig`. The demo prints a one-liner that exercises the
# programmatic surface via `cargo test`.
#
# Usage:
#   ./examples/tier4-demos/demo-01.sh
#
# Optional environment:
#   LVQR_WHISPER_MODEL   Absolute path to a whisper.cpp ggml model
#                        (e.g. ggml-tiny.en.bin). When unset the
#                        demo still runs but does not enable the
#                        captions agent.
#   LVQR_DEMO_SCRATCH    Override the scratch directory. Defaults
#                        to `$(mktemp -d)`; cleaned up on exit.
#   LVQR_DEMO_DURATION   Publish duration in seconds. Default 20.
#   LVQR_BIN             Override the lvqr binary path. Defaults
#                        to the first of $PWD/target/release/lvqr,
#                        $PWD/target/debug/lvqr, then `lvqr` on
#                        PATH.
#
# Prereqs (the script fails fast with a pointer to this README
# when any is missing):
#
#   * `lvqr` binary built with the `full` feature set (or at least
#     `whisper transcode`). Build via:
#       cargo build --release -p lvqr-cli --features full
#   * `ffmpeg` (for the synthetic RTMP publisher).
#   * `curl` + `jq` (for admin API probes).
#   * GStreamer 1.22+ with `base`, `good`, `bad`, `ugly`, and
#     `libav` plugin sets (used by the transcode feature at
#     runtime). The demo probes for `gst-launch-1.0`.
#
# See `examples/tier4-demos/README.md` for install recipes and
# troubleshooting.

set -euo pipefail

# -----------------------------------------------------------------
# Style: terse progress prints to stdout; detailed lvqr + ffmpeg
# logs redirected to files under the scratch dir.
# -----------------------------------------------------------------

log() { printf '[demo-01] %s\n' "$*"; }
warn() { printf '[demo-01] warn: %s\n' "$*" >&2; }
die() { printf '[demo-01] error: %s\n' "$*" >&2; exit 1; }

# -----------------------------------------------------------------
# Prereq probes.
# -----------------------------------------------------------------

need() {
  command -v "$1" >/dev/null 2>&1 \
    || die "required binary '$1' not on PATH. See examples/tier4-demos/README.md."
}

need ffmpeg
need curl
need jq
need gst-launch-1.0

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ -n "${LVQR_BIN:-}" ]]; then
  LVQR="$LVQR_BIN"
elif [[ -x "$REPO_ROOT/target/release/lvqr" ]]; then
  LVQR="$REPO_ROOT/target/release/lvqr"
elif [[ -x "$REPO_ROOT/target/debug/lvqr" ]]; then
  LVQR="$REPO_ROOT/target/debug/lvqr"
elif command -v lvqr >/dev/null 2>&1; then
  LVQR="$(command -v lvqr)"
else
  die "no lvqr binary found. Build with: cargo build --release -p lvqr-cli --features full"
fi

log "lvqr: $LVQR"

# Feature smoke: the --transcode-rendition flag is only compiled
# in when `lvqr-cli` ships with the `transcode` feature. Probe by
# asking `--help` for the flag; fail fast with a pointer if the
# binary is underfeatured.
if ! "$LVQR" serve --help 2>&1 | grep -q -- '--transcode-rendition'; then
  die "lvqr binary missing --transcode-rendition flag. Rebuild with '--features full' (or at least 'transcode')."
fi

WASM_FILTER="$REPO_ROOT/crates/lvqr-wasm/examples/frame-counter.wasm"
[[ -f "$WASM_FILTER" ]] \
  || die "WASM fixture not found at $WASM_FILTER. Run 'cargo run -p lvqr-wasm --example build_fixtures' from the repo root to regenerate it."

# -----------------------------------------------------------------
# Scratch dir + shutdown trap. Keep the archive around for the
# end-of-demo summary, then clean up on exit.
# -----------------------------------------------------------------

SCRATCH="${LVQR_DEMO_SCRATCH:-$(mktemp -d -t lvqr-demo-01.XXXXXX)}"
ARCHIVE_DIR="$SCRATCH/archive"
LVQR_LOG="$SCRATCH/lvqr.log"
FFMPEG_LOG="$SCRATCH/ffmpeg.log"

mkdir -p "$ARCHIVE_DIR"
log "scratch: $SCRATCH"

LVQR_PID=""
FFMPEG_PID=""

cleanup() {
  local rc=$?
  if [[ -n "$FFMPEG_PID" ]] && kill -0 "$FFMPEG_PID" 2>/dev/null; then
    kill -TERM "$FFMPEG_PID" 2>/dev/null || true
    wait "$FFMPEG_PID" 2>/dev/null || true
  fi
  if [[ -n "$LVQR_PID" ]] && kill -0 "$LVQR_PID" 2>/dev/null; then
    kill -TERM "$LVQR_PID" 2>/dev/null || true
    # Give lvqr 3 s to flush the archive before escalating.
    for _ in 1 2 3 4 5 6; do
      kill -0 "$LVQR_PID" 2>/dev/null || break
      sleep 0.5
    done
    if kill -0 "$LVQR_PID" 2>/dev/null; then
      kill -KILL "$LVQR_PID" 2>/dev/null || true
    fi
    wait "$LVQR_PID" 2>/dev/null || true
  fi
  if [[ -z "${LVQR_DEMO_SCRATCH:-}" ]]; then
    rm -rf "$SCRATCH"
  else
    log "scratch retained at $SCRATCH (LVQR_DEMO_SCRATCH set)"
  fi
  exit $rc
}
trap cleanup EXIT INT TERM

# -----------------------------------------------------------------
# Boot lvqr with every Tier 4 surface enabled.
# -----------------------------------------------------------------

# Non-default ports so the demo does not collide with a locally-
# running lvqr on the zero-config defaults.
ADMIN_PORT="${LVQR_DEMO_ADMIN_PORT:-18080}"
HLS_PORT="${LVQR_DEMO_HLS_PORT:-18888}"
RTMP_PORT="${LVQR_DEMO_RTMP_PORT:-11935}"
MOQ_PORT="${LVQR_DEMO_MOQ_PORT:-14443}"

WHISPER_ARGS=()
if [[ -n "${LVQR_WHISPER_MODEL:-}" ]]; then
  if [[ ! -f "$LVQR_WHISPER_MODEL" ]]; then
    die "LVQR_WHISPER_MODEL points at '$LVQR_WHISPER_MODEL' but the file does not exist. See examples/tier4-demos/README.md for download instructions."
  fi
  WHISPER_ARGS=(--whisper-model "$LVQR_WHISPER_MODEL")
  log "captions: enabled (model=$LVQR_WHISPER_MODEL)"
else
  log "captions: skipped (set LVQR_WHISPER_MODEL to enable)"
fi

log "boot: lvqr serve (admin=$ADMIN_PORT hls=$HLS_PORT rtmp=$RTMP_PORT moq=$MOQ_PORT)"

"$LVQR" serve \
  --port "$MOQ_PORT" \
  --admin-port "$ADMIN_PORT" \
  --hls-port "$HLS_PORT" \
  --rtmp-port "$RTMP_PORT" \
  --archive-dir "$ARCHIVE_DIR" \
  --wasm-filter "$WASM_FILTER" \
  --transcode-rendition 720p \
  --transcode-rendition 480p \
  --transcode-rendition 240p \
  "${WHISPER_ARGS[@]}" \
  >"$LVQR_LOG" 2>&1 &
LVQR_PID=$!

# -----------------------------------------------------------------
# Wait for /healthz. lvqr does not expose a readiness surface
# distinct from liveness today, so /healthz is the best-available
# wait target; bootstrapping typically takes under a second.
# -----------------------------------------------------------------

log "wait: lvqr /healthz (budget 15 s)"
health_deadline=$(( $(date +%s) + 15 ))
until curl -fsS "http://127.0.0.1:$ADMIN_PORT/healthz" >/dev/null 2>&1; do
  if ! kill -0 "$LVQR_PID" 2>/dev/null; then
    warn "lvqr exited during startup; tail of $LVQR_LOG:"
    tail -n 40 "$LVQR_LOG" >&2 || true
    die "lvqr did not come up"
  fi
  if (( $(date +%s) > health_deadline )); then
    die "timed out waiting for /healthz; see $LVQR_LOG"
  fi
  sleep 0.25
done

log "up: lvqr ready"

# -----------------------------------------------------------------
# Publish synthetic colour-bars + sine for LVQR_DEMO_DURATION
# seconds via ffmpeg. -re paces real-time so the transcode ladder
# sees arrivals at the rate a real publisher would produce them.
# -----------------------------------------------------------------

DURATION="${LVQR_DEMO_DURATION:-20}"
BROADCAST="live/demo"
log "publish: ffmpeg -> rtmp://127.0.0.1:$RTMP_PORT/${BROADCAST} ($DURATION s)"

ffmpeg -hide_banner -loglevel warning \
  -re \
  -f lavfi -i "testsrc=size=640x360:rate=30" \
  -f lavfi -i "sine=frequency=440:sample_rate=44100" \
  -t "$DURATION" \
  -c:v libx264 -preset ultrafast -tune zerolatency -pix_fmt yuv420p -g 60 \
  -c:a aac -b:a 128k -ar 44100 -ac 2 \
  -f flv "rtmp://127.0.0.1:$RTMP_PORT/${BROADCAST}" \
  >"$FFMPEG_LOG" 2>&1 &
FFMPEG_PID=$!

# -----------------------------------------------------------------
# While ffmpeg runs, poll the master playlist until it advertises
# all four ABR rungs (source + 720p + 480p + 240p). The transcode
# ladder registers its sibling broadcasts only after GStreamer
# produces output fragments, so the rungs trickle in a few
# seconds behind the first keyframe.
# -----------------------------------------------------------------

MASTER_URL="http://127.0.0.1:$HLS_PORT/hls/${BROADCAST}/master.m3u8"
log "poll: $MASTER_URL for 4 variants (budget ${DURATION}s)"

master_deadline=$(( $(date +%s) + DURATION ))
master_body=""
variant_count=0
until (( variant_count >= 4 )); do
  if (( $(date +%s) > master_deadline )); then
    warn "master playlist never reached 4 variants within ${DURATION}s"
    break
  fi
  body="$(curl -fsS "$MASTER_URL" 2>/dev/null || true)"
  if [[ -n "$body" ]]; then
    variant_count="$(printf '%s\n' "$body" | grep -c '^#EXT-X-STREAM-INF' || true)"
    master_body="$body"
  fi
  sleep 0.5
done

log "master: $variant_count variant(s) advertised"

# -----------------------------------------------------------------
# Let ffmpeg finish its -t window so the publisher disconnects
# cleanly; that disconnect is the signal that triggers the
# archive finalize path for each track.
# -----------------------------------------------------------------

log "wait: ffmpeg publish window"
wait "$FFMPEG_PID" || warn "ffmpeg exited non-zero; see $FFMPEG_LOG"
FFMPEG_PID=""

# -----------------------------------------------------------------
# The archive indexer's drain task only runs `finalize` once every
# producer-side clone of the broadcaster has dropped. That takes a
# short tick after the RTMP connection closes. Poll for the
# per-track output directory to appear.
# -----------------------------------------------------------------

ARCHIVE_BROADCAST_DIR="$ARCHIVE_DIR/$BROADCAST"
log "wait: archive under $ARCHIVE_BROADCAST_DIR (budget 10 s)"

archive_deadline=$(( $(date +%s) + 10 ))
while :; do
  if [[ -d "$ARCHIVE_BROADCAST_DIR/0.mp4" ]]; then
    break
  fi
  if (( $(date +%s) > archive_deadline )); then
    warn "archive video track did not appear at $ARCHIVE_BROADCAST_DIR/0.mp4"
    break
  fi
  sleep 0.25
done

# -----------------------------------------------------------------
# Metrics probe: the WASM filter tap increments a per-outcome
# counter on every observed fragment, so a non-zero keep count
# proves the tap actually ran.
# -----------------------------------------------------------------

METRICS_URL="http://127.0.0.1:$ADMIN_PORT/metrics"
wasm_keep="$(curl -fsS "$METRICS_URL" 2>/dev/null \
  | awk '/^lvqr_wasm_fragments_total\{.*outcome="keep"/ { gsub(/[^0-9]/,"",$NF); print $NF; exit }')"
wasm_keep="${wasm_keep:-0}"
log "metrics: lvqr_wasm_fragments_total{outcome=keep} = $wasm_keep"

# -----------------------------------------------------------------
# Streams surface snapshot. The publisher is gone by now; this
# just proves the admin router answered the whole time.
# -----------------------------------------------------------------

streams_body="$(curl -fsS "http://127.0.0.1:$ADMIN_PORT/api/v1/streams" 2>/dev/null || true)"
if [[ -n "$streams_body" ]]; then
  stream_count="$(printf '%s\n' "$streams_body" | jq 'length' 2>/dev/null || echo 0)"
else
  stream_count=0
fi

# -----------------------------------------------------------------
# Captions playlist probe (only when whisper was enabled). The
# rendition exists as soon as the captions factory installs its
# bridge on the registry, so it returns 200 even before the agent
# produces a cue.
# -----------------------------------------------------------------

caption_status="skipped"
if [[ ${#WHISPER_ARGS[@]} -gt 0 ]]; then
  caption_url="http://127.0.0.1:$HLS_PORT/hls/${BROADCAST}/captions/playlist.m3u8"
  if curl -fsS -o /dev/null -w '%{http_code}' "$caption_url" 2>/dev/null | grep -q '^200$'; then
    caption_status="playlist: 200 at $caption_url"
  else
    caption_status="playlist: not available at $caption_url (agent may still be warming)"
  fi
fi

# -----------------------------------------------------------------
# Summary. Keep it deliberately flat so the output parses at a
# glance in CI logs.
# -----------------------------------------------------------------

echo
echo "================ demo-01 summary ================"
echo "  broadcast          : $BROADCAST"
echo "  hls master         : $MASTER_URL"
echo "  hls variants       : $variant_count advertised"
echo "  wasm tap keep      : $wasm_keep fragment(s)"
echo "  archive dir        : $ARCHIVE_BROADCAST_DIR"
echo "  archive video      : $(ls -1 "$ARCHIVE_BROADCAST_DIR/0.mp4" 2>/dev/null | wc -l | tr -d ' ') file(s)"
echo "  /api/v1/streams    : $stream_count entry(ies)"
echo "  captions           : $caption_status"
echo "  c2pa sign+verify   : not wired on the CLI today -- run"
echo "                       'cargo test -p lvqr-cli --features c2pa \\"
echo "                          --test c2pa_verify_e2e' for the"
echo "                       programmatic end-to-end fixture."
echo "=================================================="
echo

# -----------------------------------------------------------------
# Non-zero exit if the primary assertions failed.
# -----------------------------------------------------------------

if (( variant_count < 4 )); then
  die "ABR ladder did not advertise 4 variants (got $variant_count); see $LVQR_LOG"
fi
if [[ ! -d "$ARCHIVE_BROADCAST_DIR/0.mp4" ]]; then
  die "archive video track did not materialize under $ARCHIVE_BROADCAST_DIR"
fi
if (( wasm_keep == 0 )); then
  warn "WASM tap reported zero keeps; the filter may not have registered"
fi

log "done: demo-01 succeeded"
