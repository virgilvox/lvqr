# LVQR Handoff Document

## Project Status: v0.4.0 -- RTSP PLAY egress for H.264 + HEVC + AAC + Opus with per-drain RTCP SR; 614 tests, all green

**Last Updated**: 2026-04-16 (session 64 close).

## Session 64 close (2026-04-16)

### What shipped (2 commits, +1490 / -182 lines, net +1308)

1. **Opus RTSP PLAY egress** (`543e002`). Extended the AAC PLAY
   scaffold to Opus:
   * `lvqr_cmaf::OpusConfig` + `extract_opus_config` parse the
     dOps box from an Opus sample entry. Returns channels,
     pre_skip, input_sample_rate, output_gain, and the serialized
     11-byte dOps body (so out-of-band signaling can echo the
     raw bytes).
   * `sdp::OpusTrackDescription` + new `AudioTrackDescription`
     enum render RFC 7587 audio (`opus/48000/<ch>`, `sprop-
     stereo`, `useinbandfec=1`) at PT 98 distinct from AAC's
     PT 97. `PlaySdp.audio` switched from `Option<AacTrackDescription>`
     to the enum so only one audio codec is describable per
     broadcast.
   * `rtp::OpusPacketizer` + `OpusDepacketizer`: one Opus frame
     per RTP packet with marker=1 per RFC 7587. Depacketizer is
     byte-transparent.
   * `play::play_drain_opus` mirrors `play_drain_aac`. No
     parameter-set re-injection (Opus decoder state comes from
     the dOps on SDP).
   * `handle_describe` tries Opus first then falls through to
     AAC. `handle_play` dispatches the audio drain variant the
     same way.
   * Integration test `rtsp_play_opus_audio_track_delivers_frame`
     round-trips a synthetic Opus frame over real TCP through
     DESCRIBE / SETUP / PLAY and `OpusDepacketizer`.
   * +13 tests.

2. **RTCP Sender Reports on every PLAY drain** (`0bfe7d7`).
   Per-SSRC SR generation so long sessions stay NTP-wallclock
   aligned:
   * New `crate::rtcp` module with `RtpStats` (lock-free
     packet/octet/last-RTP-ts counters), `write_sender_report`
     (RFC 3550 section 6.4.1, 28-byte SR, no reception reports),
     `system_time_to_ntp` (RFC 5905 seconds-since-1900), and
     `spawn_sr_task` (per-drain ticker that snapshots stats and
     pushes interleaved RTCP on the odd channel).
   * `play::PlayDrainCtx` groups `broadcast + rtp_channel +
     rtcp_channel + sr_interval` so the four drain signatures
     stay short. `DEFAULT_SR_INTERVAL = 5 s`.
   * Each drain (H.264 / HEVC / AAC / Opus) now owns an
     `Arc<RtpStats>` shared with a spawned SR task, routes every
     RTP send through `send_rtp()` so counters tick in lock step
     with the wire output, and awaits the SR task on termination
     so the task never outlives its session. Per-codec SSRCs
     (AAC_SSRC, OPUS_SSRC) keep Wireshark traces readable.
   * `handle_play` preserves both interleaved channels off the
     session transport and feeds them into each drain's
     `PlayDrainCtx`.
   * Integration test `rtsp_play_emits_rtcp_sender_report_after_interval`
     uses `start_paused = true` + `tokio::time::advance(6s)` to
     collapse the 5 s wait, drives the H.264 handshake end-to-
     end, emits one IDR, and asserts an SR arrives on channel 1
     carrying the latest RTP timestamp (9000) with packet_count
     >= 3 (SPS + PPS + IDR) and octet_count > 0.
   * +11 tests.

### Ground truth (session 64 close)

* **Head**: `0bfe7d7` on `main`. v0.4.0. **14 commits queued but
  NOT pushed to origin/main** (sessions 62-64 all unpushed).
* **Tests**: 614 passed, 0 failed, 1 ignored. Delta from session
  63: +24 (13 Opus + 11 RTCP).
* **Code**: +1490 / -182 net lines across the 2 commits.
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### RTSP PLAY status (session 64 end)

| Piece                                             | Status |
|---------------------------------------------------|--------|
| H.264 PLAY drain + SDP + re-injection             | DONE (session 62) |
| HEVC PLAY drain + RFC 7798 SDP + VPS/SPS/PPS      | DONE (session 63) |
| AAC PLAY drain + RFC 3640 SDP + config=hex        | DONE (session 63) |
| Opus PLAY drain + RFC 7587 SDP + dOps             | DONE   |
| RTCP SR generation (H.264 / HEVC / AAC / Opus)    | DONE   |
| fMP4 mdat extractor + AVCC NAL splitter           | DONE   |
| Init-segment extraction (AVC/HEVC/AAC/Opus)       | DONE   |
| DESCRIBE SDP from broadcaster meta                | DONE   |
| End-to-end integration (5 codecs + SR)            | DONE   |
| mediastreamvalidator in CI                        | pending |

RTSP PLAY egress is feature-complete for every codec LVQR carries
at ingest time (H.264, HEVC, AAC, Opus) and every long-session
stream now emits paired SRs on the standard 0/1 and 2/3 interleaved
channels. No audio codec is missing; no video codec is missing;
long sessions stay NTP-aligned for DVR scrub.

### Load-bearing invariants (all four still pinned)

Unchanged from session 60. Each PLAY drain owns only a
`BroadcasterStream` receiver (no strong `Arc<FragmentBroadcaster>`),
and the new SR task co-owned by each drain also avoids any
broadcaster reference -- it only sees `Arc<RtpStats>`, a writer
mpsc sender, and the shared cancellation token. Four drains,
four SR tasks, one shared invariant.

### Protocols supported

11 protocols. Feature-complete for every codec: RTMP / WHIP /
SRT / RTSP ingest; LL-HLS + DASH + WHEP + MoQ + WebSocket + **RTSP
PLAY (H.264 / HEVC / AAC / Opus, RTCP SR on all four)** egress.

### Known gaps

1. **mediastreamvalidator binary on CI runner**: the biggest
   remaining audit gap. `hls-conformance.yml` runs
   `continue-on-error: false` on every PR, but `macos-latest`
   ships without Apple HTTP Live Streaming Tools, so the
   validator step soft-skips and the effective gate is the
   ffmpeg-client-pull fallback. Promotion requires a
   self-hosted macOS runner or a pre-baked custom image.
2. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
3. **Tier 3**: cluster (chitchat) + observability (OTLP) not
   started.
4. **Tiers 4-5**: not started.

### Session 65 entry point

Priority order:

1. **mediastreamvalidator self-hosted runner** (infra;
   user-bound). Apple HTTP Live Streaming Tools on a
   self-hosted macOS runner (or pre-baked custom image) so
   `hls-conformance.yml`'s validator step runs authoritatively
   instead of soft-skipping. Removes the last asterisk on the
   "spec-compliant LL-HLS" claim. Schedule when the user has
   capacity for the infra work.

2. **Tier 3 planning**. Cluster (chitchat) + observability (OTLP).
   Bigger scope; do not start mid-session.

3. **24 h soak harness**. Tier 1 gap that informs the M4
   "LiveKit alternative for new projects" readiness date. Run a
   synthetic publisher + 10 concurrent PLAY subscribers over
   24 h and track fragment loss, drain CPU, SR drift. No code
   changes; purely harness + CI wiring.

4. **MediaMTX comparison harness**. Head-to-head latency +
   memory footprint benchmark so the roadmap ETAs have real
   numbers behind them.

5. **Tier 2.4 WASM client** (research). Defer until M4 is within
   reach.

### Velocity note

Session 64 landed Opus PLAY + RTCP SR in two commits (one per
feature); +24 tests; 614 total across 23 crates. Observed pace
stays at ~10-15 sessions per calendar week at ~2-4 commits and
~300-1500 lines each. RTSP PLAY egress work is now closed out;
the remaining Tier 2 surface is infra (#1) + observability (#2)
rather than protocol code. Realistic M4 ("LiveKit alternative
for new projects") ETA remains 3-5 calendar weeks of sustained
sessions, bounded by 24 h soak, multi-node cluster debugging,
and WASM / AI research components.

## Session 63 close (2026-04-16)

### What shipped (3 commits, +1321 / -43 lines, net +1278)

1. **HEVC PLAY egress** (`6937b03`). Extended the H.264 PLAY
   scaffold to HEVC:
   * `lvqr_cmaf::HevcParameterSets` gains profile/tier/level
     fields pulled off the hvcC box for RFC 7798 SDP.
   * `sdp::PlaySdp.video` becomes `VideoTrackDescription::{H264,
     Hevc}`; HEVC renders `m=video` + `rtpmap H265/90000` + RFC
     7798 fmtp with profile-space/id, tier-flag, level-id,
     profile-compatibility-indicator, interop-constraints, and
     separate base64 `sprop-vps` / `sprop-sps` / `sprop-pps`.
   * `play::play_drain_hevc` composes `HevcPacketizer` with
     `extract_hevc_parameter_sets` and the existing `fmp4`
     demux. Re-injects VPS + SPS + PPS before the first IDR
     (three packets vs H.264's two).
   * `handle_describe` prefers HEVC, falls back to AVC.
     `handle_play` picks the drain variant by testing
     `extract_hevc_parameter_sets` on the init bytes.
   * 5 new tests.

2. **AAC PLAY egress** (`618a709`). Parallel audio path:
   * `lvqr_cmaf::AacConfig` + `extract_aac_config`: decode
     ftyp+moov, find the first mp4a entry, reconstruct the
     2-byte AudioSpecificConfig from the mp4-atom
     `DecoderSpecific` (profile / freq_index / chan_conf).
     Sample rate resolved from the standard freq_index table.
   * `sdp::AacTrackDescription` renders RFC 3640 AAC-hbr:
     `m=audio` + `mpeg4-generic/<rate>/<channels>` +
     `streamtype=5;profile-level-id=1;mode=AAC-hbr;sizelength=13;
     indexlength=3;indexdeltalength=3;config=<hex>`.
   * `play::play_drain_aac`: AacPacketizer + the existing
     `fmp4::extract_mdat_body`. No parameter-set re-injection
     (AAC decoder config lives only in SDP). One RTP packet
     per access unit, marker=1 per RFC 3640, PT=97 distinct
     from video's PT=96.
   * `handle_play` now spawns video + audio drains in parallel
     from `session.transports["track1"]` and
     `session.transports["track2"]` so a client SETUPping
     both tracks gets both drains.
   * 8 new tests (4 cmaf, 2 sdp, 2 play).

3. **HEVC + AAC PLAY end-to-end integration** (`b9c7563`).
   Two new tests in `tests/play_integration.rs` beside the
   session-62 H.264 test. Real TCP client runs DESCRIBE ->
   SETUP -> PLAY against a bare RtspServer, emits a fragment
   through the broadcaster, and verifies round-trip through
   the depacketizer.

### Ground truth (session 63 close)

* **Head**: `b9c7563` on `main`. v0.4.0.
* **Tests**: 590 passed, 0 failed, 1 ignored. Delta from
  session 62: +15 (5 HEVC + 8 AAC + 2 integration).
* **Code**: +1321 / -43 net lines.
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### RTSP PLAY status (session 63 end)

| Piece                                             | Status |
|---------------------------------------------------|--------|
| H.264 PLAY drain + SDP + re-injection             | DONE (session 62) |
| HEVC PLAY drain + RFC 7798 SDP + VPS/SPS/PPS      | DONE   |
| AAC PLAY drain + RFC 3640 SDP + config=hex        | DONE   |
| fMP4 mdat extractor + AVCC NAL splitter           | DONE   |
| Init-segment param-set extraction (AVC/HEVC/AAC)  | DONE   |
| DESCRIBE SDP from broadcaster meta                | DONE   |
| End-to-end integration tests (H.264 + HEVC + AAC) | DONE   |
| RTCP SR generation                                | pending |
| Opus audio PLAY drain                             | pending |
| mediastreamvalidator in CI                        | pending |

All three codecs LVQR ships at ingest time now have a working
PLAY egress. RTCP and Opus remain but are incremental.

### Load-bearing invariants (all four still pinned)

Unchanged from session 60. Each PLAY drain holds only a
`BroadcasterStream` receiver, never a strong
`Arc<FragmentBroadcaster>`. The three drain functions have
parallel structure so future refactors can extract a common
skeleton without regressing the invariant.

### Protocols supported

11 protocols, now with fuller PLAY coverage: RTMP + WHIP + SRT +
RTSP ingest; LL-HLS + DASH + WHEP + MoQ + WebSocket + **RTSP PLAY
(H.264 / HEVC / AAC)** egress.

### Known gaps

1. **Opus PLAY drain**: WebRTC-sourced Opus currently produces an
   audio broadcaster that `extract_aac_config` rejects. The SDP
   builder needs a parallel `OpusTrackDescription` and
   `play_drain_opus`. RFC 7587 packetization is
   single-frame-per-packet and fits cleanly alongside the AAC
   scaffold.
2. **RTCP SR on PLAY**: no sender reports today. Most clients
   tolerate absence for short sessions; a long-running VLC /
   ffplay will eventually query NTP-wallclock alignment and
   silence.
3. **mediastreamvalidator binary on CI runner**: the
   `hls-conformance.yml` workflow already exists and runs on
   every PR with `continue-on-error: false`, promoted in
   session 33. But `macos-latest` ships without Apple's HTTP
   Live Streaming Tools, so the job currently soft-skips the
   validator step and the real effective gate is the
   ffmpeg-client-pull fallback (4xx detection on the pulled
   playlist + 5 s of real decode). Promotion to a
   validator-backed gate requires a self-hosted macOS runner or
   pre-baked Apple tools in a custom image.
4. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
5. **Tiers 3-5**: not started.

### Session 64 entry point

Priority order:

1. **Opus PLAY drain + SDP**. Extend the AAC scaffold:
   `OpusTrackDescription` next to `AacTrackDescription`, SDP
   block `m=audio 0 RTP/AVP <pt>` + `a=rtpmap:<pt>
   opus/48000/2` + `a=fmtp:<pt> sprop-stereo=1;useinbandfec=1`.
   `play_drain_opus` uses a new `OpusPacketizer` (RFC 7587:
   one Opus frame per RTP packet, no framing). `handle_describe`
   detects Opus by trying `extract_opus_config` first (to be
   added in `lvqr-cmaf`, parses the `dOps` box) and falls
   through to AAC. `handle_play` picks the drain variant by
   inspecting the audio broadcaster's init segment. Integration
   test against a synthetic Opus init + fragment.

2. **RTCP SR generation**. Per-SSRC Sender Reports on a short
   timer (say every 5 s). Needed for long-running sessions and
   for Wireshark sanity during external interop testing. Write
   from the same mpsc writer channel the PLAY drains use, on
   the odd interleaved channel paired with each RTP channel
   (channel 1 for video, 3 for audio under the standard
   0-1 / 2-3 pairing).

3. **mediastreamvalidator self-hosted runner**. Stand up a
   self-hosted macOS runner (or bake a custom image) carrying
   Apple HTTP Live Streaming Tools so `hls-conformance.yml`'s
   validator step runs authoritatively instead of soft-skipping.
   Removes the last asterisk on the "spec-compliant LL-HLS"
   claim. Schedule when the user has capacity for the infra
   work; incremental code changes (priorities 1 + 2) are more
   tractable inside a single Claude Code session.

4. **lvqr-cmaf with mp4-atom**. Only becomes urgent when AV1
   or timed text land in the data path.

5. **Tier 3 planning**. Cluster (chitchat) + observability (OTLP).

## Session 62 close (2026-04-16)

### What shipped (6 commits, +1770 / -37 lines, net +1733)

1. **Test doc cleanup** (`344fce6`). Audit pass caught stale
   references to the deleted `HlsFragmentBridge` /
   `DashFragmentBridge` in the cli integration-test module
   comments. Refreshed to describe the broadcaster-native
   pipeline session 60 landed.

2. **fMP4 demux helpers** (`0369120`). New
   `lvqr_rtsp::fmp4` module with `extract_mdat_body` and
   `split_avcc_nalus`. 15 tests including round-trips against
   the live output of `lvqr_cmaf::build_moof_mdat`.

3. **Init-segment parameter-set extractors** (`5f7e01f`).
   `lvqr_cmaf::{AvcParameterSets, HevcParameterSets,
   extract_avc_parameter_sets, extract_hevc_parameter_sets}`.
   Reuses the existing `mp4-atom` decode path so `lvqr-rtsp`
   consumes the ergonomic `Vec<Vec<u8>>` interface without
   pulling `mp4-atom` into its own dep tree. 5 tests: AVC +
   HEVC round-trip, cross-codec None, empty-input None.

4. **DESCRIBE SDP from broadcaster meta** (`7bb83f2`). New
   `lvqr_rtsp::sdp` module with `PlaySdp` + `H264TrackDescription`
   and a `render()` that emits RFC 6184-compliant SDP with
   `profile-level-id`, `packetization-mode=1`,
   `sprop-parameter-sets`. `handle_describe` now reads the init
   bytes off the shared registry and renders a real video `m=`
   block when the broadcaster exists; a DESCRIBE before any
   publisher returns a session-only SDP (no `m=` line) rather
   than a 404. 7 SDP tests + 2 updated server tests.

5. **PLAY drain wiring** (`308de54`). New `lvqr_rtsp::play`
   module with `play_drain_h264` composing session-61 RTP
   packetizers with session-62 mdat + param-set extractors. The
   drain subscribes to the broadcaster, re-injects SPS + PPS as
   single-NAL RTP packets before the first IDR, then loops:
   extract mdat -> split AVCC -> packetize (marker on the last
   NAL of each access unit) -> wrap in interleaved frame ->
   send via writer channel. `handle_connection` refactored to an
   mpsc-driven write model so responses and drain frames both
   flow through a single writer with natural TCP back-pressure.
   `handle_play` spawns the drain with the session's negotiated
   interleaved channel. Drain terminates on per-connection
   cancel, broadcaster close, or writer-channel close. Holds
   only a `BroadcasterStream` receiver -- no strong
   `Arc<FragmentBroadcaster>`, matching the invariant the
   archive / HLS / DASH drains already document.

6. **End-to-end PLAY integration test** (`4f63186`). Real TCP
   client drives an `RtspServer` backed by a shared registry
   through OPTIONS -> DESCRIBE -> SETUP -> PLAY, then emits
   one IDR fragment through the broadcaster and verifies:
   SDP carries H.264 `m=` block with `sprop-parameter-sets`,
   SETUP accepts `interleaved=0-1`, PLAY spawns the drain,
   two RTP packets carry the re-injected SPS + PPS on
   channel 0, the IDR packet depacks through
   `H264Depacketizer` with keyframe=true and byte-identical
   NAL bytes.

### Ground truth (session 62 close)

* **Head**: `4f63186` on `main` (unpushed; sessions 60-61
  pushed earlier in the cycle). v0.4.0.
* **Tests**: 575 passed, 0 failed, 1 ignored. Delta from
  session 61: +31 (15 fmp4 + 5 cmaf extractors + 7 SDP +
  2 play module + 2 server describe + -1 net server test
  rename + 1 integration).
* **Code**: +1770 / -37 net lines across the 6 commits.
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### RTSP PLAY status (session 62 end)

| Piece                                             | Status |
|---------------------------------------------------|--------|
| RTP packetizers (H.264 / HEVC / AAC)              | DONE   |
| fMP4 mdat extractor + AVCC NAL splitter           | DONE   |
| SPS/PPS/VPS extraction from init                  | DONE   |
| DESCRIBE SDP from broadcaster meta (H.264)        | DONE   |
| PLAY drain: subscribe + packetize + interleaved   | DONE   |
| Parameter-set re-injection before first IDR       | DONE   |
| TEARDOWN cancel propagation to drain              | DONE   |
| End-to-end integration test (real TCP client)     | DONE   |
| HEVC PLAY drain + SDP                             | pending |
| Audio (AAC / Opus) PLAY drain + SDP               | pending |
| RTCP SR generation                                | pending |

First-pass PLAY is done for H.264 video. HEVC + audio layer
on top of the same `play_drain_h264` skeleton once they're
prioritized.

### Load-bearing invariants (all four still pinned)

Unchanged from session 60. The session-62 `play_drain_h264` is
built with the same invariant as every prior drain: it owns
only a `BroadcasterStream` receiver, never a strong
`Arc<FragmentBroadcaster>`. Documented in `play.rs` source
comment so a future refactor does not regress.

### Protocols supported

Now 11 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket + **RTSP PLAY (H.264)**
egress.

### Known gaps

1. **HEVC + audio PLAY**: first-pass is H.264 video only.
2. **RTCP**: no SR / RR generation on the PLAY direction.
   Most clients tolerate absence for short sessions.
3. **Apple mediastreamvalidator in CI**: biggest audit gap.
4. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
5. **Tiers 3-5**: not started.

### Session 63 entry point

Priority order:

1. **Apple mediastreamvalidator in CI**. GitHub Actions job
   that runs Apple's validator against `lvqr-hls`-generated
   playlists and blocks merges on validator-red.

2. **HEVC + audio PLAY drain**. Extend `play_drain_h264` into a
   codec-aware drain (or sibling `play_drain_hevc` /
   `play_drain_aac`) using the HEVC + AAC packetizers session
   61 already landed. DESCRIBE needs to emit matching SDP
   blocks (hvcC -> `sprop-vps/sps/pps`; esds -> `config`).

3. **lvqr-cmaf with mp4-atom**. Swap the hand-rolled fMP4
   writer for mp4-atom once AV1 + timed text becomes load-
   bearing.

4. **Tier 3 planning**. Cluster (chitchat) + observability
   (OTLP).

## Session 61 close (2026-04-16)

### What shipped (1 commit, +563 / -4 lines)

1. **RTP packetizers: H.264 + HEVC + AAC** (`d156827`). Producer
   side of the RTP wire format. `rtp.rs` already carried the
   depacketizers the ingest / RECORD path uses since session 44;
   this commit fills in the inverse for the PLAY direction.

   * `H264Packetizer` per RFC 6184: single-NAL-unit when the NAL
     fits within `mtu`, FU-A fragmentation (NAL type 28) otherwise.
     NRI bits carried through the FU indicator; marker bit settable
     per-call on the last NAL of an access unit.
   * `HevcPacketizer` per RFC 7798: single NAL when it fits, FU
     (type 49) otherwise. PayloadHdr preserves the original
     `layer_id` + `tid` and replaces only the NAL type.
   * `AacPacketizer` per RFC 3640 AAC-hbr: one AU per packet
     (13-bit AU-size + 3-bit AU-Index=0). Multi-AU + AU
     fragmentation deferred until real-world jitter / MTU data
     demands it.

   Neither H.264 nor HEVC packetizer emits aggregation packets
   (STAP-A / AP). LVQR carries parameter sets on the fMP4 init
   segment and strips them from the fragment payload; the eventual
   PLAY wiring must re-inject SPS/PPS/VPS from the init segment
   before the first keyframe.

   16 new tests, every packetizer round-trips through its matching
   depacketizer so the RFC wire format is pinned by equivalence
   rather than re-encoded prose. Covers single-NAL, FU fragmentation
   across multiple packets (marker placement + sequence continuity),
   non-keyframes, MTU validation panics, sequence wrap, multi-NAL
   access unit marker placement, and explicit on-wire AAC layout.

### Ground truth (session 61 close)

* **Head**: `d156827` on `main`. v0.4.0.
* **Tests**: 544 passed, 0 failed, 1 ignored. Delta from session
  60: +16 (the packetizer unit tests).
* **Code**: +563 / -4. Net +559 lines in `rtp.rs` (all additions
  past the existing depacketizer section, including the 16 tests).
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### Scope note: why just the packetizers this session

RTSP playback egress is estimated at 5-8 sessions in the handoff
ROI table; session 60 identified three discrete pieces of work:

1. **RTP packetizers** (this session).
2. **Fragment → NAL demux**. Parse `mdat` body out of the fMP4
   `moof+mdat` fragment payload so the PLAY drain task can feed
   raw NAL units / AAC access units into the packetizers. AVCC
   length-prefixed bytes for video; raw AAC for audio.
3. **PLAY path wiring**. Spawn a per-session drain task that
   subscribes to the shared registry broadcaster, demuxes `mdat`
   payloads, packetizes, and writes interleaved RTP frames onto
   the TCP socket. Re-inject SPS / PPS / VPS from the init segment
   before the first keyframe. Also needs DESCRIBE's SDP to carry
   the negotiated codec params.

Piece 1 is bounded and tightly testable in isolation. Pieces 2 and
3 are interlocked, so bundling them into one commit is cleaner than
splitting; they land in session 62.

### Load-bearing invariants (all four still pinned)

Unchanged from session 60. The packetizers are pure producer
functions; they don't touch the broadcaster primitives.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **RTSP PLAY wiring**: packetizers exist now, but no task
   subscribes to the registry and emits RTP. Session 62.
2. **Apple mediastreamvalidator in CI**: biggest audit gap.
3. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
4. **Tiers 3-5**: not started.

### Session 62 entry point

Priority order:

1. **RTSP PLAY path wiring**. Build the fMP4 `mdat` extractor
   (pure fn, unit-testable: parse box header chain, find the
   `mdat` box, return its body slice). Wire `handle_play` to
   spawn a drain task per session that subscribes to the shared
   registry, extracts NAL / AAC bytes, runs the packetizer,
   and writes interleaved frames to the TCP socket. Re-inject
   parameter sets from the init segment before the first RTP
   packet of each video stream. Update DESCRIBE's SDP to carry
   real `sprop-parameter-sets` / `config` derived from the
   broadcaster meta. Add an end-to-end integration test:
   RTMP publish in, RTSP DESCRIBE + SETUP + PLAY out, verify
   RTP frames arrive at the client and depack cleanly.

2. **Apple mediastreamvalidator in CI**. GitHub Actions job
   that runs Apple's validator against `lvqr-hls`-generated
   playlists and blocks merges on validator-red.

3. **lvqr-cmaf with mp4-atom**. Swap the hand-rolled fMP4
   writer for mp4-atom once AV1 + timed text becomes load-
   bearing.

4. **Tier 3 planning**.

## Session 60 close (2026-04-16)

### What shipped (3 commits, +716 / -1211 lines, net -495)

1. **LL-HLS bridge switched to broadcaster-native** (`42a0c01`).
   `BroadcasterHlsBridge` replaces `HlsFragmentBridge`. An
   `on_entry_created` callback on the shared
   `FragmentBroadcasterRegistry` spawns one tokio drain task per
   `(broadcast, track)`; the task refreshes broadcaster meta each
   iteration to catch init-segment republishes (reconnect, codec
   change) and resets `CmafPolicyState` accordingly. The on-wire
   LL-HLS surface is byte-identical; rtmp_hls_e2e + rtsp_hls_e2e
   + srt_hls_e2e all pass unchanged against the new path.

2. **DASH bridge switched to broadcaster-native** (`e8a063b`).
   Same pattern. `BroadcasterDashBridge` subscribes via
   `on_entry_created` and stamps a monotonic `$Number$` counter
   onto every fragment; an init-change resets the counter so
   `SegmentTemplate` resolution restarts at 1 after a reconnect.
   Drops the lvqr-ingest dependency from lvqr-dash now that the
   crate no longer implements `FragmentObserver`. rtmp_dash_e2e
   passes unchanged.

3. **FragmentObserver trait + observer-side dispatch deleted**
   (`a5b9316`). Every Tier 2.1 consumer is now broadcaster-native,
   so the observer surface is dead code. Deletions:
   * `FragmentObserver`, `SharedFragmentObserver`,
     `NoopFragmentObserver` from `lvqr-ingest::observer`. Kept
     `RawSampleObserver` + `MediaCodec` (WHEP RTP packetizer tap).
   * `Option<&SharedFragmentObserver>` parameter dropped from
     `publish_init` / `publish_fragment`.
   * `with_observer` / `set_observer` builders from
     `RtmpMoqBridge` and `WhipMoqBridge`. `observer` parameter
     dropped from `SrtIngestServer::run` and `RtspServer::run`.
   * `TeeFragmentObserver` from `lvqr-cli::archive`.
   * `fragment_observers` Vec + `shared_fragment_observer` +
     `with_observer` calls from `lvqr-cli::start`.
   * Unit tests that exercised the observer trait directly
     (WHIP, SRT, RTSP, dispatch, DASH) rewritten against the
     broadcaster-registry surface.

### Ground truth (session 60 close)

* **Head**: `a5b9316` on `main`. v0.4.0.
* **Tests**: 528 passed, 0 failed, 1 ignored. Delta from session
  59: -1 (one RTSP integration test consolidated with the
  migrated full-ingest test since both now exercise the same
  broadcaster path after the dual-wire surface was removed).
* **Code**: +716 / -1211 net lines across the three commits. Net
  -495 lines: deleting the observer transitively saved more than
  the broadcaster drain tasks added.
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### Consumer-side migration status

| Consumer          | Status             | Session |
|-------------------|--------------------|---------|
| Archive indexer   | broadcaster-native | 59      |
| LL-HLS bridge     | broadcaster-native | 60      |
| DASH bridge       | broadcaster-native | 60      |
| FragmentObserver  | DELETED            | 60      |

### Load-bearing invariants (unchanged, all four still pinned)

1. FragmentBroadcaster sender lives outside the `Arc<Shared>`
   subscribers hold.
2. Registry `get_or_create` returns pointer-equal Arcs under
   contention.
3. Registry callbacks run outside every registry lock.
4. Broadcaster drain tasks do NOT hold a strong
   `Arc<FragmentBroadcaster>`. Comment on every drain function
   (archive, HLS, DASH) documents the trap; all three drains
   hold only the `BroadcasterStream` Receiver side.

### Remaining to Tier 4 entry

| Slice                                | Remaining sessions | Calendar |
|--------------------------------------|--------------------|----------|
| RTSP playback egress (RTP packetize) | 5-8                | 0.5 wk   |
| lvqr-cmaf standalone with mp4-atom   | 5-8                | 0.5 wk   |
| Apple mediastreamvalidator in CI     | 3-5                | 0.25-0.5 wk |
| Tier 1 gaps (playwright, soak, comparison) | 15-20        | 1-2 wk (24h soak bound) |
| Tier 3 full                          | 40-55              | 3-5 wk   |
| Tier 4 full                          | 50-70              | 4-7 wk   |

**Realistic M4 ETA: 3-5 calendar weeks of sustained sessions.**
The observer deletion closed the biggest structural gap the
Tier 2.1 migration had left; every new ingest protocol and every
new egress now plugs in via the registry surface alone.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **RTSP playback egress**: PLAY direction works at protocol
   level but does not packetize outbound RTP.
2. **Apple mediastreamvalidator in CI**: biggest audit gap.
3. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
4. **Tiers 3-5**: not started.

### Session 61 entry point

Priority order:

1. **RTSP playback egress**. RTP packetization for the PLAY
   direction. H.264: single NAL or FU-A per RFC 6184. HEVC:
   FU per RFC 7798. AAC: RFC 3640. Integrate with the existing
   `lvqr-rtsp` `play` path; broadcaster subscription is the
   producer side.

2. **Apple mediastreamvalidator in CI**. GitHub Actions job
   that runs Apple's validator against `lvqr-hls`-generated
   playlists and blocks merges on validator-red. Biggest
   audit-findings gap after the consumer migration.

3. **lvqr-cmaf with mp4-atom**. Existing hand-rolled fMP4
   writer works for AVC + HEVC + AAC + Opus today; mp4-atom
   buys clean AV1 + timed-text support in one swap.

4. **Tier 3 planning**. Cluster (chitchat, 4 human-weeks / ~15-20
   LLM sessions) + observability (OTLP, 2.5 human-weeks / ~10-15
   LLM sessions). Start planning once the items above land.


## Sessions 53-59 cycle summary (2026-04-09 → 2026-04-16)

Seven sessions on top of session 52's green baseline. 20 commits,
+3565 / -220 net lines. Tests 495 → 529 (+34). Every cycle
commit authored solely as Moheeb Zara, no Co-Authored-By
trailers, no emojis.

### What landed

* **Tier 2.1 primitive surface complete** (sessions 53-55).
  `MoqGroupStream` + `MoqTrackStream` (MoQ -> Fragment inverse,
  session 53). `FragmentBroadcaster` single-producer fan-out
  with lagged-subscriber skip-and-continue semantics (session
  54). `FragmentBroadcasterRegistry` keyed lookup with
  double-checked insertion (session 55). All proptested.

* **Ingest migration complete, all 4 crates** (sessions 56-58).
  RTSP (56), SRT (57), WHIP (58), RTMP bridge (58). Every
  ingest protocol publishes through both the legacy
  `FragmentObserver` and the shared `FragmentBroadcasterRegistry`
  via `lvqr_ingest::{publish_init, publish_fragment}` helpers.
  Each migration pins a dual-wire regression test proving the
  broadcaster side and observer side see identical fragments.

* **First consumer switchover** (session 59). Archive indexer
  migrated off `FragmentObserver` onto an `on_entry_created`
  callback + per-broadcaster drain task. The shared registry
  is now threaded through every ingest crate in lvqr-cli.
  `IndexingFragmentObserver` deleted.

### Load-bearing invariants captured in source comments

Future sessions that refactor any of these MUST preserve them;
regressions will either hang tests or leak resources:

1. **FragmentBroadcaster sender lives outside the Arc<Shared>
   subscribers hold** (session 54). Otherwise a subscriber keeps
   the sender alive and `recv()` never returns `Closed` after
   every producer clone drops.

2. **Registry get_or_create returns pointer-equal Arcs under
   contention** (session 55 proptest). Racing ingest peers on
   one broadcast_id collapse onto one broadcaster.

3. **Registry callbacks run outside every registry lock**
   (session 59). Lets callbacks freely subscribe / get / inspect
   without deadlocking.

4. **Broadcaster drain tasks do NOT hold a strong
   Arc<FragmentBroadcaster>** (session 59). Keepalive Arc would
   prevent the recv loop from ever seeing `Closed`; in the
   archive indexer case it held redb's exclusive lock forever.
   Subscribe is sufficient; the `BroadcasterStream` already owns
   only the Receiver.

### Velocity note

Project started ~1 calendar week ago. Observed pace is ~10-15
sessions per calendar week at ~2-3 commits + ~300-500 lines each.
Ingest migration accelerated over time: session 56 did 1 crate,
session 57 did 1, session 58 did 2. Once the primitive surface
landed, per-crate wiring became mechanical. Revised realistic M4
ETA: 3-5 calendar weeks of sustained sessions, bounded by
genuinely-calendar items (24h soak, multi-node cluster
iteration, WASM / AI research items).

### Cycle gates

On every commit (not just each session close):

* `cargo fmt --all --check` clean
* `cargo clippy --workspace --all-targets -- -D warnings` clean
* `cargo test -p <crate> --lib` for focused work; `--tests` for
  integration-touching work; `cargo test --workspace` before the
  handoff commit

`git log -1 --format='%an <%ae>'` verified `Moheeb Zara
<hackbuildvideo@gmail.com>` after every commit.

## Session 59 close (2026-04-16)

### What shipped (2 commits, +336/-127 lines)

1. **Registry entry-created callback hook** (`7f90ff8`). Adds
   `FragmentBroadcasterRegistry::on_entry_created(callback)` that
   fires exactly once per fresh (broadcast, track) insertion. The
   callback runs with every registry lock released so it may
   freely subscribe / inspect the registry without deadlocking
   (a dedicated test pins this property). Racing callers that
   collapse onto an existing entry do NOT fire the callback.
   3 new tests: fires-exactly-once, multiple-callbacks-all-fire,
   callback-may-subscribe-without-deadlock.

2. **Archive indexer switched to broadcaster-native**
   (`3bf386a`). First consumer-side switchover of the Tier 2.1
   migration. The archive / DVR indexer is no longer a
   FragmentObserver; it is a registry on_entry_created callback
   that spawns one tokio drain task per (broadcast, track) and
   streams `next_fragment()` into the same on-disk layout + redb
   index the observer path used.

   Wiring: `lvqr-cli::start` now constructs one shared
   `FragmentBroadcasterRegistry` and threads it through every
   ingest crate via `with_registry`. RtmpMoqBridge, WhipMoqBridge,
   SrtIngestServer, RtspServer all publish to this one registry
   so a single consumer per (broadcast, track) sees fragments
   regardless of which ingest protocol produced them.

   `IndexingFragmentObserver` is deleted. HLS + DASH stay on the
   observer path for now; their switchovers follow.

   Critical non-regression documented in the drain() source: the
   task does NOT hold a strong `Arc<FragmentBroadcaster>`. A
   first draft stashed one "for keepalive" and the recv loop
   never saw `Closed` after every ingest clone dropped; redb
   held its exclusive lock forever and the rtmp_archive_e2e
   test panicked on a follow-up open. Removing the Arc keepalive
   is the fix and the comment on the drain function calls the
   trap out so future refactors do not regress.

### Ground truth (session 59 close)

* **Head**: `3bf386a` on `main`. v0.4.0.
* **Tests**: 529 passed, 0 failed, 1 ignored. Delta from session
  58: +3 (3 new registry callback tests; archive E2E runs
  against the new dispatch path without regression).
* **Code**: +336 lines / -127 lines. Net +209 lines (new
  BroadcasterArchiveIndexer + callback API, minus deleted
  IndexingFragmentObserver).
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### Consumer-side migration status

| Consumer                      | Status             | Session |
|-------------------------------|--------------------|---------|
| Archive indexer               | broadcaster-native | 59      |
| LL-HLS bridge                 | observer-only      | -       |
| DASH bridge                   | observer-only      | -       |

One down, two to go. After both HLS + DASH migrate, the observer
side of `publish_init` / `publish_fragment` becomes dead code and
the `FragmentObserver` trait can be deleted from every ingest
crate (and from `lvqr-ingest::observer`).

### Remaining to Tier 4 entry

| Slice                                | Remaining sessions | Calendar |
|--------------------------------------|--------------------|----------|
| HLS + DASH switchover + observer deletion | 4-6            | 0.25-0.5 wk |
| lvqr-cmaf standalone with mp4-atom   | 5-8                | 0.5 wk   |
| Apple mediastreamvalidator in CI     | 3-5                | 0.25-0.5 wk |
| Tier 1 gaps (playwright, soak, comparison) | 15-20        | 1-2 wk (24h soak bound) |
| Tier 3 full                          | 40-55              | 3-5 wk   |
| Tier 4 full                          | 50-70              | 4-7 wk   |

**Realistic M4 ETA: 3-5 calendar weeks of sustained sessions.**
Unchanged floor: 24h soak + multi-node cluster iteration +
WASM/AI research components.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **HLS + DASH consumer switchover**: same pattern as archive.
   Both live in lvqr-cli today (HlsFragmentBridge in hls.rs,
   DashFragmentBridge re-exported from lvqr-dash). Switch each
   to register an on_entry_created callback against the shared
   registry, then delete the observer impls.
2. **FragmentObserver deletion**: once every consumer is
   broadcaster-native, the trait + its transitive API
   (publish_*'s observer branch, with_observer builders on
   every ingest crate) is dead. Remove.
3. **RTSP playback egress**: PLAY direction works at protocol
   level but does not packetize outbound RTP.
4. **Apple mediastreamvalidator in CI**: biggest audit gap.
5. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
6. **Tiers 3-5**: not started.

### Session 60 entry point

Priority order:

1. **HLS bridge switchover**. HlsFragmentBridge in
   `lvqr-cli/src/hls.rs` is a FragmentObserver today. Replace
   with a broadcaster-native version that registers an
   on_entry_created callback; per-broadcaster drain task pushes
   into the HLS chunk coalescer. Delete the observer impl.
2. **DASH bridge switchover**. DashFragmentBridge in lvqr-dash.
   Same pattern.
3. **Delete FragmentObserver**. Remove the observer branches of
   publish_init / publish_fragment; remove with_observer
   builders from every ingest crate; drop the trait from
   lvqr-ingest::observer.
4. **RTSP playback egress** (independent, slots in after
   cleanup).

## Session 58 close (2026-04-16)

### What shipped (2 commits, +220/-33 lines)

1. **WHIP bridge dual-wired** (`0593bf2`). Third ingest crate.
   WhipMoqBridge gains `FragmentBroadcasterRegistry` field
   (+ `with_registry` builder + `registry()` getter). Four emit
   sites migrated: video init (codec_fourcc -> 0.mp4 @ 90 kHz),
   audio init (Opus -> 1.mp4 @ 48 kHz), video fragment (codec
   from sample.codec: avc1/hev1), audio fragment (Opus).

   One new unit test:
   `dual_wire_broadcaster_matches_observer_for_video_and_audio`
   drives an H.264 keyframe + Opus frame through the bridge,
   asserts both observer spy and registry broadcasters receive
   init segments and fragments, then pushes a second round and
   proves a subscriber taken on the pre-existing broadcaster
   receives the video delta and the second Opus fragment.

2. **RTMP bridge dual-wired** (`0d31c34`). Fourth and final ingest
   crate. Same pattern, adapted for the closure-based callback
   architecture: `registry_video` and `registry_audio` clones
   alongside the existing `observer_video` / `observer_audio`
   clones in `create_rtmp_server`. Four emit sites inside the
   on_video + on_audio closures migrated. Codec strings RFC 6381
   (avc1 video, mp4a.40.2 AAC audio). Audio timescale captured
   from the AAC sequence header.

### Ground truth (session 58 close)

* **Head**: `0d31c34` on `main`. v0.4.0.
* **Tests**: 526 passed, 0 failed, 1 ignored. Delta from session
  57: +1 (WHIP dual-wire integration test).
* **Code**: +220 lines, -33 lines. Four files touched
  (lvqr-whip/src/bridge.rs, lvqr-ingest/src/bridge.rs and the
  test-only additions).
* **CI gates locally clean**: fmt, clippy (`-D warnings`),
  test --workspace all green.

### Tier 2.1 ingest migration: COMPLETE

| Crate                      | Status      | Session |
|----------------------------|-------------|---------|
| lvqr-rtsp                  | dual-wired  | 56      |
| lvqr-srt                   | dual-wired  | 57      |
| lvqr-whip                  | dual-wired  | 58      |
| lvqr-ingest (RTMP bridge)  | dual-wired  | 58      |

Every ingest protocol now publishes fragments through both the
legacy `FragmentObserver` callback AND the shared
`FragmentBroadcasterRegistry`. Next phase is the consumer-side
switchover: migrate `lvqr-record`/archive indexer and the HLS
bridge to `registry.subscribe(broadcast, track)`, then delete
the `FragmentObserver` trait + `publish_init` / `publish_fragment`
observer branches, leaving a single broadcaster-only dispatch
path.

### Remaining to Tier 4 entry

| Slice                                | Remaining sessions | Calendar |
|--------------------------------------|--------------------|----------|
| Consumer-side migration + observer deletion | 4-8         | 0.25-0.5 wk |
| lvqr-cmaf standalone with mp4-atom   | 5-8                | 0.5 wk   |
| Apple mediastreamvalidator in CI     | 3-5                | 0.25-0.5 wk |
| Tier 1 gaps (playwright, soak, comparison) | 15-20        | 1-2 wk (24h soak bound) |
| Tier 3 full                          | 40-55              | 3-5 wk   |
| Tier 4 full                          | 50-70              | 4-7 wk   |

**Realistic M4 ETA: 3-5 calendar weeks of sustained sessions.**
Ingest migration moved faster than the session-57 estimate (2
crates this session vs 1 forecast), validating the "mechanical at
this point" observation once the primitive surface landed.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **Consumer-side migration**: archive indexer + HLS bridge
   still read the observer callback. Switch each to
   `registry.subscribe(...)`, then delete the observer path from
   every dispatch site. Session 59 priority.
2. **RTSP playback egress**: PLAY direction works at protocol
   level but does not packetize outbound RTP.
3. **lvqr-cmaf with mp4-atom**: existing hand-rolled fMP4 writer
   works for H.264+HEVC+AAC+Opus; `mp4-atom` for AV1 / captions
   in Tier 2 polish.
4. **Apple mediastreamvalidator in CI**: biggest audit gap.
5. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
6. **Tiers 3-5**: not started.

### Session 59 entry point

Priority order:

1. **Consumer-side switchover**. The archive indexer
   (`lvqr-archive` and/or `lvqr-record` -- the actual indexer
   lives in cli wiring around `IndexingFragmentObserver`) is the
   simplest starting point: replace its `FragmentObserver` impl
   with a background task that calls `registry.subscribe(broadcast,
   track)` for each broadcast it cares about, reads
   `next_fragment()`, and writes to disk + redb index.

2. **HLS bridge switchover**. Same pattern for
   `lvqr-cli::HlsFragmentBridge` (or wherever the HLS observer
   hook lives today). The HLS path has more state (playlist
   builder, chunk coalescer) but the incoming surface is
   identical.

3. **Delete FragmentObserver**. Once every consumer is
   broadcaster-native, the observer branch inside
   `publish_init` / `publish_fragment` becomes dead. Remove the
   observer side, drop the trait from `lvqr-ingest::observer`,
   drop the `with_observer` builders from every ingest crate.

4. **RTSP playback egress** (independent, slots in after the
   cleanup work).

## Session 57 close (2026-04-16)

### What shipped (2 commits, +346/-34 lines)

1. **Shared dispatch module** (`754aaae`). The session-56 dual-wire
   helpers lived as local fns inside lvqr-rtsp/server.rs. Hoisted
   them into `lvqr-ingest::dispatch` so every ingest crate imports
   one pair of helpers. Module doc captures the migration rationale:
   observer call happens first (preserves pre-migration ordering
   for existing tests); broadcaster side is idempotent via
   get_or_create; no unnecessary fragment clones. 3 unit tests:
   publish_init writes to both paths; publish_fragment writes to
   both paths; publish with None observer still feeds the registry.

   lvqr-rtsp/server.rs updated to import from
   `lvqr_ingest::{publish_init, publish_fragment}` and the local
   copies removed. All 60 lvqr-rtsp tests continue to pass.

2. **SRT ingest dual-wired** (`6b345db`). Second ingest crate
   migrated. SrtIngestServer now owns (or accepts via
   with_registry) a FragmentBroadcasterRegistry, threads it through
   handle_connection -> process_pes -> {process_h264, process_hevc,
   process_aac}, and calls the shared publish_init / publish_fragment
   helpers at every emit site.

   Observer-None early-return removed in each process_* path: the
   broadcaster side still wants the fragment even when no observer
   is wired (future configurations may have only broadcaster
   consumers).

   Codec strings for broadcaster meta are RFC 6381 form (avc1,
   hev1, mp4a.40.2). Audio path uses the real captured
   state.audio_timescale so fragment meta matches ADTS-derived
   sample rate.

   New inline unit test:
   dual_wire_h264_publishes_to_observer_and_broadcaster builds a
   synthetic Annex-B H.264 PES (SPS+PPS+IDR), drives it through
   process_h264 with both a spy observer and a live broadcaster
   subscription, and asserts both see the same init bytes (on
   broadcaster meta) and the same keyframe fragment payload.

### Ground truth (session 57 close)

* **Head**: `6b345db` on `main`. v0.4.0.
* **Tests**: 525 passed, 0 failed, 1 ignored. Delta from session
  56: +4 (3 dispatch module unit tests + 1 SRT dual-wire).
* **Code**: +188 lines across `lvqr-ingest::dispatch` (new),
  -44 lines of duplicated local helpers from lvqr-rtsp. SRT
  grew by +124 lines (dual-wire wiring + integration test).
* **CI gates locally clean**: fmt, clippy (`-D warnings`), test
  --workspace all green.

### Ingest migration status

| Crate         | Status        | Session shipped |
|---------------|---------------|-----------------|
| lvqr-rtsp     | dual-wired    | 56              |
| lvqr-srt      | dual-wired    | 57 (this)       |
| lvqr-whip     | observer-only | -               |
| lvqr-ingest (RTMP bridge) | observer-only | -   |

Halfway through ingest. WHIP + RTMP bridge remain. Once all four
are dual-wired, the consumer-side migration starts: archive
indexer + HLS bridge switch to `registry.subscribe()`, then the
observer trait itself gets deleted.

### Remaining to Tier 4 entry

Slight update from session 56 estimates. Now ~4 sessions into
the migration, with 2 of 4 ingest crates dual-wired. The
per-crate dual-wire pattern is mechanical at this point.

| Slice                                | Remaining sessions | Calendar |
|--------------------------------------|--------------------|----------|
| Ingest migration (WHIP, RTMP, consumer-side, observer deletion) | 6-10 | 0.5-1 week |
| lvqr-cmaf standalone with mp4-atom   | 5-8                | 0.5 week |
| Apple mediastreamvalidator in CI     | 3-5                | 0.25-0.5 week |
| Tier 1 gaps (playwright, 24h soak, MediaMTX comparison) | 15-20 | 1-2 weeks |
| Tier 3 full                          | 40-55              | 3-5 weeks |
| Tier 4 full                          | 50-70              | 4-7 weeks |

**Realistic M4 ETA: 3-5 calendar weeks of sustained sessions.**
Unchanged from session 56: floor is 24h soak + multi-node cluster
iteration + WASM/AI research items.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **WHIP + RTMP ingest migrations**: same dual-wire pattern.
   Session 58 priority.
2. **Consumer-side migration**: once every ingest is dual-wired,
   switch archive indexer + HLS bridge to
   `registry.subscribe(broadcast, track)` one at a time, then
   delete the observer path.
3. **RTSP playback egress**: RTP packetization of PLAY direction.
4. **Apple mediastreamvalidator in CI**: biggest audit gap.
5. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
6. **Tiers 3-5**: not started.

### Session 58 entry point

1. **Dual-wire WHIP ingest**. WhipMoqBridge has an internal
   MoqTrackSink today; add a registry alongside, thread it
   through the per-connection ingest pump, and use the same
   publish_init / publish_fragment helpers.
2. **Dual-wire RTMP bridge**. RtmpMoqBridge pattern is similar
   to WHIP. Same shape.
3. **Start consumer-side migration**. Archive indexer first
   (it is the simplest consumer; HLS bridge has more state).
4. **RTSP playback egress** (independent, slot in when
   ingest migration wraps).

## Session 56 close (2026-04-16)

## Timeline note (2026-04-16)

Revised estimates based on observed velocity. This project started
~1 calendar week ago and is at session 56, v0.4.0, 521 tests, 10
protocols, every Tier 2.1 primitive shipped, first ingest migration
landed. That is ~10-15 sessions per calendar week with ~2-3 commits
each averaging 300-500 lines. Work throughput is roughly 10-20x a
single human engineer on Rust scaffolding + tests + docs.

Re-baselining ROADMAP calendar (originally written for single-human
velocity, 18-24 months to M4) against LLM velocity:

| Slice                                | Human weeks | LLM sessions | Calendar |
|--------------------------------------|-------------|--------------|----------|
| Tier 2 leftover (ingest migration + lvqr-cmaf + mediastreamvalidator) | 3-5 | 15-25 | 1-2 weeks |
| Tier 1 gaps (playwright, soak, comparison) | 3-4 | 15-20 | 1-2 weeks, bounded by 24h soak calendar |
| Tier 3 (cluster, DVR, OTLP, webhooks, captions, stream keys) | 12-14 | 40-55 | 3-5 weeks |
| Tier 4 (io_uring, WASM, C2PA, federation, AI agents, transcoding, SLO, one-token) | 10-12 | 50-70 | 4-7 weeks |

**Realistic M4 ETA: 3-5 calendar weeks of sustained sessions.**
Floor set by genuinely calendar-bound items (24h soak literally
takes 24h; 3-node chitchat cluster debugging needs real iteration;
WASM filter + AI agents have research components that cannot be
shortcut through raw token throughput).

## Session 56 close (2026-04-16)

### What shipped (1 commit, +250/-31 lines)

1. **RTSP ingest dual-wired to FragmentBroadcasterRegistry**
   (`c1d145c`). First ingest-path migration against the Tier 2.1
   primitive surface.

   `RtspServer` now owns (or accepts via `with_registry`) a
   `FragmentBroadcasterRegistry`, threads it through the full
   connection handling chain, and at every emit site publishes
   both:

   * Legacy `SharedFragmentObserver` callback path (preserved
     exactly for existing HLS / archive consumers).
   * New `FragmentBroadcaster` via `registry.get_or_create(...)`
     + `bc.set_init_segment(init)` + `bc.emit(frag)`.

   Two small helpers, `publish_init` and `publish_fragment`,
   centralize the dual dispatch. These are the seam future
   sessions delete from once every consumer has moved to the
   broadcaster side. The codec strings written into the
   broadcaster meta are standard RFC 6381 form
   (`"avc1"`, `"hev1"`, `"mp4a.40.2"`) with the right timescale
   per track.

   New public API:

   * `RtspServer::with_registry(addr, registry)` constructor
     for external-owned registries (future: multi-protocol
     shared registry at the cli level).
   * `RtspServer::registry()` getter so tests and cli wiring
     can subscribe.

   New integration test: `dual_wire_broadcaster_matches_observer_for_h264_keyframe`
   runs the full ingest handshake + STAP-A + IDR push over real
   TCP, pulls the broadcaster handle out of the registry, asserts
   the init segment is carried on `broadcaster.meta()`, subscribes,
   then verifies both the broadcaster subscription and the observer
   spy receive the IDR keyframe fragment with `flags.keyframe`
   preserved. Pins the migration contract: broadcaster-native
   consumers see equivalent data to observer-native consumers.

   All 60 existing lvqr-rtsp tests still pass.

### Ground truth (session 56 close)

* **Head**: `c1d145c` on `main`. v0.4.0.
* **Tests**: 521 passed, 0 failed, 1 ignored. Delta from session
  55: +1 (new dual-wire integration test).
* **Code**: lvqr-rtsp grew by +219 net lines across `src/server.rs`
  and `tests/integration_server.rs`.
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all green.

### Tier 2.1 migration status

Per-crate migration progress (dual-wired = publishes to both
legacy observer and new broadcaster; broadcaster-only = observer
side removed):

| Crate         | Status        | Session shipped |
|---------------|---------------|-----------------|
| lvqr-rtsp     | dual-wired    | 56 (this)       |
| lvqr-srt      | observer-only | -               |
| lvqr-whip     | observer-only | -               |
| lvqr-ingest   | observer-only | -               |

One down, three to go. SRT is the next-cleanest target (same
ingest shape as RTSP, similar emit-site layout). WHIP and RTMP
already have internal MoqTrackSink wiring so the migration pattern
needs a small adaptation.

### Protocols supported

Unchanged. 10 protocols: RTMP + WHIP + SRT + RTSP ingest;
LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **SRT + WHIP + RTMP ingest migrations**: same dual-wire pattern
   as RTSP. Session 57 priority.
2. **Consumer-side migration off the observer**: archive indexer,
   HLS bridge, etc. still read the observer callback. Once every
   ingest is dual-wired, switch each consumer to
   `registry.subscribe(broadcast, track)` one at a time, then
   delete the observer path.
3. **RTSP playback egress**: PLAY direction works at the protocol
   level but does not packetize outbound RTP.
4. **Apple mediastreamvalidator in CI**: biggest audit gap.
5. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
6. **Tiers 3-5**: not started.
7. **Contract**: 7 missing slots across 5 crates.

### Session 57 entry point

Priority order:

1. **Dual-wire SRT ingest**. Same pattern as RTSP: thread a
   registry through `SrtIngestServer` -> per-connection ingest
   handler -> `process_h264` / `process_hevc` / `process_aac`.
   Add a dual-wire integration test mirroring the RTSP one.
2. **Dual-wire WHIP ingest**. The WHIP bridge already uses
   MoqTrackSink internally; add a registry alongside and publish
   via `publish_init` / `publish_fragment` helpers copy-adapted
   from lvqr-rtsp.
3. **Dual-wire RTMP bridge**. Same as WHIP.
4. **Consumer migration**. Once every ingest is dual-wired, move
   the archive indexer + HLS bridge to `registry.subscribe(...)`,
   delete the observer path from each crate, and delete
   `FragmentObserver` itself.
5. **RTSP playback egress**: RTP packetization (H.264 NAL ->
   single NAL / FU-A, HEVC -> FU, AAC -> RFC 3640). Closes the
   last RTSP competitive gap.
6. **Apple mediastreamvalidator in CI**: the biggest audit gap.

## Session 55 close (2026-04-16)

### What shipped (2 commits, +444/-0 lines)

1. **FragmentBroadcasterRegistry** (`77d602e`). Multi-track keyed
   lookup layer for the Unified Fragment Model. A single
   FragmentBroadcaster covers one logical track; a real server
   hosts many concurrent broadcasts with video + audio + future
   captions + simulcast layers. The registry is the
   `(broadcast, track) -> Arc<FragmentBroadcaster>` lookup table
   the ingest migration will dispatch through.

   API: `get_or_create`, `get`, `subscribe` (convenience over
   `get`), `remove`, `keys`, `len`, `is_empty`. Returns
   `Arc<FragmentBroadcaster>` so racing `get_or_create` callers
   on the same key all see pointer-equal handles. Thread-safe
   via `RwLock<HashMap>`. Clone-cheap via `Arc`.

   Design calls:

   * Double-checked insertion under the write lock: two
     concurrent `get_or_create` calls on the same key collapse
     onto one broadcaster. Real-world scenario: two ingest peers
     that accidentally publish the same `broadcast_id`. The
     registry's contract is they fan out together onto one
     broadcaster rather than splitting into silos.

   * `remove` drops only the registry-side Arc; external clones
     keep the broadcaster alive. Subscribers see `Closed` when
     the last producer-side clone of the sender goes away,
     matching the FragmentBroadcaster contract established in
     session 54.

   * Intentionally not a pub/sub topic space (no wildcards, no
     patterns). Not a lifecycle manager; `BroadcastStarted` /
     `Stopped` events continue to live on `lvqr_core::EventBus`.

   8 inline unit tests covering identity under get_or_create,
   key isolation, miss-returns-None, emission routing, remove
   semantics, keys snapshot, clone sharing, is_empty tracking.

2. **Registry proptest** (`7d96d30`). Two properties in
   `tests/proptest_registry.rs`:

   * **Convergence under racing get_or_create**. 2-8 tokio tasks
     race to `get_or_create` the same key. Every returned Arc is
     pointer-equal. `registry.len() == 1` after the race. A
     single emit through any returned handle fans out to
     subscribers taken through any other handle.

   * **Multi-key isolation**. A randomized workload of up to 5
     distinct `(broadcast, track)` pairs with per-pair plans of
     up to 6 payloads each. Every key's subscriber receives
     exactly that key's plan in order, with no cross-key
     leakage, and every stream closes cleanly after the
     producer side drops.

### Ground truth (session 55 close)

* **Head**: `7d96d30` on `main`. v0.4.0.
* **Tests**: 520 passed, 0 failed, 1 ignored. Delta from session
  54: +10 (lvqr-fragment: 26 -> 35 tests; 8 new inline unit, 2
  new proptest).
* **Code**: lvqr-fragment grew by +444 lines (registry.rs at
  ~293 lines + proptest_registry.rs at ~151 lines).
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all green.
* **lvqr-fragment public API**: now exports
  `FragmentBroadcasterRegistry` alongside
  `FragmentBroadcaster`, `BroadcasterStream`,
  `DEFAULT_BROADCASTER_CAPACITY`, `MoqGroupStream`,
  `MoqTrackStream`, `FragmentStream`, `MoqTrackSink`,
  `MoqSinkError`, `Fragment`, `FragmentFlags`, `FragmentMeta`.

### Tier 2.1 primitive surface: complete

Every building block the ingest migration composes over now
exists, is tested, and has a stable API:

| Primitive                       | Session |
|---------------------------------|---------|
| Fragment / FragmentMeta / FragmentStream | 50 |
| MoqTrackSink (Fragment -> MoQ)  | 50      |
| MoqGroupStream / MoqTrackStream (MoQ -> Fragment) | 53 |
| FragmentBroadcaster (fan-out)   | 54      |
| FragmentBroadcasterRegistry (multi-track lookup) | 55 |

The next step is actual wiring. Each ingest crate (RTMP, WHIP,
SRT, RTSP) constructs a FragmentBroadcasterRegistry on startup,
calls `get_or_create(broadcast, track, meta)` on first fragment,
and emits via the returned broadcaster instead of calling
FragmentObserver directly. The existing FragmentObserver becomes
a subscriber shim: a tokio task per live broadcaster reads
`next_fragment()` and re-invokes the observer callback. Once
every ingest has moved and every consumer is broadcaster-native,
the FragmentObserver + shim pair gets deleted.

### Remaining work to reach Tier 4 (rough engineer-weeks)

Mostly unchanged from session 54. Ingest migration now has no
architectural blockers; it is wiring work against a fixed
interface. Updated breakdown:

* **Tier 2 leftover** (~3 weeks): ingest migration (1-2),
  lvqr-cmaf standalone with mp4-atom (~1), Apple
  mediastreamvalidator in CI (~1, biggest audit gap).
* **Tier 1 leftover** (~3-4 weeks): playwright E2E, 24h soak rig,
  MediaMTX comparison harness.
* **Tier 3 full** (~13 weeks): chitchat cluster (4), DVR scrub UI
  (1.5), webhook+OAuth+signed URLs (1.5), OTLP observability +
  Grafana (2.5), hot config reload (1), captions+SCTE-35 (2),
  stream key lifecycle (1).

Total: ~18-20 focused engineer-weeks before Tier 4 entry.
Tier 4 itself is 10-12 weeks. M4 at roughly week 28-32 of
concentrated work from this point.

### Protocols supported

Unchanged from session 52/53/54. 10 protocols: RTMP + WHIP + SRT +
RTSP ingest; LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **Ingest migration to broadcaster dispatch**: every primitive
   is shipped and tested; per-crate wiring deferred. Session 56
   priority #1.
2. **RTSP playback egress**: PLAY direction works at the
   protocol level but does not packetize outbound RTP.
3. **Peer mesh media relay**: topology works, forwarding is
   Tier 4.
4. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
5. **Tiers 3-5**: cluster, observability, WASM, SDKs not
   started.
6. **Contract**: 7 missing slots across 5 crates.

### Session 56 entry point

Priority order:

1. **Ingest migration, RTSP first**. Add
   `FragmentBroadcasterRegistry` to the RTSP server state.
   Replace the direct `observer.on_fragment(broadcast, track, &frag)`
   calls in `process_video_rtp` / `process_audio_rtp` with
   `broadcaster.emit(fragment)` where `broadcaster` comes from
   `registry.get_or_create(...)`. Wire the existing observer as
   a subscriber shim: spawn a tokio task per broadcaster that
   reads `next_fragment()` and re-invokes the observer. On
   `init_segment` emit, call the observer's `on_init` out-of-
   band (since init is not a Fragment). Preserve every existing
   test. Once RTSP is green, repeat for SRT (same shape),
   WHIP (already partly fragment-shaped), RTMP last.

2. **RTSP playback egress**: RTP packetization for the PLAY
   direction. H.264 NAL -> single NAL / FU-A, HEVC -> FU,
   AAC -> RFC 3640. Closes the last RTSP competitive gap.

3. **Apple mediastreamvalidator in CI**: the biggest audit gap.
   Wire as a GitHub Actions step against lvqr-hls-generated
   playlists. Blocking LL-HLS changes until green.

4. **Tier 3 planning**: cluster via chitchat (4 weeks), OTLP
   observability (2.5 weeks).

## Session 54 close (2026-04-16)

### What shipped (2 commits, +496/-1 lines)

1. **FragmentBroadcaster primitive** (`b4815ef`). Single-producer,
   multi-subscriber fan-out of `Fragment` values in lvqr-fragment,
   built on `tokio::sync::broadcast`. Completes the Tier 2.1
   infrastructure surface the ingest migration composes over.

   Design:

   * Producer: `FragmentBroadcaster::emit(Fragment)` returns the
     count of subscribers that received the fragment. Zero is a
     valid state (no subscribers connected yet), not an error.
     Never blocks on slow consumers.

   * Consumer: `FragmentBroadcaster::subscribe() -> BroadcasterStream`
     returns a `FragmentStream`-impl backed by a broadcast receiver.
     Slow subscribers hit `RecvError::Lagged`, the adapter
     skip-counts them, warn-logs the gap, and continues. Live
     datapath never stalls.

   * Metadata update: `set_init_segment(Bytes)` / `replace_meta(FragmentMeta)`
     update the broadcaster's canonical meta. Future subscribers
     see it immediately; existing subscribers call `refresh_meta()`
     to observe a late init-segment bind without resubscribing.

   * Clone-cheap via `Arc<Shared>` + cloned `broadcast::Sender`.
     The Sender deliberately lives on FragmentBroadcaster alone,
     NOT inside the Arc subscribers hold -- otherwise a live
     subscriber would extend the sender's lifetime and `recv()`
     would never return `Closed` after every producer-side clone
     was dropped. A first draft shipped the bug; a test hung
     until the split. Module doc calls the split out explicitly
     so future refactors do not reintroduce it.

   8 inline unit tests (single subscriber, multi subscriber, late
   subscriber, lagged skip, producer-drop closes, init bind visible
   to new, refresh_meta for late init, clone shares state).

2. **FragmentBroadcaster proptest** (`0e917c5`). Two properties in
   `tests/proptest_broadcaster.rs`:

   * For any plan of up to 16 fragments and up to 4 subscribers,
     every subscriber receives every emitted fragment in order
     with bytes preserved, and every subscriber's stream closes
     after the last emit when the producer drops. Pins the fan-out
     guarantee across the realistic MoQ + HLS + archive + one-extra
     egress count.

   * Deterministic lag scenario: capacity 4, 20 emits, drop
     producer, drain. Surviving fragments arrive with monotonic
     group_id (no out-of-order within the survived window) and
     the stream closes cleanly. Pins the non-hang property that
     the first broadcaster draft regressed on; any future refactor
     that reintroduces the sender-in-Arc trap fails this test.

### Ground truth (session 54 close)

* **Head**: `0e917c5` on `main`. v0.4.0.
* **Tests**: 510 passed, 0 failed, 1 ignored. Delta from session
  53: +10 (lvqr-fragment: 16 -> 26 tests; 8 new inline unit, 2 new
  proptest).
* **Code**: lvqr-fragment grew by +390 lines (broadcaster.rs at
  ~370 lines + proptest_broadcaster.rs at ~107 lines) plus small
  lib.rs / Cargo.toml updates. tokio gained as a runtime dep
  (default-features = false, features = ["sync"]).
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all green.
* **lvqr-fragment public API**: now exports `FragmentBroadcaster`,
  `BroadcasterStream`, `DEFAULT_BROADCASTER_CAPACITY`,
  `MoqGroupStream`, `MoqTrackStream`, `FragmentStream`,
  `MoqTrackSink`, `MoqSinkError`, `Fragment`, `FragmentFlags`,
  `FragmentMeta`.

### Tier 2.1 status

The Unified Fragment Model infrastructure surface is now complete:

* `Fragment`, `FragmentMeta`, `FragmentStream` types (session 50).
* `MoqTrackSink`: Fragment -> MoQ projection (session 50).
* `MoqGroupStream`, `MoqTrackStream`: MoQ -> Fragment inverse
  (session 53).
* `FragmentBroadcaster`: in-process fan-out primitive for the
  ingest migration (session 54).

What is left for Tier 2.1: wiring. Each ingest crate (RTMP, WHIP,
SRT, RTSP) should construct a `FragmentBroadcaster` per
`(broadcast, track)` on first fragment and emit into it. The
existing `FragmentObserver` hook becomes a subscriber shim over
the broadcaster during the migration period and is removed once
every ingest has moved.

### Remaining work to reach Tier 4 (rough engineer-weeks)

* **Tier 2 leftover**: ingest migration (1-2 weeks now that the
  primitive is stable), `lvqr-cmaf` standalone with `mp4-atom`
  for HEVC/AV1 sample entries (~1 week), Apple
  `mediastreamvalidator` in CI (~1 week, biggest audit gap).
* **Tier 1 leftover**: playwright E2E, 24h soak rig, MediaMTX
  comparison harness (~3-4 weeks).
* **Tier 3 full**: cluster via chitchat (4), DVR scrub UI (1.5),
  webhook + OAuth + signed URLs (1.5), OTLP observability +
  Grafana (2.5), hot config reload (1), captions + SCTE-35 (2),
  stream key lifecycle (1). ~13 weeks.

Total: ~18-21 focused engineer-weeks before Tier 4 entry. Tier 4
itself is 10-12 weeks. M4 (ROADMAP's "LiveKit-alternative" gate)
at roughly week 28-33 of concentrated work from this point.

### Protocols supported

Unchanged from session 52/53. 10 protocols: RTMP + WHIP + SRT +
RTSP ingest; LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Known gaps

1. **Ingest migration to broadcaster dispatch**: primitive shipped;
   per-crate wiring deferred. Session 55 priority #1.
2. **RTSP playback egress**: PLAY direction works at the RTSP
   protocol level but does not packetize outbound RTP.
3. **Peer mesh media relay**: topology works, forwarding is Tier 4.
4. **Tier 1 infra**: no playwright, no 24h soak, no MediaMTX
   comparison harness.
5. **Tiers 3-5**: cluster, observability, WASM, SDKs not started.
6. **Contract**: 7 missing slots across 5 crates.

### Session 55 entry point

Priority order:

1. **Ingest migration, one crate at a time**. RTSP is the newest
   and cleanest place to start: introduce a
   `Map<(broadcast, track), Arc<FragmentBroadcaster>>` in the
   server, migrate `process_video_rtp` / `process_audio_rtp` to
   `broadcaster.emit(fragment)`, and wire the existing
   FragmentObserver as a subscriber shim that consumes the
   broadcaster via a background task and re-emits the observer
   callbacks. This preserves every downstream consumer (archive,
   HLS) unchanged. Once RTSP is green, SRT next (same shape),
   then WHIP + RTMP (already partly fragment-shaped internally).

2. **RTSP playback egress** -- RTP packetization for the PLAY
   direction. H.264 NAL -> single NAL / FU-A, HEVC -> FU,
   AAC -> RFC 3640. Closes the last RTSP competitive gap.

3. **Apple mediastreamvalidator in CI** -- biggest audit gap.
   Wire as a GitHub Actions step against `lvqr-hls`-generated
   playlists. Blocking LL-HLS changes until green.

4. **Tier 3 planning** -- cluster (chitchat), observability (OTLP).

## Session 53 close (2026-04-16)

### What shipped (2 commits, +573/-1 lines)

1. **MoQ -> FragmentStream adapters** (`e8d736a`).
   `lvqr-fragment` gains `moq_stream` module with two new types:

   * `MoqGroupStream` consumes a single `lvqr_moq::GroupConsumer`
     and yields one `Fragment` per frame. When the meta carries an
     init segment the first frame of the group is stripped as init
     prefix; the next frame is flagged KEYFRAME, the rest DELTA.
     `without_init_prefix` constructor skips that step when every
     frame is a payload.

   * `MoqTrackStream` composes `MoqGroupStream` across a
     `lvqr_moq::TrackConsumer`. Pulls the next group, drains it,
     pulls the next, presenting a flat Fragment sequence to
     consumers. Keyframe flag resets per group boundary.

   Round-trip is documented as payload-lossless but not
   field-identity: dts/pts/duration/priority are zero on the
   consumer side (sink does not encode them onto the wire), and
   group_id / object_id are re-derived from the MoQ group sequence
   rather than carried from the producer's Fragment.

   Closes ROADMAP 2.1 deliverable line 154 "Adapter from
   `moq_lite::Group` to `FragmentStream`". Enables future
   relay-of-relays, DVR fetch, and cross-node fanout paths to
   treat MoQ-sourced broadcasts identically to locally-ingested
   ones via the same downstream sink interfaces.

   3 unit tests inline (group init-prefix strip, group without
   init, sink->track roundtrip through two groups).

2. **Round-trip integration + proptest coverage** (`3976c4c`).

   * `tests/moq_stream_roundtrip.rs`: one fixed scenario with a
     late-binding init segment and two groups (kf1+d1+d2, kf2).
     Symmetric partner of the existing `integration_sink.rs`.
     Verifies group_id projects from MoQ sequence, object_id
     resets per group, and the keyframe flag restarts at boundaries.

   * Extended `tests/proptest_fragment.rs` with
     `track_stream_roundtrip_preserves_every_payload_byte`: runs
     the existing `fragment_plan_strategy` plans through
     MoqTrackSink -> MoqTrackStream and asserts the flat payload
     sequence comes back byte-for-byte across arbitrary keyframe /
     delta interleavings. Consumer-side twin of the existing
     `sink_roundtrip_preserves_every_payload_byte` property.

### Ground truth (session 53 close)

* **Head**: `3976c4c` on `main`. v0.4.0.
* **Tests**: 500 passed, 0 failed, 1 ignored, 96+ suites. Delta
  from session 52: +5 (lvqr-fragment: 11 -> 16 tests).
* **Code**: lvqr-fragment grew to 820 lines (was ~530) across
  5 source files (added `moq_stream.rs` at 253 lines) plus
  expanded tests (+126 lines). Workspace still 23 crates.
* **CI gates locally clean**: `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` all green.
* **lvqr-fragment public API**: now exports `MoqGroupStream` and
  `MoqTrackStream` alongside the existing `Fragment`, `FragmentFlags`,
  `FragmentMeta`, `FragmentStream`, `MoqTrackSink`, `MoqSinkError`.

### Protocols supported

Unchanged from session 52. 10 protocols total: RTMP + WHIP + SRT +
RTSP ingest, LL-HLS + DASH + WHEP + MoQ + WebSocket egress.

### Competitive position

The MoQ bridge is now bidirectionally symmetric in the Fragment
model: any MoQ track LVQR subscribes to can be re-projected into
downstream sinks without a codec-specific path. MediaMTX and Ant
Media have no unified-fragment equivalent; LiveKit has no MoQ at
all. This is groundwork for Tier 3 cross-node MoQ relay.

### Known gaps (rolling from session 52)

1. **Ingest migration to FragmentStream dispatch**: SRT, RTSP,
   RTMP, WHIP still emit fragments via the `FragmentObserver` hook
   pattern rather than producing a shared `FragmentStream` the
   MoqTrackSink and other consumers subscribe to. The types and
   both directions of the MoQ bridge now exist; the remaining
   piece is a multi-subscriber `FragmentBroadcaster` plus
   per-crate wiring. This is deliberately deferred because it
   ripples across four ingest crates and wants its own session's
   attention for a clean commit sequence.
2. **RTSP playback egress**: PLAY direction works at the RTSP
   protocol level but does not packetize fragments into outbound
   RTP.
3. **Peer mesh media relay**: topology works, forwarding is Tier 4.
4. **Tier 1 infra**: no playwright, no 24h soak, no comparison.
5. **Tiers 3-5**: cluster, observability, WASM, SDKs not started.
6. **Contract**: 7 missing slots across 5 crates (lvqr-record
   fuzz, lvqr-moq fuzz+conformance, lvqr-fragment fuzz+conformance,
   lvqr-whip conformance, lvqr-whep conformance). Note:
   lvqr-fragment now has a clear proptest round-trip for both
   directions of the MoQ bridge, which closes the bulk of the
   proptest contract slot even though the formal contract file
   may still list the crate as pending.

### Session 54 entry point

Priority order:

1. **FragmentBroadcaster + ingest migration**. Introduce a
   multi-subscriber `FragmentBroadcaster` in lvqr-fragment
   (single producer, fan-out via `tokio::sync::broadcast` or a
   conducer-style primitive so lagged subscribers do not block the
   hot path). Migrate one ingest at a time (RTSP is the newest
   and simplest, SRT next, WHIP/RTMP already have MoqTrackSink
   wired internally so they are easier than they look). Keep the
   existing `FragmentObserver` as a thin adapter over the
   broadcaster for a migration-era backwards compat shim, then
   delete it once every ingest has moved.

2. **RTSP playback egress** -- RTP packetization for the PLAY
   direction. H.264 NAL -> single NAL / FU-A, HEVC -> FU,
   AAC -> RFC 3640. Would close the last RTSP competitive gap.

3. **Tier 3 planning** -- cluster (chitchat), observability (OTLP).

4. **Contract gaps** -- 7 missing slots. Close opportunistically.

## Session 52 close (2026-04-16)

### What shipped (11 commits, +2,113/-24 lines)

1. **RTSP H.264 fragment emission** (`0e51e02`). Wired
   `process_rtp_frame` to extract SPS/PPS from depacketized NALs,
   emit AVC init segment via `write_avc_init_segment`, then build
   AVCC-framed `moof/mdat` fragments through the shared
   `FragmentObserver`. Mirrors the SRT `process_h264` pattern.
   ConnectionState gains ingest bookkeeping fields (sps, pps,
   video_init_emitted, video_seq, prev_video_dts). 2 new tests.

2. **HEVC RTP depacketizer + dual-codec fragment emission**
   (`9b6c120`). `HevcDepacketizer` in the rtp module handles single
   NAL (types 0-47), AP (type 48), and FU (type 49) per RFC 7798.
   Keyframe detection for IDR_W_RADL (19), IDR_N_LP (20), CRA (21).
   Refactored `process_rtp_frame` to detect codec from session SDP
   tracks and dispatch to H.264 or HEVC processing. HEVC emission
   uses `write_hevc_init_segment` with SPS parsing via `lvqr-codec`.
   Unified `nals_to_length_prefixed` with `NalFilter` enum. 6 new
   tests (38 total in crate).

3. **Wire RTSP into lvqr-cli** (`62891cc`). `--rtsp-port` flag
   (default 0 / disabled, env `LVQR_RTSP_PORT`). When non-zero,
   `start()` pre-binds an `RtspServer` TCP listener and spawns it
   alongside SRT/RTMP/WHIP. `rtsp_addr` added to `ServeConfig`,
   `ServerHandle`, `TestServer`, and `TestServerConfig`
   (`with_rtsp()` builder).

4. **RTSP -> HLS E2E integration test** (`81fd54e`). Proves the
   full path: TCP connect, ANNOUNCE with H.264 SDP, SETUP with
   interleaved transport, RECORD, push interleaved RTP frames
   (STAP-A SPS+PPS, two IDR keyframes), verify HLS playlist at
   `/hls/publish/rtsp_test/playlist.m3u8` contains segments. Fixed
   `RtspServer::bind()` to stash pre-bound `TcpListener` instead
   of drop+rebind race on ephemeral ports.

5. **Proptest for RTSP/RTP parsers** (`952f616`). 11 property
   tests covering every parser that handles untrusted network
   input: RTSP request, Transport header, interleaved framing, RTP
   header, H.264/HEVC/AAC depacketizers, FU reassembly sequences,
   fmtp config parser, plus interleaved round-trip.

6. **AAC RTP depacketizer + audio fragment emission** (`8b656b6`).
   `AacDepacketizer` for RFC 3640 AAC-hbr mode: parses AU headers,
   extracts Access Units. `parse_aac_config_from_fmtp` extracts
   hex-encoded AudioSpecificConfig from SDP fmtp lines. Server
   detects audio vs video by interleaved channel-to-track mapping,
   emits AAC init segments + per-AU fragments on track "1.mp4".
   RTSP now handles H.264 + HEVC + AAC.

7. **Integration tests + contract scope** (`69ecf80`). 3 real-TCP
   integration tests (OPTIONS, DESCRIBE, full ingest handshake with
   spy observer). Enabled lvqr-rtsp in contract checker scope.
   Updated CONTRACT.md.

8. **Fuzz targets** (`35b510e`). 5 libfuzzer targets covering
   parse_rtsp_request, parse_rtp_header + interleaved framing,
   H.264 depack, HEVC depack, AAC depack.

9. **ffprobe conformance** (`57ad808`). AVC init + IDR media and
   AAC init + AU media validated through ffprobe. Soft-skips when
   ffprobe is not on PATH.

### Ground truth (session 52 close)

* **Head**: `57ad808` on `main`. v0.4.0.
* **Tests**: 495 passed, 0 failed, 1 ignored, 96 suites.
* **Code**: 37,381 lines of Rust across 23 crates.
* **CI**: `cargo fmt`, `cargo clippy --workspace -D warnings`,
  `cargo test --workspace` all clean.
* **Contract**: lvqr-rtsp is 5/5 (proptest, fuzz, integration,
  E2E, conformance). 7 missing slots across 5 crates remain
  (all low-value: pure-value-type fuzz, external-tool conformance).
* **lvqr-rtsp crate**: 60 tests (44 unit, 11 proptest, 3
  integration, 2 conformance) plus 5 fuzz targets and 1 E2E
  in lvqr-cli.

### Protocols supported

| Protocol | Direction | Crate | Status | E2E tested |
|----------|-----------|-------|--------|------------|
| RTMP | ingest | lvqr-ingest | DONE | yes |
| WHIP | ingest | lvqr-whip | DONE | yes |
| SRT | ingest | lvqr-srt | DONE (H.264+HEVC+AAC) | yes |
| RTSP | ingest | lvqr-rtsp | DONE (H.264+HEVC+AAC) | yes |
| WebSocket | ingest+egress | lvqr-cli | DONE | yes |
| LL-HLS | egress | lvqr-hls | DONE | yes |
| DASH | egress | lvqr-dash | DONE | yes |
| WHEP | egress | lvqr-whep | DONE | yes |
| MoQ | egress | lvqr-moq | DONE | yes |

### Competitive position

LVQR now has 10 protocols (4 ingest, 6 egress). The only Rust
library with HEVC over SRT and RTSP as composable crates. No
competitor offers RTMP + WHIP + SRT + RTSP ingest behind a single
binary with LL-HLS + DASH + WHEP + MoQ egress. LiveKit has no SRT
or RTSP. MediaMTX handles RTSP but is monolithic Go. Ant Media's
RTSP is Java.

### Known gaps

1. **RTSP playback egress**: PLAY direction works at the RTSP
   protocol level (state machine, DESCRIBE/SETUP/PLAY handshake)
   but does not packetize fragments into outbound RTP.
2. **Unified Fragment Model** (Tier 2.1): types exist but ingress
   not migrated to fragment-stream-based dispatch.
3. **Peer mesh media relay**: topology works, forwarding is Tier 4.
4. **Tier 1 infra**: no playwright, no 24h soak, no comparison.
5. **Tiers 3-5**: cluster, observability, WASM, SDKs not started.
6. **Contract**: 7 missing slots across 5 crates (lvqr-record
   fuzz, lvqr-moq fuzz+conformance, lvqr-fragment fuzz+conformance,
   lvqr-whip conformance, lvqr-whep conformance). All explicitly
   low-value or waiting on external tooling.

### Session 53 entry point

Priority order:

1. **Tier 2.1 unified fragment model** -- migrate ingest paths
   to fragment-stream-based dispatch. The Fragment/FragmentStream
   types exist in lvqr-fragment; the bridge needs to produce
   FragmentStream instead of writing directly to observers.

2. **RTSP playback egress** -- RTP packetization for the PLAY
   direction. H.264 NAL -> single NAL / FU-A, HEVC -> FU,
   AAC -> RFC 3640. Would make RTSP bidirectional.

3. **Tier 3 planning** -- cluster (chitchat), observability (OTLP).

4. **Contract gaps** -- 7 missing slots, all low-value. Close
   opportunistically when touching adjacent code.

## Session 51 close (2026-04-16)

### What shipped (7 commits, +2,349/-30 lines)

1. **HEVC in SRT ingest** (`469d8d3`). Wired the stubbed H.265
   code path: VPS/SPS/PPS extraction from Annex B NALs, HEVC SPS
   parsing via lvqr-codec, fMP4 init segment via write_hevc_init_segment,
   keyframe detection (IDR_W_RADL/IDR_N_LP/CRA), annex_b_to_hvcc
   conversion. SRT ingest now handles H.264 + HEVC + AAC, matching
   the WHIP bridge. 3 unit tests + 1 E2E test.

2. **Criterion bench for TsDemuxer** (`cc62f86`). Three benchmark
   groups: feed_bulk, feed_per_packet, pes_reassembly. Immediately
   revealed a 113x throughput bottleneck.

3. **TsDemuxer O(N^2) perf fix** (`71e9649`). Replaced per-packet
   drain(..188) with zero-copy cursor over input slice. 100x4KB
   bulk: 44 MiB/s -> 4.96 GiB/s (113x improvement). Large SRT
   datagrams are now processed without copying into remainder buffer.

4. **lvqr-rtsp crate scaffold** (`6db660d`). Tier 2.9 foundation:
   RTSP/1.0 message parser, session state machine (Init -> Ready
   -> Playing/Recording), SDP track parser, TCP server with
   per-connection handler, full playback + ingest handshake flows.
   23 unit tests. +1,221 lines across 4 source files.

5. **RTP interleaved frame parsing + H.264 depacketization**
   (`cf8e3e9`). rtp module with interleaved TCP frame parser,
   RTP header parser, H.264 depacketizer (single NAL, STAP-A,
   FU-A per RFC 6184). Connection handler distinguishes $-framed
   RTP from RTSP text requests. 8 unit tests (31 total in crate).

6. **Handoff refresh** (`2a53d83`). Session 51 audit block.

### Ground truth (session 51 final audit)

* **Head**: `cf8e3e9` on `main`. v0.4.0.
* **Tests**: 465 passed, 0 failed, 1 ignored, 92 suites.
* **Code**: 35,089 lines of Rust across 23 crates.
* **CI**: All 4 workflows green. `cargo fmt`, `cargo clippy
  --workspace -D warnings`, `cargo test --workspace` all clean.
* **Target cleaned**: 116 GB -> 5.9 GB (accumulated build artifacts).
* **Bench baselines**: TsDemuxer bulk feed 1.2-5.0 GiB/s, per-packet
  ~800ns, PES reassembly 0.9-3.9 GiB/s. PlaylistBuilder render at
  60 segments ~43 us.

### Protocols supported

| Protocol | Direction | Crate | Status | E2E tested |
|----------|-----------|-------|--------|------------|
| RTMP | ingest | lvqr-ingest | DONE | yes |
| WHIP | ingest | lvqr-whip | DONE | yes |
| SRT | ingest | lvqr-srt | DONE (H.264+HEVC+AAC) | yes |
| WebSocket | ingest+egress | lvqr-cli | DONE | yes |
| LL-HLS | egress | lvqr-hls | DONE | yes |
| DASH | egress | lvqr-dash | DONE | yes |
| WHEP | egress | lvqr-whep | DONE | yes |
| MoQ | egress | lvqr-moq | DONE | yes |
| RTSP | playback+ingest | lvqr-rtsp | RTP DEPACK (no fragment emit) | no |

### Competitive position

LVQR is the only Rust library offering HEVC over SRT as composable
crates. LiveKit has zero SRT support. MediaMTX handles HEVC/SRT but
is monolithic Go (cannot be embedded). Ant Media's HEVC is partial.
No mature Rust RTSP server crate exists; our hand-rolled approach
is the correct call. Watch for: HEVC parameter sets spanning TS
packet boundaries in 10-bit Main 10 profile streams (industry gotcha).

### Known gaps

1. **RTSP fragment emission**: rtp module depacketizes H.264 NALs
   but does not yet emit fragments. Needs: reconstruct Annex B from
   depacketized NALs, extract SPS/PPS, call write_avc_init_segment,
   emit moof/mdat via fragment observer. Mirror the SRT ingest
   pattern in `crates/lvqr-srt/src/ingest.rs`.
2. **RTSP HEVC depacketization**: H.265 RTP uses RFC 7798 (FU/AP
   NAL units). NAL type is `(byte >> 1) & 0x3F` (same as HEVC
   everywhere else in the codebase). Add to rtp module after H.264
   fragment emission is proven.
3. **RTSP E2E test**: no test pushes RTP into the RTSP server yet.
   Use ffmpeg `ffmpeg -f lavfi -i testsrc -c:v libx264 -f rtsp
   rtsp://localhost:PORT/publish/test` or build synthetic RTP
   packets like the SRT E2E test does for MPEG-TS.
4. **Unified Fragment Model** (Tier 2.1): types exist but ingress
   not migrated to fragment-stream-based dispatch.
5. **Peer mesh media relay**: topology works, forwarding is Tier 4.
6. **Tier 1 infra**: no playwright, no 24h soak, no comparison.
7. **Tiers 3-5**: cluster, observability, WASM, SDKs not started.
8. **Contract**: 7 missing slots across 5 crates.

### Session 52 entry point

Priority order:

1. **RTSP fragment emission** -- wire the H264Depacketizer output
   to the fragment observer. In `server.rs::process_rtp_frame`,
   collect depacketized NALs into Annex B format, extract SPS/PPS,
   call `write_avc_init_segment` on first keyframe, then emit
   `build_moof_mdat` fragments. The SRT `process_h264` function
   (crates/lvqr-srt/src/ingest.rs:186) is the exact pattern.

2. **RTSP E2E test** -- push synthetic RTP packets over interleaved
   TCP, verify HLS playlist appears. Follow the SRT E2E test
   pattern (crates/lvqr-cli/tests/srt_hls_e2e.rs).

3. **RTSP HEVC depack** -- add HevcDepacketizer to rtp module
   (RFC 7798 FU/AP), then wire fragment emission for HEVC the
   same way SRT's `process_hevc` does it.

4. **Wire RTSP into lvqr-cli** -- add `--rtsp-port` flag,
   spawn RtspServer in the serve command alongside SRT.

5. **Contract gaps** -- fuzz targets for lvqr-record, lvqr-moq,
   lvqr-fragment; conformance tests for lvqr-whip, lvqr-whep.

6. **Tier 3 planning** -- cluster (chitchat), observability (OTLP).

## Session 44 close (2026-04-16)

### M1 scope decision

The roadmap's risk table says: "Cut SRT and RTSP from Tier 2
and move them to Tier 3 if Tier 2 hits 14 weeks without
reaching M1." We are well past that.

**Decision: M1 = RTMP + WHIP + HLS + DASH + WHEP + MoQ.
SRT and RTSP are post-M1.**

LVQR already surpasses MediaMTX on LL-HLS spec compliance,
DASH, WHIP/WHEP, DVR archive, and MoQ. SRT and RTSP are
important for professional broadcast but do not block the
core product story.

### Session 44 commits

1. **CORS on HLS + DASH routers** (`1bda636`). Browser-hosted
   hls.js/dash.js can now fetch cross-origin.
2. **README refresh** (`806d7e4`). Status, CLI reference,
   architecture diagram, crate table all current.

### What M1 looks like now

`lvqr serve` with no flags:
- RTMP ingest on :1935, WHIP via --whip-port
- LL-HLS on :8888 (configurable timing, DVR window, PDT,
  ENDLIST on disconnect)
- DASH via --dash-port (type=static on disconnect)
- WHEP via --whep-port, MoQ on :4443
- Admin on :8080 with optional auth
- Optional recording + archive
- CORS on all HTTP surfaces
- 420 tests, all CI green

### M1 shipped (sessions 45-47)

Both M1 blockers closed: CHANGELOG + v0.4.0 version bump
(`d17e992`), quickstart rewritten for HLS/DASH (`c7d499e`).

Post-M1 SRT foundation: MPEG-TS demuxer (`e8a3cba`) with
proptest + fuzz (`388b76f`). `TsDemuxer::feed` parses PAT/PMT,
reassembles PES with PTS/DTS, handles sync recovery. 4 unit
tests + 5 proptest invariants (deterministic chunked reassembly,
never-panic, valid timestamps). Fuzz target in lvqr-codec/fuzz.

**v0.4.0** on `origin/main`. 429 tests, 0 failures.

### Sessions 49-50 close (2026-04-16)

1. **Delete lvqr-wasm** (`58e1327`). Removed deprecated dead
   code: crate directory, workspace member, CI WASM job, README
   entry. -512 lines.
2. **Fix SRT frame duration** (`7686d0a`). Video and audio
   frame durations now computed from PTS deltas instead of
   hardcoded 3000/1024. Fixes A/V drift on real streams.
3. **SRT -> HLS E2E test** (`758960b`). Pushes synthetic
   MPEG-TS (PAT + PMT + H.264 PES) over SRT via srt-tokio
   caller, fetches HLS playlist from the HTTP surface, asserts
   segments appear. SrtIngestServer gains bind() for ephemeral
   port pre-binding. TestServerConfig gains with_srt().

### Ground truth (session 50 audit)

* **Head**: `758960b` on `origin/main`. v0.4.0.
* **Tests**: 430 passed, 0 failed, 1 ignored, 90 test suites.
* **Code**: 33,318 lines of Rust across 22 crates (lvqr-wasm
  deleted). 37 commits since session 34 start (+4,126/-647).
* **Contract**: 7 missing slots. 5 crates fully compliant
  (lvqr-ingest, lvqr-codec, lvqr-cmaf, lvqr-hls, lvqr-dash).
  Remaining: lvqr-record fuzz, lvqr-moq fuzz+conformance,
  lvqr-fragment fuzz+conformance, lvqr-whip conformance,
  lvqr-whep conformance.
* **CI**: 4 workflows (CI, LL-HLS Conformance, Test Contract,
  Fuzz). All passing on HEAD.

### Protocols supported

| Protocol | Direction | Crate | Status | E2E tested |
|----------|-----------|-------|--------|------------|
| RTMP | ingest | lvqr-ingest | DONE | yes |
| WHIP | ingest | lvqr-whip | DONE | yes |
| SRT | ingest | lvqr-srt | DONE (H.264+AAC) | yes |
| WebSocket | ingest+egress | lvqr-cli | DONE | yes |
| LL-HLS | egress | lvqr-hls | DONE | yes |
| DASH | egress | lvqr-dash | DONE | yes |
| WHEP | egress | lvqr-whep | DONE | yes |
| MoQ | egress | lvqr-moq | DONE | yes |
| RTSP | -- | -- | NOT STARTED | -- |

### Known gaps

1. **HEVC in SRT**: code path exists but returns early. Needs
   VPS/SPS/PPS detection + write_hevc_init_segment wiring.
2. **RTSP server** (Tier 2.9): no crate exists. Last protocol
   gap vs MediaMTX.
3. **Unified Fragment Model** (Tier 2.1): types exist but
   ingress not migrated to fragment-stream-based dispatch.
   Roadmap's #1 architectural decision.
4. **Peer mesh media relay**: topology planning works (13 unit
   tests), actual peer-to-peer media forwarding is Tier 4.
5. **Tier 1 infra**: no playwright E2E, no 24h soak, no
   MediaMTX comparison harness, no testcontainers.
6. **Tiers 3-5**: cluster, observability, WASM, SDKs all not
   started.

### Session 51 entry point

* **HEVC in SRT** (small, complete -- wire the existing code
  path with VPS/SPS/PPS detection).
* **RTSP server** (Tier 2.9 -- the last protocol gap).
* **Criterion bench for TS demuxer** throughput.
* **Tier 3 planning** (cluster, observability, webhooks).

## Sessions 34-43 audit (2026-04-16)

Ten sessions shipped 23 commits (+2,099 lines across 27 files)
that took the LL-HLS and DASH egress from "working prototype"
to "production-grade with operator-tunable knobs". Everything
is on `origin/main` at `62660c4`. CI is green across all three
workflows (Test Contract, LL-HLS Conformance, CI).

### Ground truth numbers

* **Tests**: 420 passing, 0 failing, 1 ignored doctest, 88
  test suites. Delta from session 33: +12 tests.
* **Contract**: 7 missing slots across 5 crates (lvqr-record
  fuzz; lvqr-moq fuzz + conformance; lvqr-fragment fuzz +
  conformance; lvqr-whip conformance; lvqr-whep conformance).
  5 crates fully compliant (lvqr-ingest, lvqr-codec, lvqr-cmaf,
  lvqr-hls, lvqr-dash). Delta from session 33: 9 -> 7.
* **Benches**: render at 60 segments (production cap) ~43 us;
  push_partial ~630 ns; push_segment_boundary ~1 us.
* **CI**: Test Contract 6 s, LL-HLS Conformance 5 m, CI 10 m.
  All passing on HEAD.

### What sessions 34-43 shipped

| Session | Scope | Key commit |
|---------|-------|------------|
| 34 | Sliding-window eviction (max_segments, cache purge, production cap) | `87f1b41` |
| 35 | lvqr-hls + lvqr-cmaf fuzz skeletons (contract 9->7) | `3466dcd`, `bfc02dd` |
| 36 | First criterion bench for PlaylistBuilder | `32cd6dc` |
| 37 | EXT-X-PROGRAM-DATE-TIME per segment (RFC 8216bis) | `293ebe2` |
| 38 | EXT-X-ENDLIST + finalize + --hls-dvr-window flag | `c27d4c9`, `e6db779` |
| 39 | RTMP disconnect -> HLS finalize E2E test | `fac6131` |
| 40 | WHIP disconnect -> BroadcastStopped | `9675a13` |
| 41 | DASH finalize on disconnect (type=static) | `cf6d07f` |
| 42 | DASH finalize unit + E2E tests | `bd4ee91` |
| 43 | Configurable segment/partial timing CLI flags | `8b59fc9` |

### CLI flags added

```
--hls-dvr-window <secs>             DVR rewind depth (default 120)
--hls-target-duration <secs>        segment size (default 2)
--hls-part-target <ms>              partial size (default 200)
```

All confirmed visible in `lvqr serve --help`.

### LL-HLS spec conformance

Every mandatory RFC 8216bis tag is emitted: VERSION, INDEPENDENT-
SEGMENTS, TARGETDURATION, SERVER-CONTROL (CAN-BLOCK-RELOAD,
PART-HOLD-BACK, HOLD-BACK, CAN-SKIP-UNTIL), PART-INF, MAP,
MEDIA-SEQUENCE, PART, PRELOAD-HINT, PROGRAM-DATE-TIME, SKIP,
ENDLIST. ServerControl timing auto-derives from the configured
segment/part durations.

### Disconnect story

Both RTMP (`on_unpublish`) and WHIP (`on_disconnect`) emit
`BroadcastStopped` on the event bus. HLS appends
`EXT-X-ENDLIST`; DASH switches to `type="static"` and omits
`minimumUpdatePeriod`. Both paths have E2E tests.

### What LVQR can do end-to-end

* RTMP ingest (OBS / ffmpeg): AVC video + AAC audio.
* WHIP ingest: H.264 / H.265 video + Opus audio.
* LL-HLS egress: bounded sliding window, per-segment PDT,
  EXT-X-ENDLIST on disconnect, operator-tunable DVR depth +
  segment/part timing, delta playlists, blocking reload.
* MPEG-DASH egress: live-profile dynamic MPD, static on
  disconnect, per-broadcast fan-out.
* WHEP egress: WebRTC subscribers via str0m.
* DVR archive: redb index + HTTP `/playback/*`.
* WebSocket fMP4 relay, disk record, pluggable auth (JWT +
  static token + open access).

### Maturity against ROADMAP.md

* **Tier 0 (audit findings)**: DONE.
* **Tier 1 (test infra)**: ~67%. First criterion bench. Two
  fuzz slots closed. 7 contract slots remaining.
  `mediastreamvalidator` still soft-skips on CI.
* **Tier 2 (protocols)**: ~89%. LL-HLS + DASH production-ready.
  Remaining: VP9/AV1 parsers (2.2), byte-range partials (2.5),
  SRT (2.8), RTSP (2.9), single-binary M1 default (2.10).
* **Tier 3 / 4 / 5**: NOT STARTED.

### Session 44 entry point

Primary scope candidates, in priority order:

1. **SRT ingest** (Tier 2.8). Biggest competitive gap vs
   MediaMTX. Requires libsrt binding (srt-rs or raw FFI),
   MPEG-TS demux, and fragment conversion. Multi-session scope
   but the crate skeleton + socket accept loop can land in one
   session without being half-finished if scoped as an
   `lvqr-srt` crate with a `SrtListener::accept` that yields
   raw TS bytes.

2. **Byte-range partials for LL-HLS**. Cuts per-partial HTTP
   overhead. Requires cache layout change + renderer +
   Range header support. 3-4 commits.

3. **Self-hosted macOS runner** for `mediastreamvalidator`
   primary CI signal.

4. **CORS middleware**. Flagged as tech debt since session 30.
   A permissive default for dev + restrictive for production
   is one commit.



## Session 42 close (2026-04-16)

One commit on top of session 41's docs commit. Closes the
testing gap deferred from session 41.

### Session 42 commit

1. **DASH finalize tests** (`bd4ee91`). Two unit tests in
   `server.rs` (`finalize_switches_mpd_to_static`,
   `finalize_twice_is_harmless`) and one E2E test in
   `rtmp_dash_e2e.rs` (`rtmp_disconnect_produces_static_dash_manifest`)
   that publishes via RTMP, disconnects, and asserts the MPD
   switches from `type="dynamic"` to `type="static"` with no
   `minimumUpdatePeriod`.

### Verification

420 tests passing (+3 from session 41), 1 ignored doctest.
`cargo clippy --workspace --all-targets -- -D warnings` clean.
All pushed to `origin/main`.

### Sessions 34-42 summary (the production readiness arc)

| Session | What | Tests added |
|---------|------|-------------|
| 34 | LL-HLS sliding-window eviction | +3 |
| 35 | lvqr-hls + lvqr-cmaf fuzz skeletons | 0 (nightly-only) |
| 36 | First criterion bench | 0 (bench) |
| 37 | EXT-X-PROGRAM-DATE-TIME | +4 |
| 38 | EXT-X-ENDLIST + finalize + --hls-dvr-window | +2 |
| 39 | RTMP disconnect -> HLS finalize E2E | +1 |
| 40 | WHIP disconnect -> BroadcastStopped | 0 |
| 41 | DASH finalize on disconnect | 0 |
| 42 | DASH finalize unit + E2E tests | +3 |

Total: 410 -> 420 tests across 9 sessions.

### Session 43 entry point

* **Byte-range partials for LL-HLS** (cuts per-partial HTTP
  overhead).
* **Self-hosted macOS runner for mediastreamvalidator**.
* **SRT ingest** (Tier 2.8 in the roadmap -- the biggest
  competitive gap against MediaMTX).
* **Criterion benches for lvqr-cmaf/lvqr-ingest**.



## Session 41 close (2026-04-16)

One commit on top of session 40's `9675a13`. DASH finalize on
broadcaster disconnect, completing the disconnect story for
every egress path.

### Session 41 commit

1. **DASH finalize on disconnect** (`cf6d07f`). `DashState`
   gains `finalized: AtomicBool`. `DashServer::finalize()` sets
   it; `render_manifest()` then produces an MPD with
   `type="static"`, profile `isoff-on-demand`, and no
   `minimumUpdatePeriod`. `MultiDashServer::finalize_broadcast()`
   looks up and finalizes the per-broadcast server. A new
   event subscriber in `lvqr-cli::start()` mirrors the HLS
   finalize subscriber.

## Session 40 close (2026-04-16)

One commit. WHIP disconnect now emits `BroadcastStopped`.

### Session 40 commit

1. **WHIP disconnect -> BroadcastStopped** (`9675a13`).
   `IngestSampleSink` trait gains `on_disconnect(&self, broadcast)`
   (default no-op). `WhipMoqBridge` implements it to remove the
   broadcast from its `DashMap` and emit `BroadcastStopped`.
   `run_session_loop` is split into outer + inner so
   `on_disconnect` fires unconditionally on every exit path.
   `WhipMoqBridge` gains `with_events(EventBus)` builder method;
   `lvqr-cli` wires it.

### The complete disconnect story (sessions 39-41)

| Ingest | Event | HLS | DASH |
|--------|-------|-----|------|
| RTMP | `on_unpublish` -> `BroadcastStopped` (session 39) | `EXT-X-ENDLIST` | `type="static"` |
| WHIP | `on_disconnect` -> `BroadcastStopped` (session 40) | `EXT-X-ENDLIST` | `type="static"` |

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. All DASH tests pass
including the 3 golden MPD fixtures (the live path still renders
`minimumUpdatePeriod="PT2.0S"` unchanged). All 13 cli tests
pass. All pushed to `origin/main`.

### Session 42 entry point

* **Byte-range partials for LL-HLS** (cuts per-partial HTTP
  overhead, 3-4 commits).
* **Self-hosted macOS runner for mediastreamvalidator**.
* **Criterion benches for lvqr-cmaf and lvqr-ingest**.
* **SRT ingest** (Tier 2.8 in the roadmap).



## Session 39 close (2026-04-16)

One commit on top of session 38's `7aba946`. Session 39 wired
the final mile of the LL-HLS DVR surface: when an RTMP publisher
disconnects, the playlist now gains `#EXT-X-ENDLIST` and the
retained window becomes a VOD surface that clients can scrub
freely.

### Session 39 commit list

1. **Wire HLS finalize on RTMP disconnect** (`fac6131`).
   `MultiHlsServer` gains `finalize_broadcast(&self, name)`:
   looks up the video and audio `HlsServer` handles by name
   and calls `finalize().await` on each. A new event subscriber
   in `lvqr-cli::start()` listens for `BroadcastStopped` on
   the event bus and calls `finalize_broadcast`. E2E test
   `rtmp_disconnect_produces_endlist_in_playlist` publishes two
   keyframes, drops the RTMP stream, and asserts
   `#EXT-X-ENDLIST` appears and `#EXT-X-PRELOAD-HINT` is
   suppressed.

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. All 13 `lvqr-cli` tests
pass including the new E2E test. The test runs in ~1 s end-to-end:
RTMP publish -> disconnect -> event propagation -> HTTP GET ->
assertion.

### The complete LL-HLS DVR arc (sessions 34-39)

1. Session 34: `max_segments` sliding window eviction
2. Session 37: `EXT-X-PROGRAM-DATE-TIME` per segment
3. Session 38: `EXT-X-ENDLIST` + `finalize()` + `--hls-dvr-window`
4. Session 39: `finalize_broadcast()` + event bus wiring + E2E test

A broadcaster can now:
  - Publish via RTMP (or WHIP, but disconnect wiring is RTMP-only
    for now)
  - Viewers watch live via LL-HLS with configurable DVR depth
    (`--hls-dvr-window <secs>`)
  - When the broadcaster disconnects, the playlist gains
    `#EXT-X-ENDLIST` and the retained window becomes VOD
  - Full-broadcast replay beyond the DVR window is served via
    `lvqr-record` + `lvqr-archive` at `/playback/*`

### Known gap: WHIP disconnect

The WHIP ingest path does not emit `BroadcastStopped` events.
`WhipMoqBridge` has no event bus integration (unlike
`RtmpMoqBridge` which has it at `bridge.rs:212`). A WHIP
publisher that disconnects will not trigger HLS finalize. This
is a session 40 item.

### Session 40 entry point

* **WHIP disconnect -> BroadcastStopped** (close the WHIP gap).
* **Byte-range partials for LL-HLS**.
* **Self-hosted macOS runner for mediastreamvalidator**.
* **DASH finalize on disconnect** (the DASH bridge has the same
  gap: no end-of-stream signal).



## Session 38 close (2026-04-16)

Two commits on top of session 37's `8f0182e`. Session 38 closed
the LL-HLS DVR / VOD surface end-to-end by answering the
session-35 design question with a pragmatic insight: no new
archive cache is needed. The existing architecture handles DVR
with two small changes: `#EXT-X-ENDLIST` for end-of-stream and a
configurable sliding window. Full-broadcast replay beyond the DVR
window already works via `lvqr-record` + `lvqr-archive`.

### Design decision: DVR surface architecture

The session-35 HANDOFF asked "does the archive cache belong
inside `lvqr-hls` or at the `lvqr-record` layer?" Answer:
**neither needs a new archive**. DVR scrub within the live
window is `max_segments` (already configurable since session 34).
End-of-stream VOD within the window is `#EXT-X-ENDLIST` (session
38 Commit A). Full-broadcast replay is the existing
`lvqr-record` + `lvqr-archive` pipeline at `/playback/*`.

### Session 38 commit list

1. **EXT-X-ENDLIST + PlaylistBuilder::finalize** (`c27d4c9`).
   `Manifest` gains `ended: bool`. When true, the renderer
   appends `#EXT-X-ENDLIST` and suppresses `#EXT-X-PRELOAD-HINT`
   (the two are mutually exclusive). `PlaylistBuilder::finalize()`
   closes the pending segment, clears the preload hint, sets
   `ended=true`. Idempotent (calling twice is harmless).
   `HlsServer::finalize()` is the async wrapper that coalesces
   the last segment's bytes and purges evicted URIs. After
   finalize the retained window becomes a VOD surface.

2. **`--hls-dvr-window` CLI flag** (`e6db779`). Adds
   `--hls-dvr-window <secs>` (env `LVQR_HLS_DVR_WINDOW`, default
   120) to `lvqr-cli serve`. Translates to
   `max_segments = dvr_secs / target_duration_secs` at
   construction time. 0 means unbounded. Replaces the session-34
   hardcoded `max_segments = Some(60)` with operator-tunable
   depth.

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo test -p lvqr-cli
--tests` green (12 tests including rtmp_hls_e2e and
rtmp_dash_e2e). `cargo test --workspace` reports **416 tests
passed** (+2 finalize unit tests from session 37 Commit A), 1
ignored doctest.

### Session 39 entry point

The LL-HLS surface now covers every mandatory RFC 8216bis tag:
`EXT-X-PROGRAM-DATE-TIME` (session 37), `EXT-X-ENDLIST` (session
38), `CAN-SKIP-UNTIL` + delta playlists (session 31), sliding
window eviction (session 34), and per-broadcast PDT anchoring
(session 37). The DVR window is operator-tunable via
`--hls-dvr-window`.

Primary scope options for session 39:

* **Wire finalize on broadcaster disconnect**. Session 38 added
  the `HlsServer::finalize()` method but no caller invokes it
  yet. When an RTMP publisher disconnects (or a WHIP session
  ends), the HLS bridge should call finalize so the trailing
  `#EXT-X-ENDLIST` actually appears. Trace the disconnect path
  from `lvqr-ingest` -> fragment observer -> HLS bridge to find
  the right injection point.
* **Byte-range partials for LL-HLS**. Cuts per-partial HTTP
  overhead.
* **Self-hosted macOS runner with Apple HTTP Live Streaming
  Tools** for `mediastreamvalidator` primary signal.
* **Criterion benches for `lvqr-cmaf` and `lvqr-ingest`**.



## Session 37 close (2026-04-16)

One commit on top of session 36's `2cb5b0e`. Session 37 shipped
a spec-conformance fix rather than more test infra: the LL-HLS
renderer advertises `CAN-SKIP-UNTIL=12.000` in
`EXT-X-SERVER-CONTROL` but never emitted
`#EXT-X-PROGRAM-DATE-TIME`, which RFC 8216bis requires when
skip is advertised.

### Session 37 commit list

1. **EXT-X-PROGRAM-DATE-TIME per segment** (`293ebe2`).
   `PlaylistBuilderConfig` gains `program_date_time_base:
   Option<u64>` (millis since UNIX epoch for the wall-clock
   time of DTS=0). When set, `close_pending_segment` computes
   each segment's PDT by adding cumulative segment-duration
   offset to the base and stores it on
   `Segment.program_date_time_millis`. The renderer emits
   `#EXT-X-PROGRAM-DATE-TIME:<iso8601>` before each non-skipped
   segment. The ISO 8601 formatter uses the Hinnant
   `civil_from_days` algorithm inline -- no chrono/time
   dependency. `MultiHlsServer` stamps
   `program_date_time_base = SystemTime::now()` per-broadcast
   at `ensure_video` / `ensure_audio` creation time so every
   broadcast anchors its PDT independently. Audio renditions
   inherit the same base via `audio_config_from`.

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo test --workspace`
reports **414 tests passed** (+4 from session 36), 1 ignored
doctest. Four new unit tests: `render_emits_program_date_time_per_segment`,
`program_date_time_omitted_when_base_is_none`,
`format_program_date_time_known_epoch`,
`format_program_date_time_unix_epoch`.

### Session 38 entry point

Primary scope options, in priority order:

* **LL-HLS VOD / DVR archive surface**. Now that session 34's
  sliding window drops bytes and session 37's PDT gives every
  segment a wall-clock anchor, a DVR surface behind
  `#EXT-X-PLAYLIST-TYPE:EVENT` is the natural next step.
  Design question: `lvqr-hls` archive cache vs `lvqr-record`
  archive index.
* **Byte-range partials for LL-HLS**. Cuts per-partial HTTP
  overhead. 3-4 commit scope.
* **Self-hosted macOS runner with Apple HTTP Live Streaming
  Tools** for `mediastreamvalidator` primary signal.
* **`lvqr-record` fuzz target** on the redb write path (last
  plausible fuzz slot: `lvqr-fragment` / `lvqr-moq` are
  data-model / re-export crates with no parser surface).



## Session 36 close (2026-04-16)

One commit on top of session 35's `4c0eae6`. Session 36 focused
on Tier 1 test infra after sessions 34 and 35 closed the LL-HLS
feature gap and the `lvqr-hls` / `lvqr-cmaf` fuzz slots.

### Session 36 commit list

1. **Criterion bench for PlaylistBuilder** (`32cd6dc`). First
   criterion bench in the workspace, closing the "zero criterion
   benches" gap the session-30 maturity audit listed. Three bench
   groups: `push_partial` (hot path, ~200ns), `push_segment_boundary`
   (close + eviction, unbounded ~0.9us at 60 segs, capped60
   ~1.9us at 240 primed), `render` (Manifest::render, ~46us at
   the production cap of 60 segments). The 60-segment production
   cap means render stays under 50us per manifest in the steady
   state, which is well within the blocking-reload polling budget.

### CI verification

CI run `24486545369` against commit `4c0eae6` (session 35 head)
confirmed all three workflows pass on `origin/main` with the
session-34 eviction path live:

* **CI**: 5m2s, success.
* **LL-HLS Conformance**: 3m10s, success. The ffmpeg client-pull
  path ran against the `max_segments=Some(60)` production config.
  No 404 on any segment URI. The eviction cap did not fire in the
  10-segment CI fixture, which is correct: with a 60-segment
  window and a 10-segment fixture, eviction never triggers.
* **Test Contract**: 7s, success.

### Session 37 entry point

Primary scope options, in priority order:

* **LL-HLS VOD / DVR archive surface**. Session 34's eviction
  drops bytes for good. A DVR surface behind
  `#EXT-X-PLAYLIST-TYPE:EVENT` retention policy is a real gap
  now that the live window is bounded. Design question still
  open: `lvqr-hls` archive cache vs `lvqr-record` archive
  index.
* **Byte-range partials for LL-HLS**. Cuts per-partial HTTP
  overhead. Requires Range header support in the router,
  Part.byterange data model, renderer emission, cache layout
  change. 3-4 commit scope.
* **Self-hosted macOS runner with Apple HTTP Live Streaming
  Tools**. `mediastreamvalidator` from soft-skip to primary
  signal.
* **Close remaining contract slots**. `lvqr-fragment` and
  `lvqr-moq` are data-model / re-export crates with no real
  parser surface -- adding fuzz targets there would be
  theatrical. `lvqr-whip` / `lvqr-whep` conformance slots
  need real WebRTC interop vectors. `lvqr-record` fuzz on the
  redb write path is plausible.



## Session 35 close (2026-04-15)

Two concrete commits on top of session 34's `f9b0314`. The
session closed two of the nine missing 5-artifact contract
slots the session-30 maturity audit enumerated. No feature work;
this was a disciplined Tier 1 test-infra push to build up the
audit backlog the critical-path plan puts before v1.0 M1.

### Session 35 commit list

1. **lvqr-hls PlaylistBuilder fuzz skeleton** (`3466dcd`).
   `crates/lvqr-hls/fuzz/` mirrors the session-33 `lvqr-dash`
   fuzz layout. The `playlist_builder` target interprets the
   fuzzer input as a sequence of `(duration, kind)` tuples and
   feeds them into a `PlaylistBuilder` configured with
   `max_segments=Some(4)` so any non-trivial input exercises
   the session-34 sliding-window eviction path. DTS is forced
   strictly monotonic by the target itself and durations are
   forced non-zero; any error return from `push` is still
   tolerated. After every successful push the target renders
   the manifest and asserts the output begins with `#EXTM3U`
   and contains exactly one header. At end-of-input it
   force-closes the pending segment, drains the session-34
   eviction buffer, and re-renders under the same invariants so
   the end-of-stream coalesce + eviction paths are covered.
   Excluded from the workspace like every other libfuzzer-sys
   consumer; run with `cargo +nightly fuzz run playlist_builder`.

2. **lvqr-cmaf codec-string-detector fuzz skeleton** (`bfc02dd`).
   `crates/lvqr-cmaf/fuzz/` with a `detect_codec_strings` target
   that drives both `detect_video_codec_string` and
   `detect_audio_codec_string` with arbitrary bytes. These
   functions parse a publisher-supplied fMP4 init segment and
   extract an RFC 6381 codec string that feeds into the HLS
   master playlist's `CODECS="..."` attribute and the DASH
   Representation's `codecs` attribute. Ingest does not
   schema-validate the init segment before calling them, so a
   crash inside the parser is a publisher-triggered denial of
   service against the egress surface. The target asserts
   neither detector panics; libfuzzer's OOM sentinel enforces
   the implicit no-unbounded-allocation invariant. Run with
   `cargo +nightly fuzz run detect_codec_strings`.

### Contract slot accounting

`scripts/check_test_contract.sh` now reports **7 missing slot(s)**
(down from 9 at session 34 close). Both `lvqr-cmaf` and
`lvqr-hls` satisfy every slot. Remaining gaps, by crate:

* `lvqr-record`: fuzz.
* `lvqr-moq`: fuzz, conformance.
* `lvqr-fragment`: fuzz, conformance.
* `lvqr-whip`: conformance.
* `lvqr-whep`: conformance.

The remaining fuzz slots are all protocol-layer parsers
(`lvqr-fragment` fMP4 boxes, `lvqr-moq` wire protocol,
`lvqr-record` disk recorder); adding them is the same kind of
mechanical libfuzzer-sys wiring this session used, and each
one should stay its own commit so the reviewer can evaluate
the target choice. The remaining conformance slots are the
harder work: `lvqr-whip` / `lvqr-whep` want a WebRTC interop
golden or a Chrome/Firefox spec vector, and `lvqr-moq` /
`lvqr-fragment` need wire-protocol golden fixtures against the
IETF draft.

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo test --workspace`
remains at 410 passing / 1 ignored / 0 failing across 88 test
binaries (no new tests this session; fuzz targets are excluded
from the workspace build and run only under nightly). Every
session-35 commit is authored by `Moheeb Zara
<hackbuildvideo@gmail.com>` alone.

### Not-yet-pushed

Session 34 + 35 together land **six commits** on local `main`
that are not yet on `origin/main`:

    87f1b41 Tier 2.5: LL-HLS PlaylistBuilder sliding-window eviction
    54cd1bb Tier 2.5: LL-HLS server-side cache purge on sliding-window eviction
    99b514d Tier 2.5: cap lvqr-cli production HLS sliding window at 60 segments
    f9b0314 docs: session 34 close + LL-HLS sliding-window eviction
    3466dcd Tier 1: cargo-fuzz skeleton for lvqr-hls PlaylistBuilder
    bfc02dd Tier 1: cargo-fuzz skeleton for lvqr-cmaf detect_*_codec_string

Next session must `git push origin main` before starting new
work so the `hls-conformance.yml` required check runs against
the session-34 eviction path.

### Session 36 entry point

Primary scope options, in priority order:

* **Close more contract fuzz slots**. `lvqr-fragment` fMP4 box
  parser is the highest-value target because it sits on the
  publisher-reachable parse path and has a well-defined byte
  surface. `lvqr-moq` wire parser is second. `lvqr-record`
  disk recorder is third. Each one is its own commit.
* **LL-HLS VOD / DVR archive surface**. Design decision
  pending: `lvqr-hls` archive cache vs `lvqr-record` archive
  index. Now that session 34's sliding window drops bytes for
  good, a rewind-capable surface is a real gap.
* **Byte-range partials** for LL-HLS (cuts per-partial HTTP
  overhead; builds on the session-33 coalesce path).
* **Self-hosted macOS runner with Apple HTTP Live Streaming
  Tools** so `hls-conformance.yml` flips `mediastreamvalidator`
  from soft-skip to primary signal.



## Session 34 close (2026-04-15)

Three concrete commits on top of session 33's `8b5c3c9`. The
session landed the LL-HLS sliding-window eviction called out as
primary scope in the session 33 entry point: the unbounded growth
in `PlaylistBuilder.manifest.segments` is now capped, the
server-side byte cache purges in lock-step, and the lvqr-cli
production path runs with a 60-segment window (~120 s of history
at the default 2 s target duration).

### Session 34 commit list

1. **LL-HLS PlaylistBuilder sliding-window eviction** (`87f1b41`).
   Adds `max_segments: Option<usize>` to `PlaylistBuilderConfig`
   (default `None` for backwards compat), an `evicted_uris` buffer
   on `PlaylistBuilder`, and a `drain_evicted_uris` method.
   `close_pending_segment` drains overflow from the front of
   `manifest.segments` and pushes both the dropped segment URI
   and each of its constituent part URIs into the buffer. The
   audio rendition config derivation in `server.rs` propagates
   `max_segments` so audio playlists slide in lock-step with
   video. Two unit tests in `manifest.rs`: one asserts the
   retained window, the advanced sequence, the drained URI list
   (including the off-by-one-looking part indices the builder's
   `part_index` reset behaviour produces), and the advanced
   `#EXT-X-MEDIA-SEQUENCE`; the second asserts the default config
   stays unbounded.

2. **Server-side cache purge on eviction** (`54cd1bb`).
   `push_chunk_bytes` and `close_pending_segment` in `HlsServer`
   drain `evicted_uris` from the builder and call `cache.remove`
   on each entry after the builder lock releases. Also switches
   `collect_coalesce_work` from index-based to sequence-based
   detection: eviction may shrink `manifest.segments` from the
   front inside the same push, which would break a positional
   `[prev_seg_len..]` walk. Capturing the prior tail sequence and
   filtering for strictly-greater survivors keeps the session-33
   coalesce path correct under the new window. Integration test
   `sliding_window_purges_cache_and_advances_media_sequence` in
   `integration_server.rs` builds a server with `max_segments=3`,
   pushes six 10-part segments plus a seventh Segment-kind close,
   and asserts `EXT-X-MEDIA-SEQUENCE:3`, the playlist no longer
   references `seg-0.m4s`, `GET /seg-0.m4s` returns 404 (cache
   purged), and `GET /seg-3.m4s` returns non-empty coalesced
   bytes. The existing `closed_segment_uri_serves_coalesced_bytes`
   regression test still passes.

3. **lvqr-cli production cap at 60 segments** (`99b514d`). Wires
   the `MultiHlsServer::new` call in the `lvqr-cli` serve path to
   `PlaylistBuilderConfig { max_segments: Some(60), .. }`. Sixty
   segments at 2 s target duration is 120 s of history, roughly
   matching the DVR scrub depth the archive path already
   supports. `rtmp_hls_e2e` and `rtmp_dash_e2e` pass under the new
   cap. Tests under `crates/lvqr-hls` stay on
   `PlaylistBuilderConfig::default()` directly, which keeps
   `max_segments=None` and continues to exercise the unbounded
   path.

### Verification

`cargo fmt --all --check` clean. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo test --workspace`
reports 88 `test result: ok` lines, **410 tests passed**, 1
ignored doctest (up from 408 at session 33 close: +2 unit tests
in `manifest.rs` and +1 integration test in
`integration_server.rs`, all exercising the new eviction path).
`git log -1 --format='%an <%ae>'` after each commit is
`Moheeb Zara <hackbuildvideo@gmail.com>` alone.

### Session 35 entry point

Primary scope options in priority order:

* **LL-HLS VOD / DVR windows**. The sliding-window eviction now
  drops bytes for good. A DVR surface wants a secondary "archive"
  cache behind an explicit `#EXT-X-PLAYLIST-TYPE:EVENT` retention
  policy so Rewind-style clients can pull N minutes back without
  pinning the live window. Design question: does the archive
  cache belong inside `lvqr-hls` or at the `lvqr-record` layer
  that already owns the archive index path `rtmp_archive_e2e.rs`
  exercises?

* **Byte-range partials**. Session 34 keeps each partial as its
  own `part-<msn>-<idx>.m4s` URI. Apple LL-HLS spec also allows
  a single closed-segment file with `#EXT-X-BYTERANGE` partials
  inside it; this cuts the per-partial HTTP overhead at the cost
  of a more elaborate cache-coalesce path. Worth sketching now
  that the coalesce path exists.

* **Self-hosted macOS runner for `mediastreamvalidator`**. The
  `hls-conformance.yml` workflow still soft-skips validator
  because `macos-latest` images do not ship Apple HTTP Live
  Streaming Tools. Promoting the ffmpeg client-pull path to a
  self-hosted macOS runner with the tools pre-installed would
  flip the validator from advisory to primary and close the
  biggest gap called out in the session-30 maturity audit.

Known gaps carried over from session 33 that session 34 did not
touch: `lvqr-dash` still has no external conformance slot (the
golden fixtures and libfuzzer skeleton closed the other three
slots), and the Tier 2 crates below `lvqr-hls` / `lvqr-dash`
still have the 11 missing contract slots the session-30 audit
enumerated.



## Session 33 close (2026-04-15)

Eight concrete commits on top of the session-32 head. The session
fixed the LL-HLS closed-segment-bytes cache bug the first
`hls-conformance.yml` run surfaced, closed every open slot of
the 5-artifact contract for `lvqr-dash`, hardened the MPD
renderer's XML attribute escape path, verified the fix against
a clean CI artifact, and flipped the workflow from advisory to
required.

### Verification

Run 24480404358 (commit `6be52e2`, on a fresh macos-latest
runner) produced a clean `hls-conformance-output` artifact:

* `ffmpeg-pull.log` is empty -- no `audio-seg-0.m4s` 404.
* `ffmpeg-pull.mp4` was created at 5 s length (ffmpeg was given
  `-t 5` for the client-side pull).
* `ffprobe.log` decodes 5.001 s of H.264 Constrained Baseline at
  640x360 from the video stream and 4.922 s of AAC-LC 44.1 kHz
  stereo from the audio stream. Both streams resolved through
  the full master -> media playlist -> init -> segment pipeline,
  which means the session-33 closed-segment coalesce in
  `HlsServer::push_chunk_bytes` is feeding the client correctly.
* `master.m3u8` / `playlist.m3u8` still carry the session-31
  spec surface unchanged (EXT-X-INDEPENDENT-SEGMENTS,
  EXT-X-SERVER-CONTROL with CAN-SKIP-UNTIL, EXT-X-PART-INF,
  EXT-X-PRELOAD-HINT, EXT-X-RENDITION-REPORT).

`mediastreamvalidator` itself still soft-skips -- the
macos-latest runner image does not ship Apple HTTP Live
Streaming Tools, so the ffmpeg client-pull is the only
compliance read. A self-hosted macOS runner with HLS Tools
pre-installed is the only path to a true validator-backed gate
and is a future-session item.

### Session 33 commit list

Two concrete commits on top of the session-32 head:

1. **LL-HLS closed-segment cache coalesce** (`76fc6a0`). Fixes
   the `audio-seg-0.m4s` 404 the first `hls-conformance.yml` CI
   run surfaced via the ffmpeg client-side compliance pass.
   `HlsServer::push_chunk_bytes` and `HlsServer::close_pending_segment`
   now snapshot `manifest.segments.len()` before mutating the
   builder, and after mutation any newly-closed segment's
   constituent part URIs are looked up in the cache and
   concatenated (via `BytesMut`) into a single blob that is
   inserted under `seg.uri`. The coalesce runs under the cache
   write lock after the builder write lock is released so the
   two locks are never held simultaneously. Audio renditions
   get the same fix for free because `seg.uri` already carries
   the `audio-<prefix>` rewrite.

   A new integration test in `crates/lvqr-hls/tests/integration_server.rs`
   (`closed_segment_uri_serves_coalesced_bytes`) pushes 10
   partials plus one Segment-kind chunk, fetches `/seg-0.m4s`,
   and asserts the response body is the byte-exact concatenation
   of the 10 pushed part bodies. The existing
   `playlist_init_and_segment_round_trip` test continues to
   exercise the part-URI resolution path unchanged.

2. **lvqr-dash proptest harness** (`810b334`). Closes the
   proptest slot of the 5-artifact contract for `lvqr-dash`,
   leaving fuzz + external conformance as the still-open slots.
   `crates/lvqr-dash/tests/proptest_mpd.rs` covers five
   invariants at 200 cases each: render never panics on
   well-formed input, rendered body starts/ends with XML
   markers, `<AdaptationSet ` count matches input, `render_mpd`
   free function is byte-equal to `Mpd::render`, and the
   `type="dynamic"` / `"static"` attribute matches `MpdType`.
   `scripts/check_test_contract.sh` now reports `ok proptest`
   for `lvqr-dash`; total missing slots falls from 12 to 11.

**Tests**: 88 test binaries (+2 from session 32), **402 tests**
(+6 from session 32: +1 regression test for the HLS fix and +5
proptest cases for lvqr-dash), 0 failures, 1 ignored doctest.

### Session 33 commits landed in session

On top of the two originally-planned commits (`76fc6a0`,
`810b334`), session 33 also landed:

3. `2ae8caf` docs: session 33 close (self-describing)
4. `6be52e2` Tier 2.6: golden conformance + fuzz skeleton for
   `lvqr-dash`. Three byte-exact golden MPD fixtures plus a
   libfuzzer skeleton closed the conformance + fuzz slots of
   the 5-artifact contract.
5. `ca1ceda` Tier 2.6: XML attribute escaping in `lvqr-dash`
   MPD renderer. New `mpd::esc` helper wraps every
   user-controlled attribute value with Cow-based entity
   rewriting (`&` `<` `>` `"` `'`). Safe inputs hit the
   borrowed fast path so production allocates nothing extra;
   hostile codec strings crafted to inject via a closing `"`
   are escaped. Three unit tests cover `esc` itself plus a
   hostile-codecs-string round trip; the proptest harness now
   generates codecs from `\\PC*` so every invariant runs
   against 200 cases of adversarial string content; the fuzz
   target asserts the previously-deferred one-`<MPD`-root
   invariant on every iteration.
6. `1956ee6` ci: flip `hls-conformance.yml` to required. See
   the Verification block above.

### Session 34 entry point

Primary scope: LL-HLS sliding-window eviction for
`PlaylistBuilder.segments`. Confirmed from `manifest.rs:461`
that the builder's `close_pending_segment` calls
`self.manifest.segments.push(seg)` with no eviction path,
and no caller truncates the vector later. Both the rendered
playlist and the `HlsState` cache (including the session-33
coalesced closed-segment bytes) therefore grow unboundedly
on long broadcasts. A 24 h stream at the default 2 s target
duration produces ~43 200 segments, each holding ~10 parts;
memory use scales linearly without bound.

Planned commits:

**Commit A** -- `manifest.rs`: add
`max_segments: Option<usize>` to `PlaylistBuilderConfig`
(default `None` for backwards compat), add an
`evicted_uris: Vec<String>` buffer field to `PlaylistBuilder`,
and a `drain_evicted_uris(&mut self) -> Vec<String>` method
that empties it. Modify `close_pending_segment` so that after
the `push(seg)` call, when `config.max_segments.is_some_and(|m| segments.len() > m)`,
drain the overflow from the front of `segments` and push
every dropped `segment.uri` plus every `part.uri` inside that
segment into `evicted_uris`. Unit tests in the `tests` module
at the bottom of `manifest.rs`: construct a builder with
`max_segments = Some(3)`, push 6 Segment-kind chunks,
assert `segments.len() == 3`, assert `segments.first().sequence == 3`,
assert `drain_evicted_uris()` returned the expected URIs
for the three dropped segments (`seg-0.m4s`, `seg-1.m4s`,
`seg-2.m4s`) plus each drained segment's parts, and assert
`EXT-X-MEDIA-SEQUENCE:3` in the rendered manifest.

**Commit B** -- `server.rs`: inside `push_chunk_bytes` and
`close_pending_segment`, after the existing `builder.push` /
`builder.close_pending_segment` call (and before the builder
lock is released), also collect `drain_evicted_uris()` into
the same work stash that already carries the coalesce list.
After dropping the builder lock, under the cache write lock,
call `cache.remove(&uri)` for every drained URI. Integration
test in `tests/integration_server.rs`: construct an
`HlsServer` with `max_segments = Some(3)`, push 6 full
segments through `push_chunk_bytes`, assert that
`GET /seg-0.m4s` returns 404 (coalesced bytes evicted from
cache), `GET /seg-3.m4s` returns non-empty (latest retained
segment resolves), and the rendered playlist shows
`EXT-X-MEDIA-SEQUENCE:3` via a `GET /playlist.m3u8`.

**Commit C** -- `lvqr-cli/src/lib.rs`: flip the
`MultiHlsServer::new(PlaylistBuilderConfig::default())` call
to a configured one with `max_segments = Some(60)` (120 s
history at 2 s target duration, roughly matching the DVR
scrub depth the archive path already supports). No test
update needed; existing tests use `PlaylistBuilderConfig::default()`
directly and stay at `None`.

**Commit D** -- docs: session 34 close, HANDOFF, memory.

Secondary scopes for session 34 if budget allows:
LL-HLS VOD windows for DVR scrub; byte-range partials;
promotion of the ffmpeg client-pull path to a self-hosted
macOS runner with Apple HTTP Live Streaming Tools so
`mediastreamvalidator` becomes the primary signal
(currently the workflow soft-skips because macos-latest
runner images do not ship the tool, and the ffmpeg pull is
the only compliance read).

## Session 32 close (2026-04-15)
push of the session-26-through-31 backlog, Tier 2.6 lvqr-dash
DashServer + MultiDashServer + DashFragmentBridge + --dash-port
wiring + integration router tests + RTMP->DASH E2E, 5-artifact
contract promotion for lvqr-dash)

**Branch head**: session-32 close. Session 32 was the first
session that pushed to origin/main: the session-26-through-31
backlog (29 commits) landed at the top of the session,
immediately followed by the Tier 2.6 finishing commits. The
`.github/workflows/hls-conformance.yml` workflow ran for the
first time and succeeded on macos-15-arm64; the Apple HTTP
Live Streaming Tools (`mediastreamvalidator`) are not
present on that runner image, so the workflow's soft-skip
path fires and the ffmpeg client-pull is the only compliance
read for now. That pull surfaced one real finding: the LL-HLS
audio sub-playlist exposes closed-segment URIs
(`audio-seg-0.m4s` and siblings) that return 404 because
`HlsServer::push_chunk_bytes` only caches partial URIs rather
than the coalesced closed-segment URI the manifest renderer
lists under `#EXTINF`. See the session-33 entry point for
the fix plan.
**Tests**: `cargo test --workspace` green under the default
feature set: **86 test binaries, 395 individual tests passing**,
0 failures, 1 ignored doctest. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo fmt --all --check`
clean. Session-31 delta over session-30's `432290c` baseline:
+19 tests, +2 test binaries. Breakdown: 1 lvqr-whip
recording-observer unit test proving `WhipMoqBridge` fires
`FragmentObserver::on_init` + `on_fragment` for the `1.mp4`
Opus track; 2 lvqr-hls manifest tests covering
`EXT-X-PRELOAD-HINT` population through a segment boundary
and across the audio-prefix config; 2 lvqr-hls integration
tests covering `EXT-X-RENDITION-REPORT` sibling-rendition
emission in the multi-broadcast router; 4 lvqr-hls manifest
tests covering `CAN-SKIP-UNTIL` + `EXT-X-SKIP` delta-playlist
decision logic across the spec floors; 2 lvqr-hls integration
tests covering `_HLS_skip=YES` directive routing through the
axum router; 2 lvqr-hls proptest invariants covering delta
playlist spec floors and render-matches-skip-decision
isomorphism; 6 lvqr-dash MPD unit tests covering the happy
path, audio AdaptationSet emission, empty-state rejections,
and the VOD static-type variant.

## Session 32 close (2026-04-15)

Six concrete commits, each in its own scope:

1. **Push session 26-31 backlog** (force-of-habit first act).
   29 commits that had been queued on the working branch
   since before session 26 finally landed on `origin/main`
   in one `git push`. The session-31 addition of the Apple
   mediastreamvalidator workflow ran for the first time as
   a consequence, and its output fed the triage below.

2. **Tier 2.6 DashServer + MultiDashServer axum router**
   (`33fcc44`). First half of the Tier 2.6 finishing
   sequence. `crates/lvqr-dash/src/server.rs` lands a
   per-broadcast state machine that mirrors the lvqr-hls
   shape: `DashServer` owns video + audio init bytes + a
   HashMap-keyed segment cache + a codec-string cache
   parsed through `lvqr_cmaf::detect_*_codec_string` at
   init time, and projects the state onto four routes:
   `/manifest.mpd` (application/dash+xml), `/init-video.m4s`,
   `/init-audio.m4s`, and `/{*uri}` dispatching
   `seg-video-<n>.m4s` / `seg-audio-<n>.m4s` by numeric
   parse. Pre-init codec fallback is the conservative
   `avc1.640020` / `mp4a.40.2` pair the LL-HLS master
   playlist already uses. `MultiDashServer` fans per-broadcast
   `DashServer` instances under `/dash/{broadcast}/...` via
   the same trailing-slash split the multi-HLS router uses
   for `live/test`-style nested broadcast names. Seven new
   unit tests bring the crate to 13.

3. **Tier 2.6 DashFragmentBridge observer** (`6e70da3`).
   `crates/lvqr-dash/src/bridge.rs` lands a
   `FragmentObserver` impl that feeds the `MultiDashServer`.
   Unlike `HlsFragmentBridge`, there is no `CmafPolicyState`
   walk: the DASH live profile addresses whole segments via
   SegmentTemplate `$Number$` URIs, so the bridge just
   stamps a monotonic per-track counter onto every observed
   fragment and pushes the payload bytes under that number.
   The counter resets on every `on_init` for the same
   broadcast so a republish (RTMP reconnect, WHIP session
   rollover) restarts segment numbering at 1. Four unit
   tests bring the crate to 17.

4. **Tier 2.6 wire --dash-port into lvqr-cli serve path**
   (`eace85c`). `ServeArgs` gains `--dash-port` (default 0,
   env `LVQR_DASH_PORT`), `ServeConfig` gains
   `dash_addr: Option<SocketAddr>`, and `start()` constructs
   a `MultiDashServer` + `DashFragmentBridge` pair when
   the field is `Some`, appends the bridge into the shared
   `fragment_observers` tee so RTMP and WHIP publishers
   both feed it, pre-binds the DASH TCP listener, and spawns
   a dedicated axum server. `ServerHandle` grows
   `dash_addr()` + `dash_url()` accessors and
   `TestServerConfig` grows `with_dash()` plus
   `TestServer::dash_addr()` / `dash_url()` for the
   follow-on E2E test.

5. **Tier 2.6 integration + E2E tests for lvqr-dash**
   (`0ebf516`).
   `crates/lvqr-dash/tests/integration_router.rs` drives the
   axum router surface through `tower::ServiceExt::oneshot`
   with four cases: single-broadcast av round trip,
   404-before-any-push, 404-on-unknown-segment-number, and
   multi-broadcast per-broadcast dispatch (including the
   404 on unknown broadcast).
   `crates/lvqr-cli/tests/rtmp_dash_e2e.rs` is the
   TCP-loopback E2E counterpart: starts a `TestServer` with
   `with_dash()`, publishes two keyframes via `rml_rtmp`
   past the segmenter's 2 s boundary, then issues raw
   HTTP/1.1 GETs to the DASH surface and asserts the
   manifest renders with `type="dynamic"` plus the
   `SegmentTemplate` URI, `init-video.m4s` starts with a
   real `ftyp` box, `seg-video-1.m4s` starts with a `moof`
   box, and an unknown broadcast returns 404. Brings the
   workspace to 86 test binaries, 395 tests, still zero
   failures and the one unchanged ignored doctest.

6. **5-artifact contract promotion + docs** (this commit).
   `scripts/check_test_contract.sh` promotes `lvqr-dash` into
   the active `IN_SCOPE` list. The crate passes the
   integration + E2E slots and surfaces warnings for the
   still-open proptest, fuzz, and conformance slots (future
   session). `tests/CONTRACT.md` grows a `lvqr-dash` row
   matching the existing lvqr-hls structure. README +
   HANDOFF + memory snapshots update to reflect the Tier 2.6
   DONE flip.

### Mediastreamvalidator triage (first CI run)

The session-31 `hls-conformance.yml` workflow ran for the
first time on the session-32 push. The runner image
(`macos-15-arm64`) does not ship Apple HTTP Live Streaming
Tools, so the workflow's `Locate mediastreamvalidator` step
drops into the `mediastreamvalidator unavailable (soft gap)`
branch and the ffmpeg client-pull is the only compliance
signal. `/tmp/msv-out` uploaded as `hls-conformance-output`:

* `master.m3u8`, `playlist.m3u8` -- captured cleanly. The
  video playlist has 601 `#EXT-X-PART` lines, correct
  `#EXT-X-MEDIA-SEQUENCE:0`, the session-31 spec tags
  (`EXT-X-INDEPENDENT-SEGMENTS`, `EXT-X-SERVER-CONTROL` with
  `CAN-SKIP-UNTIL=12.000`, `EXT-X-PART-INF`,
  `EXT-X-PRELOAD-HINT`, `EXT-X-RENDITION-REPORT`).
* `ffmpeg-pull.log` surfaces one real finding:
  `http://127.0.0.1:8888/hls/live/test/audio-seg-0.m4s`
  returns 404. Segments 6/7/8 of playlist 0 also 404. The
  audio sub-playlist exposes closed-segment URIs under
  `#EXTINF` lines, but `HlsServer::push_chunk_bytes` only
  caches the *partial* URIs on push -- the coalesced
  closed-segment URIs never get bytes stored in the cache.
  This is a pre-existing LL-HLS bug not introduced in this
  session; normal LL-HLS clients that fetch partials
  directly do not hit it, but plain HLS clients (ffmpeg,
  Safari fallback) do. See the session-33 entry point.
* `ffprobe.log` shows the ffmpeg pull bailed because of the
  audio 404 before getting to real conformance reads, so
  the only compliance signal this run produces is the
  session-31 synthetic-stub variant still running under
  `cargo test --workspace`.

Escalation path for hard-required promotion: either a
self-hosted macOS runner with Apple HTTP Live Streaming
Tools pre-installed, or mirror the `.pkg` into private
storage and install it as a workflow step. Either lands
after the session-33 closed-segment-bytes fix, so the
ffmpeg-client read comes back green before the workflow is
flipped to hard-required.

### Maturity audit deltas

* **Tier 2.6 `lvqr-dash`**: was PARTIAL ~25% at session-31
  close, now DONE. Server + bridge + CLI wiring +
  integration + E2E all shipped. Proptest / fuzz /
  conformance slots remain open and surface in
  `check_test_contract.sh` warnings.
* **Tier 1 "Apple mediastreamvalidator in CI"**: still
  PARTIAL. The workflow runs on every push but cannot
  actually run `mediastreamvalidator` because the runner
  image lacks it. Promotion to required gates on the
  self-hosted escalation, which itself gates on the
  closed-segment-bytes fix so the ffmpeg client-pull is
  clean.
* **Tier 2.5 LL-HLS**: still PARTIAL ~95% (spec surface
  unchanged). Session 33 closes the closed-segment-bytes
  cache bug the first CI run surfaced.

### Session 33 entry point

Primary scope: fix the LL-HLS closed-segment-bytes bug the
first `hls-conformance.yml` run surfaced. The audio
sub-playlist lists `audio-seg-<n>.m4s` URIs under `#EXTINF`
lines, but the cache only holds `audio-part-<n>-<m>.m4s`
URIs because `HlsServer::push_chunk_bytes` stores bytes
keyed on the trailing preliminary_part URI only. The fix
has two reasonable shapes:

1. On `#EXTINF` segment close, coalesce the just-closed
   segment's partials into a single CMAF blob and insert
   it into the cache under the closed-segment URI. The
   coalesce is just `bytes::Bytes::concat` over the
   partials' `Bytes` values; the partials are already
   aligned to a `moof` boundary.
2. Or, resolve closed-segment URIs on demand in the router
   by walking the manifest and concatenating the partials
   for the requested `seq`.

(1) is simpler but bloats the cache (every closed segment
is stored twice). (2) is trickier but keeps the cache
bounded. Session-33 commit A picks (1) and benchmarks the
cache footprint; commit B is the axum route handler update
that serves the cached bytes.

Secondary scopes for session 33: the proptest + fuzz +
external conformance slots for `lvqr-dash` that
`check_test_contract.sh` is flagging, plus a DASH-IF
conformance reader once the self-hosted runner lands.

## Session 31 close (2026-04-15)

Two concrete deliverables, each in its own commit, plus this docs pass.

1. **WHIP Opus -> LL-HLS fragment observer** (`d4378bd`). The
   last loose thread in the WHIP audio story. `WhipMoqBridge`
   already fired the raw-sample observer for Opus frames so WHEP
   subscribers got same-codec passthrough, but it did not fire
   the fragment observer, so LL-HLS and the archive tee never
   saw the audio track. `ensure_audio_initialized` now calls
   `FragmentObserver::on_init` for `1.mp4` at 48 kHz with the
   Opus CMAF init bytes, and `push_audio_sample` calls
   `on_fragment` after a successful sink push. Both follow the
   same release-the-DashMap-entry-before-observing reentrancy
   pattern the video path already uses. `HlsFragmentBridge` was
   already codec-agnostic above the init segment
   (`MultiHlsServer::ensure_audio` takes timescale,
   `HlsServer::push_init` runs the init bytes through
   `detect_audio_codec_string`), so no HLS-side changes were
   needed.

   Verified by a new bridge unit test that wires a recording
   `FragmentObserver` into the bridge, pushes one AVC keyframe
   plus two Opus frames, and asserts (a) the audio `on_init`
   fires at 48 kHz with non-empty CMAF `ftyp`-prefixed bytes
   that `detect_audio_codec_string` recognises as `"opus"`,
   and (b) two `1.mp4` fragments fire in DTS order with the
   expected 960-tick durations. The end-to-end chain from WHIP
   input through the LL-HLS audio rendition master playlist is
   covered transitively by session 30's
   `master_playlist_reports_opus_codec_when_audio_rendition_has_opus_init`
   in `lvqr-hls/tests/integration_master.rs`.

2. **LL-HLS conformance workflow** (`e2698f9`). New
   `.github/workflows/hls-conformance.yml` runs on every PR and
   `main` push against `macos-latest`:

   * Builds `lvqr-cli` in release mode, starts `lvqr serve` in
     the background on canonical ports (RTMP 1935, HLS 8888).
   * Pushes a deterministic 20s 640x360@30 H.264 baseline + AAC
     44.1 kHz stereo fixture to `rtmp://127.0.0.1:1935/live/test`
     via ffmpeg.
   * Captures `master.m3u8` and `playlist.m3u8` to the run log.
   * Runs Apple `mediastreamvalidator` against the master
     playlist and tees its output into an uploaded build
     artifact. `continue-on-error` stays on until the baseline
     clears: real findings are expected. Flip to required once
     the baseline is green.
   * Always runs a second-signal ffmpeg-as-client pull + ffprobe
     on the same playlist so every run captures an independent
     compliance read even when `mediastreamvalidator` is
     unavailable on the runner image. Apple's HTTP Live Streaming
     Tools are not brew-installable; the `locate
     mediastreamvalidator` step treats absence as a soft gap
     rather than a failure and logs an escalation note.
   * Uploads `/tmp/msv-out` (playlists, validator logs, ffmpeg
     logs, `lvqr serve` log) as a 14-day build artifact.

   This is the single highest-leverage Tier 1 item on the
   critical path: every previous "LVQR LL-HLS is
   spec-compliant" claim was unverified because
   `mediastreamvalidator` had never been run against real LVQR
   output. The job closes that gap for PR feedback; the
   existing `lvqr-hls/tests/conformance_manifest.rs` still runs
   a synthetic-stub variant in the normal `cargo test` path as
   a unit-test-scope conformance check.

### Maturity audit deltas

* **Tier 1 "Apple mediastreamvalidator in CI"**: was NOT
  STARTED, now PARTIAL. The workflow exists and runs on every
  PR; it is not yet a required check because the baseline
  findings have not been triaged. Promotion to required is the
  session-32 entry point -- once the first successful run is
  captured, every finding gets fixed until exit zero is
  reliable, then `continue-on-error` comes off.
* **Tier 2.7 "WHIP Opus -> LL-HLS fragment observer"**: was an
  explicit carry-over item from session 30, now DONE.
* **Tier 2.5 LL-HLS**: was PARTIAL ~85%, now PARTIAL ~95%.
  Session 31 closed four of the five live-playlist `Open`
  items in the session-30 audit row: `EXT-X-INDEPENDENT-SEGMENTS`
  (`365b964`), `EXT-X-PRELOAD-HINT` (`365b964`),
  `EXT-X-RENDITION-REPORT` (`a637ee2`), and `CAN-SKIP-UNTIL` +
  `EXT-X-SKIP` delta-playlist support for the `_HLS_skip=YES`
  directive (`aab6156`, `6b63da1`). Still open: LL-HLS VOD
  windows for DVR scrub, byte-range addressing for partials.
* **Tier 2.6 `lvqr-dash`**: was NOT STARTED, now PARTIAL ~25%.
  Commit `3aefa5f` lands the crate skeleton + typed MPD
  renderer + 6 unit tests. The `DashServer` / axum router /
  `DashFragmentBridge` observer impl land in a follow-up.
* Everything else in the maturity audit remains unchanged from
  the `432290c` baseline.

3. **Tier 1 contract-script correction + whip fuzz target**
   (`faa8d58`, `529ae8b`). Session-30 audit appendix claimed
   the 5-artifact enforcement script was NOT STARTED; that
   was stale. Promoted `lvqr-whip` + `lvqr-whep` into the
   script's active `IN_SCOPE` list and added a new
   `lvqr-whip-fuzz` skeleton (`fuzz/fuzz_targets/parse_annex_b.rs`)
   that libfuzzes `split_annex_b` + `annex_b_to_avcc` +
   `hevc_nal_type` over arbitrary bytes with a pointer-bounds
   invariant. Wired both the new whip target and the
   pre-existing-but-orphaned whep `parse_offer_sdp` target
   into `.github/workflows/fuzz.yml` matrix so both now run
   on every relevant PR + nightly.

7. **Tier 2.6 `lvqr-dash` MPD renderer** (`3aefa5f`). First
   concrete step on the Tier 2.6 DASH egress budget the
   roadmap calls a "one-session project". This commit lands
   the crate skeleton + typed MPD renderer; the HTTP server
   (`DashServer`, `MultiDashServer`, axum router,
   `DashFragmentBridge` `FragmentObserver` impl) is a
   follow-up session so this commit can be reviewed in
   isolation.

   Architecture deliberately mirrors `lvqr-hls`: same
   `FragmentObserver` contract, same multi-broadcast shape,
   same reuse of `lvqr_cmaf::detect_video_codec_string` /
   `detect_audio_codec_string` so H.264 / HEVC / AAC / Opus
   publishers all surface the right codec attribute without
   any DASH-specific detection. The difference is the
   on-the-wire manifest format: an MPD XML document with a
   `Period` → `AdaptationSet` → `Representation` →
   `SegmentTemplate` hierarchy.

   Scope choices: live profile only (`type="dynamic"` with
   `$Number$` addressing); hand-written XML rather than a
   `quick-xml` serializer to keep the crate
   dependency-light and the output byte-stable for golden
   tests; no bandwidth discovery (conservative 2.5 Mbps /
   128 kbps hardcodes, like the LL-HLS master playlist).

   Six unit tests: full-skeleton happy path; audio
   AdaptationSet with `lang="en"` appended to a video MPD;
   empty-period / empty-MPD / empty-AdaptationSet rejections
   (typed `DashError` instead of panic); VOD static-type
   variant rendering `type="static"` instead of `dynamic`.

6. **Tier 2.5 CAN-SKIP-UNTIL + EXT-X-SKIP delta playlists**
   (`aab6156`, `6b63da1`). Fourth LL-HLS spec fix, closing
   the last non-VOD open item. Apple's `_HLS_skip=YES`
   directive lets a client request a truncated playlist that
   omits older segments in favour of a single
   `#EXT-X-SKIP:SKIPPED-SEGMENTS=N` tag, cutting bytes over
   the wire for long-running live sessions.

   `ServerControl` gained `can_skip_until: Option<Duration>`,
   defaulted to 12 s (6 * TARGETDURATION per Apple's
   recommendation). `Manifest::render` advertises
   `CAN-SKIP-UNTIL` in the `EXT-X-SERVER-CONTROL` line when
   `Some`, and a new public
   `Manifest::render_with_skip(skip_count)` companion renders
   a delta playlist with the first N segments replaced by the
   skip tag. `EXT-X-MEDIA-SEQUENCE` stays pointed at the
   original first segment per the spec. The decision of how
   many segments to skip lives in
   `Manifest::delta_skip_count` and enforces three spec floors
   in one place: `can_skip_until == None` → 0;
   total < 6 * TARGETDURATION → 0 (Apple spec 6.2.5.1);
   remaining-after-skip < 4 * TARGETDURATION → clamped down.
   `render_playlist` threads a new `_HLS_skip` query field
   through; both single and multi-broadcast routers now
   recognise the directive case-insensitively for `YES` or
   `v2`.

   Six new tests cover the path: four in `manifest.rs::tests`
   (server-control emission, below-floor refusal, window
   walk against a 20 s playlist, `can_skip_until=None`
   standalone-manifest regression guard); two in
   `integration_server.rs` (router-level delta happy path
   against 10 segments, short-playlist graceful degradation
   that ignores `_HLS_skip`).

5. **Tier 2.5 EXT-X-RENDITION-REPORT** (`a637ee2`). Third
   LL-HLS spec fix. Each media playlist now declares an
   `EXT-X-RENDITION-REPORT` tag for every sibling rendition in
   the master playlist, pointing at the sibling's live
   `(LAST-MSN, LAST-PART)` so a subscriber polling one
   rendition can discover the others' position without an
   extra round trip. New public
   `HlsServer::current_rendition_position` accessor reads the
   builder's manifest and returns the LL-HLS-correct position
   tuple (open-segment sequence + trailing partial index when
   partials pend, otherwise last-closed-segment sequence +
   last part index). `handle_multi_get` computes a
   single-element `RenditionReport` slice for whichever
   rendition the target is NOT currently rendering; the single-
   broadcast router always passes an empty slice.
   `render_playlist` gained a `reports: &[RenditionReport]`
   parameter and appends the lines via `append_rendition_reports`
   after the preload hint. Two new integration tests cover the
   happy path (video + audio both publishing, asserts both
   playlists contain the sibling report with the expected
   MSN/PART) and the video-only-broadcast regression guard
   (asserts no dangling report is emitted).

4. **Tier 2.5 LL-HLS media-playlist spec fixes** (`365b964`).
   Two mechanical additions the conformance workflow's Apple
   `mediastreamvalidator` is almost certain to flag on first
   run:

   * `EXT-X-INDEPENDENT-SEGMENTS` top-level tag. The builder
     only closes a segment on a Segment-kind chunk, which
     always carries a keyframe, so every closed segment is
     genuinely independently decodable. The invariant was
     already true; the playlist just never advertised it.
   * `EXT-X-PRELOAD-HINT:TYPE=PART,URI="..."` emitted after
     the trailing preliminary-part block. New
     `Manifest::preload_hint_uri: Option<String>` field is
     populated by `PlaylistBuilder::push` and
     `close_pending_segment` via a new private
     `next_part_uri()` helper that reuses the exact format
     `push` uses for fresh parts. URIs advance through
     `part-0-1` -> `part-0-2` -> `part-1-0` across a segment
     boundary so a client polling immediately after the
     boundary still sees a reachable URI. The audio prefix
     (`audio-`) flows through the helper correctly.

### Session 32 entry point

Two concrete pieces of work, in priority order:

**Step 1 -- Push the branch and triage the validator baseline**
(variable scope, do first so the CI run can churn in parallel
with Step 2).

The branch is 15 commits ahead of `origin/main` and
`hls-conformance.yml` has never run. `git push` is the first
action. Once the workflow runs on macos-latest, capture the
`mediastreamvalidator` output from the uploaded artifact. Session
31 pre-emptively closed the four live-playlist spec items most
validators flag first (`EXT-X-INDEPENDENT-SEGMENTS`,
`EXT-X-PRELOAD-HINT`, `EXT-X-RENDITION-REPORT`, `CAN-SKIP-UNTIL` +
`EXT-X-SKIP`), so the baseline should be narrower than it would
have been off the session-30 head. Remaining suspects to watch
for:

* `EXT-X-PART:DURATION` precision drift if the policy's
  `part_target_secs` (0.2 s) and the actual fragment durations
  disagree by more than a few ms.
* Blocking-reload `_HLS_msn` / `_HLS_part` query response
  formatting.
* Byte-range addressing for partials (not yet implemented; may
  be flagged as a warning rather than an error).
* LL-HLS VOD windows for DVR scrub (not yet implemented; same).
* Runtime gotchas in the workflow itself: the `mediastreamvalidator`
  binary may not be present on the `macos-latest` runner image
  (Apple HTTP Live Streaming Tools are not brew-installable).
  The workflow handles that via a soft-skip step; if the real
  validator never runs, the ffmpeg client-pull second signal is
  the only compliance read. Escalation path: self-hosted macOS
  runner with the HLS Tools .dmg pre-installed, or a mirror of
  the .pkg in a private S3 bucket.

Fix each finding in `lvqr-hls` in place as its own commit. Do
not bundle unrelated fixes. After the baseline is clean, flip
`.github/workflows/hls-conformance.yml`'s `continue-on-error: true`
to `false` in one final commit so the job becomes a required
check.

**Step 2 -- Finish the `lvqr-dash` Tier 2.6 work** (one focused
session of remaining scope after Step 1).

Session 31 landed the crate skeleton and the typed MPD renderer
(`3aefa5f`). The remaining work to close Tier 2.6 and let DASH
ride alongside LL-HLS in a single `lvqr serve` invocation:

* `crates/lvqr-dash/src/server.rs`: `DashServer` + `MultiDashServer`
  types mirroring `lvqr-hls::HlsServer` / `MultiHlsServer`. State:
  `Arc<DashState>` with `init_video` / `init_audio` bytes, a
  segment cache keyed on `(track, seq)`, latest segment counters
  for the MPD renderer. Methods: `new(DashConfig)`, `push_init`,
  `push_segment`, `router()` returning an `axum::Router`.
* Router routes:
    * `GET /dash/{broadcast}/manifest.mpd` -- renders the current
      MPD by composing an `Mpd` value from `DashConfig` + latest
      segment counters + codec strings pulled from the shared
      `lvqr_cmaf::detect_*_codec_string` helpers.
    * `GET /dash/{broadcast}/init-video.m4s` and
      `/init-audio.m4s` -- serve the cached init bytes.
    * `GET /dash/{broadcast}/seg-video-{n}.m4s` and
      `/seg-audio-{n}.m4s` -- look up cached segment bytes by
      `(track, n)`; 404 on miss.
* `crates/lvqr-dash/src/bridge.rs`: `DashFragmentBridge`
  implementing `lvqr_ingest::FragmentObserver`. `on_init` stashes
  bytes into the server's init slot. `on_fragment` writes segment
  bytes into the cache under `seg-<track>-<n>.m4s` where `n` is
  derived from the Fragment's `sequence` field. Follows the same
  drop-entry-before-observing reentrancy pattern the WHIP bridge
  and the HLS bridge already use.
* `crates/lvqr-cli/src/main.rs`: new `--dash-port` arg (env
  `LVQR_DASH_PORT`, default 0 = disabled). When non-zero,
  `serve_from_args` constructs a `MultiDashServer`, attaches it
  as a `FragmentObserver` on the RTMP + WHIP bridges, and mounts
  its router on a dedicated axum server bound to the configured
  address.
* Integration test in `crates/lvqr-dash/tests/integration_router.rs`
  driving two video segment pushes + one audio segment push
  through `tower::ServiceExt::oneshot`, asserting the MPD body
  contains both AdaptationSets and that every segment URI the
  manifest references resolves to the cached bytes.
* CLI integration test at `crates/lvqr-cli/tests/rtmp_dash_e2e.rs`
  mirroring `rtmp_hls_e2e.rs`: start a `TestServer` with both
  HLS + DASH ports, push an RTMP broadcast via `rml_rtmp`, read
  back `/dash/live/test/manifest.mpd` + at least one segment
  body over a real HTTP/1.1 loopback client.
* 5-artifact contract slots for `lvqr-dash`: promote to
  `IN_SCOPE` in `scripts/check_test_contract.sh` once at least
  proptest + integration + E2E are filled in.

Step 2 verification gates (on top of Step 1's):

    cargo fmt --all
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    git log -1 --format='%an <%ae>'   # Moheeb Zara alone

Commit scope: one commit per concrete change. Plan:

  1. Server + types.
  2. Bridge observer impl.
  3. CLI --dash-port wiring.
  4. Integration tests.
  5. Contract scope + docs.

After Step 2, Tier 2.6 in the maturity row flips from
PARTIAL ~25% to DONE, and the critical-path weight shifts to
`lvqr-srt` / `lvqr-rtsp` (the last two missing ingest protocols)
or to the Tier 3 webhook auth provider (the cheapest production-
auth item).

## Maturity audit -- what is left to v1.0 (session 30 follow-up)

Written after session 30 closed to calibrate the next 10-12
sessions against `tracking/ROADMAP.md`. Status values against
the roadmap's five tiers: **DONE**, **PARTIAL**, **NOT STARTED**.
Where a tier item is DONE in spirit but lacks CI / test /
interop validation, that's called out explicitly because the
difference between "the code exists" and "v1.0 can ship it"
is almost always a test harness gap, not a code gap.

### Tier 0 -- Fix the Audit Findings: DONE

Every item in the roadmap's Tier 0 list closed by session 19 at
the latest. The two historically-deferred items
(full `IngestProtocol` dispatch from the CLI, MoQ auth path
split) have not regressed but also have not been revisited;
neither blocks anything in Tier 1+.

### Tier 1 -- Test Infrastructure: PARTIAL (~55%)

| Item | Status | Notes |
|---|---|---|
| `lvqr-conformance` crate skeleton | DONE | Scaffold only; no crate pulls it as a dep yet beyond the kvazaar HEVC fixture and the ffprobe helper. |
| `lvqr-loadgen` crate | NOT STARTED | The data-plane load generator for concurrent subscriber sessions, byte-rate measurement, stall tracking, OTLP emission. Required for the M1 benchmark story. |
| `lvqr-chaos` crate | NOT STARTED | Fault injection (drop / reorder / delay / partition). Not blocking v1.0 but required for the 24h soak rig. |
| Proptest harnesses for every parser | PARTIAL | FLV, fMP4, AAC, HEVC SPS, WHEP RTP packetizer, WHIP depack all have proptests. Missing: MoQ wire messages, HLS playlist round-trip, catalog. |
| `cargo-fuzz` targets + nightly runner | DONE | `.github/workflows/fuzz.yml` runs libFuzzer targets on nightly rustc; PR + nightly cadences both wired. |
| `TestServer` in `lvqr-test-utils` | DONE | Used by five `lvqr-cli` integration tests (rtmp_hls_e2e, rtmp_archive_e2e, auth_integration, and smoke). |
| `testcontainers` fixtures (MinIO, etc.) | NOT STARTED | Archive-to-S3 upload story needs this. Not blocking because local-disk archive already works. |
| Playwright E2E suite at `tests/e2e/` | PARTIAL | Directory exists with `test-app.spec.ts`. `.github/workflows/e2e.yml` wired. Needs end-to-end coverage of WHIP publish + LL-HLS play + WHEP play cases. |
| ffprobe validation in CI | DONE | ffmpeg installed in the Linux + macOS jobs; the golden-fMP4 test exercises a real validator. |
| MediaMTX comparison harness | NOT STARTED | Blocks M1 "LVQR is structurally equivalent to MediaMTX" claim. One-session project once Tier 2.5 LL-HLS is frozen. |
| 24-hour soak rig | NOT STARTED | Nightly job that runs `lvqr-loadgen` against a long-lived server, asserts no memory / FD / gauge drift. Required for M2. |
| 5-artifact CI enforcement script | DONE (educational mode); PARTIAL for strict-mode promotion | Audit-appendix claim was stale: `scripts/check_test_contract.sh` and `.github/workflows/contract.yml` both exist and run on every PR in soft-fail educational mode. Session 31 promoted `lvqr-whip` + `lvqr-whep` into the in-scope list and added a `lvqr-whip-fuzz` target (`fuzz/fuzz_targets/parse_annex_b.rs`). Remaining work: add `lvqr-archive` once its in-crate test slots land, wire `lvqr-whip` / `lvqr-whep` fuzz targets into `fuzz.yml`, flip `LVQR_CONTRACT_STRICT=1` once the last "open" rows in CONTRACT.md are closed. |
| Golden-file regression corpus | PARTIAL | fMP4 writer has goldens; LL-HLS and master-playlist goldens don't exist yet. |
| `cargo audit` in CI | DONE | `.github/workflows/ci.yml` runs cargo audit on every PR. |
| Apple `mediastreamvalidator` in CI | PARTIAL (session 31) | `.github/workflows/hls-conformance.yml` runs on every PR against macos-latest, spins up `lvqr serve`, pushes a deterministic ffmpeg RTMP fixture, and runs `mediastreamvalidator` (plus an ffmpeg client-pull second signal). `continue-on-error: true` until the baseline findings are triaged; promotion to required is the session-32 entry point. |

Bottom line on Tier 1: the foundations that blocked Tier 2
progress are all done. The items still open are **benchmarking
and conformance gates**, not basic test ability. Nothing in
this list blocks feature work, but several items block the M1
release claim ("LVQR is measurably equivalent or better than
MediaMTX / MediaMTX-style servers").

### Tier 2 -- Unified Data Plane + Protocol Parity: PARTIAL (~70%)

| Sub-tier | Scope | Status | Notes |
|---|---|---|---|
| 2.1 | `lvqr-moq` facade + `lvqr-fragment` | DONE | The single most important call in the entire roadmap is closed. Both H.264 + HEVC video and Opus audio ride the unified model end-to-end. |
| 2.2 | `lvqr-codec` | PARTIAL | AAC (hardened with proptest + 7350 Hz floor), HEVC SPS, H.264 SPS all shipped. VP9 and AV1 parsers not started. Opus is opaque (no parser needed for passthrough); a `codec_string` generator would be nice-to-have. |
| 2.3 | `lvqr-cmaf` segmenter | DONE | AVC, HEVC, AAC, and Opus init writers all ship with unit tests and mp4-atom round-trip validation. `CmafPolicy`, `TrackCoalescer`, `build_moof_mdat` all in place. AV1 / VP9 init writers deferred alongside 2.2. |
| 2.4 | `lvqr-archive` | DONE | redb segment index, on-disk segments, HTTP playback surface (`/playback/*`), traversal guard, `SharedAuth` gate. S3 upload via `object_store` is NOT STARTED but roadmap's "optional" slot. |
| 2.5 | `lvqr-hls` + LL-HLS | PARTIAL (~95%) | Blocking reload, partials, master playlist with codec-aware CODECS, audio rendition group, multi-broadcast routing, `EXT-X-INDEPENDENT-SEGMENTS`, `EXT-X-PRELOAD-HINT`, `EXT-X-RENDITION-REPORT`, `CAN-SKIP-UNTIL`, and `EXT-X-SKIP` / `_HLS_skip=YES` delta playlists all done. **Open**: VOD windows for archive scrub in the HLS surface, byte-range addressing for partials. Apple `mediastreamvalidator` workflow landed in session 31 (`e2698f9`) but has not had its first baseline run yet -- promotion to required and triage of findings is the session-32 entry point. |
| 2.6 | `lvqr-dash` | PARTIAL (~25%, session 31) | MPD renderer skeleton landed in `3aefa5f`: `Mpd`, `Period`, `AdaptationSet`, `Representation`, `SegmentTemplate`, `MpdType`, `DashError` types plus `Mpd::render()` producing the `urn:mpeg:dash:profile:isoff-live:2011` live profile XML (hand-written to avoid a `quick-xml` dependency and keep the output byte-stable for golden tests). Six unit tests green including happy-path live MPD, audio AdaptationSet with `lang`, empty-MPD/empty-period/empty-set rejections, and the VOD static-type variant. **Open**: `DashServer` + `MultiDashServer` + axum router, `DashFragmentBridge` observer impl, `lvqr-cli` `--dash-port` flag, 5-artifact contract scope admission. |
| 2.7 | WHIP + WHEP | DONE | H.264, H.265, and Opus all ride end-to-end in both directions. Session 31 closed the last open thread: `WhipMoqBridge` now fires `FragmentObserver::on_init` + `on_fragment` for the `1.mp4` Opus track so LL-HLS audio renditions and the archive tee pick up WHIP audio automatically. |
| 2.8 | `lvqr-srt` | NOT STARTED | libsrt FFI, MPEG-TS demuxer, broadcast-encoder interop. Roadmap calls this ~2.5 weeks. One of the two "cut if Tier 2 blows its budget" candidates. |
| 2.9 | `lvqr-rtsp` server | NOT STARTED | Hand-rolled state machine + `retina` for RTSP-pull. The other cut candidate. |
| 2.10 | CLI single-binary default (M1 gate) | PARTIAL | RTMP + WHIP + WHEP + HLS + MoQ already wire up in one `lvqr serve` invocation. SRT + RTSP would round out the "all protocols at once" claim. `lvqr serve --demo` flag does not exist yet. |

### Tier 3 -- Cluster, Archive UI, Operational: NOT STARTED

Zero progress. Every item is deferred:

* **3.1 Cluster (chitchat + cross-node MoQ relay-of-relays)** -- the Bet 4 validation slot. Untested.
* **3.2 DVR scrub UI** -- the archive exists, the `lvqr-archive::SegmentIndex` can serve time-range queries via `/playback/*`, but no HLS/DASH VOD window binding exists and no player UI surfaces it.
* **3.3 Webhook + OAuth2 + signed URLs** -- the `AuthProvider` trait has noop / static / JWT impls. Webhook, OAuth2/JWKS, and HMAC signed-URL providers all NOT STARTED.
* **3.4 Observability (OTLP + Grafana + Alertmanager)** -- Prometheus metrics exist, OTLP exporter and dashboards do not.
* **3.5 Hot config reload (notify-rs + SIGHUP)** -- NOT STARTED. The `--config` flag was removed in the readiness audit because the loader did not exist; no config file plumbing has landed since.
* **3.6 Captions / WebVTT + SCTE-35 passthrough** -- NOT STARTED.
* **3.7 Stream key lifecycle + admin API expansion** -- NOT STARTED.

### Tier 4 -- Differentiation Moats: NOT STARTED

None of the Tier 4 moats have even a prototype:

* **4.1 io_uring datapath** (Linux-only, feature-flagged) -- NOT STARTED.
* **4.2 WASM per-fragment filters** (wasmtime host, single-filter pipeline) -- NOT STARTED.
* **4.3 C2PA signed media** (c2pa-rs at finalization) -- NOT STARTED.
* **4.4 Cross-cluster federation** -- NOT STARTED.
* **4.5 In-process AI agents** (whisper.cpp captions) -- NOT STARTED.
* **4.6 Server-side transcoding** (gstreamer-rs bridge, ABR ladder) -- NOT STARTED.
* **4.7 Latency SLO scheduling** -- NOT STARTED.
* **4.8 One-token-all-protocols** -- NOT STARTED.

### Tier 5 -- Ecosystem: NOT STARTED

Nothing from the ecosystem tier is built:

* Helm chart -- NOT STARTED.
* Kubernetes operator (`lvqr-operator`) -- NOT STARTED.
* Terraform module -- NOT STARTED.
* Web admin UI -- NOT STARTED (the brutalist `test-app/` is the only thing remotely like it).
* iOS / Swift SDK -- NOT STARTED.
* Android / Kotlin SDK -- NOT STARTED.
* Go SDK -- NOT STARTED.
* Rust client SDK (`bindings/rust/`) -- NOT STARTED.
* Docs site -- NOT STARTED.
* Tutorial videos -- NOT STARTED.

The JS side has `@lvqr/core` + `@lvqr/player` skeletons; the
Python admin client exists in `bindings/python/` (per README).

### Carry-over internal tech debt (from AUDIT-INTERNAL-2026-04-13 + subsequent sessions)

Items that are not tier items but block a clean v1.0 release:

1. **`lvqr-wasm` deletion**. Deprecated in v0.3; still a workspace member + a CI workflow build target. Removing it is a small cleanup but the CI workflow step lingers.
2. **CORS restrictive default**. `lvqr-admin` uses `CorsLayer::permissive()`; the `/playback/*` archive surface and the admin API both inherit it. Needs to be gated behind a `--cors-origin` flag before a public-internet deployment.
3. **`lvqr-mesh` media relay**. The tree topology planner ships; the actual DataChannel fanout that would make the mesh useful has never been implemented. The admin API reports "offload percentage" which today is intended, not actual. Flagged as Tier 4 and accurately labeled in the README.
4. **JWT integration test** (not just unit test). The `lvqr-cli::auth_integration` test covers the static-token path end-to-end; the JWT path is only covered by lvqr-auth unit tests.
5. **Criterion benchmarks**. The roadmap says fanout, fragment build, and archive scan should have criterion slots. Workspace has **zero** benches. Blocks any honest "X ns per op" marketing claim and blocks the M2 "published benchmarks vs MediaMTX" milestone.
6. **MoQ session auth publish-vs-subscribe split**. Tier 0 documented this as deferred to moq-native upstream. Still open.
7. **Track name convention is stringly-typed**. `"0.mp4"` / `"1.mp4"` literals appear in 6+ crates. A `TrackId` newtype would make the "kind by track name" invariant compiler-checked.
8. ~~**WHIP Opus -> LL-HLS fragment observer**~~. Closed in session 31 (`d4378bd`). `WhipMoqBridge::push_audio_sample` now fires the fragment observer alongside the raw-sample observer, so LL-HLS + archive pick up WHIP Opus without any HLS-side changes.
9. **Real-browser HEVC + Opus interop smoke**. The loopback E2Es prove the byte pipelines work; running against Safari / flag-enabled Chromium / Firefox is a deployment-validation step that hasn't happened.

### Gap summary by strategic weight

Roughly in the order a focused engineer should burn them down
to hit the M1 milestone (`lvqr serve --demo` is real and
defensible vs MediaMTX):

1. **LL-HLS spec-compliance gates** (Tier 2.5 follow-up): Apple `mediastreamvalidator` in CI, MediaMTX comparison harness, VOD window for DVR scrub. Blocks the "LVQR LL-HLS is actually compliant" claim.
2. **`lvqr-dash` egress** (Tier 2.6): one session, reuses existing CMAF segments + codec detectors. Closes the DASH column in the competitive matrix.
3. **Criterion benches + soak rig** (Tier 1 follow-up): blocks M2. One focused session each.
4. **Observability OTLP + Grafana dashboard pack** (Tier 3.4): blocks any production deployment evaluation.
5. **Webhook / OAuth2 / signed URLs** (Tier 3.3): blocks the "production auth" story; webhook is the cheapest path since it just POSTs `AuthContext` to a URL.
6. **SRT + RTSP** (Tier 2.8 / 2.9): the last two missing ingest protocols for M1 "single binary, all protocols". Can be cut if the budget slips.
7. **Hot config reload + stream key lifecycle** (Tier 3.5 / 3.7): ergonomics for operators.
8. **Helm + Kubernetes operator** (Tier 5): unlocks multi-node deployment stories for M3.
9. **Tier 4 differentiators**: none are on the critical path to v1.0. Each is its own ~3-week MVP capped block and should not start until M1 is green.
10. **SDKs (iOS / Android / Go / Rust)**: M5 material. Skip entirely until the server story is stable.

**Honest estimate of remaining work to v1.0 (M1 gate):**
roughly **6-10 focused sessions** for items 1-3 above, another
**8-12 sessions** for items 4-6, and a further **5-8 sessions**
of hygiene / cleanup / documentation. Total ~20-30 sessions to
a shippable v1.0-rc1, assuming no tier blowouts. Matches the
roadmap's "one focused engineer, 18-24 months" estimate.

---

## Session 30 (2026-04-15): Opus through LL-HLS and WHEP

One code commit on top of session 29's `7d72ba7`. Closes
session-29 recommended entry-point item 1. WHIP Opus publishers
now reach **both** LL-HLS subscribers (via a codec-aware
master playlist CODECS attribute) and WHEP subscribers (via a
same-codec Opus passthrough on the existing `Str0mAnswerer`
poll loop). With this, every egress path now honours Opus
audio end-to-end from a WHIP publisher.

### What landed

* **`VideoCodec` -> `MediaCodec` rename** in
  `lvqr-ingest::observer`. The original enum was introduced in
  session 26 with only `H264` / `H265` variants; session 30
  added `Aac` + `Opus` so the observer signaling carries real
  audio codec information. The new name is accurate (it covers
  audio and video) and the type alias back-compat was briefly
  left in place and then deleted once the rename cascaded
  through every call site in the workspace. Default is still
  `H264` so pre-session-28 code paths that did not pass an
  explicit codec keep their original shape.

* **RTMP bridge** now stamps `MediaCodec::Aac` on audio
  samples (was `VideoCodec::default()`, which resolved to
  H264 -- cosmetically wrong but harmless because WHEP
  dropped the AAC track on the track-name guard anyway).
  Session 30 makes this explicit.

* **WHIP bridge** `push_audio_sample` now fires the
  `SharedRawSampleObserver` with `MediaCodec::Opus`. Session
  29 deliberately skipped this because WHEP still hard-coded
  the H.264 packetizer; session 30 lifts that restriction.

* **WHEP `Str0mAnswerer`** grew parallel `audio_mid` /
  `audio_pt_opus` slots on `SessionCtx`. `absorb_event` learns
  the audio mid from `Event::MediaAdded { kind: Audio }`. The
  pt resolution sweep adds a `Codec::Opus` arm populating the
  audio pt slot. The old `write_video_sample` was generalised
  to `write_sample` with a codec-aware route:
  * `MediaCodec::H264` / `H265` -> video mid + `avcc_to_annex_b`
    + `Frequency::NINETY_KHZ`.
  * `MediaCodec::Opus` -> audio mid + raw payload bytes (Opus
    is opaque to str0m's packetizer) + `Frequency::FORTY_EIGHT_KHZ`.
  * `MediaCodec::Aac` -> warn-once drop (no AAC -> Opus
    transcoder). This keeps the RTMP publisher / WHEP
    subscriber pair from spamming logs when a user
    accidentally mixes RTMP ingest with a WHEP subscriber.
  `Str0mSessionHandle::on_raw_sample` now accepts track
  `"1.mp4"` alongside `"0.mp4"` and routes both through the
  shared `SessionMsg::Video` channel (name retained for
  continuity but carries audio too now).

* **`lvqr-cmaf::detect_audio_codec_string`** new sibling of
  the session-27 video detector. Walks a moov and returns
  `"opus"` for `Codec::Opus` or `"mp4a.40.<aot>"` for
  `Codec::Mp4a`. Returns `None` on parse failure, missing
  audio trak, or an unrecognised audio sample entry.

* **`lvqr-hls::HlsServer`** grew a cached
  `audio_codec_string: RwLock<Option<String>>` populated by
  `push_init` alongside the existing `video_codec_string`.
  A new accessor `video.audio_codec_string().await` mirrors
  the video one. `handle_master_playlist` reads the audio
  string from the audio sibling server (`multi.audio(broadcast)`)
  and folds it into the variant's `CODECS="..."` attribute.
  Falls back to `"mp4a.40.2"` when the audio init has not been
  decoded, matching the pre-session-30 default so a client
  hitting master.m3u8 mid-setup still sees a syntactically
  valid variant.

### 5-artifact contract status for session 30

| Slot | Status |
|---|---|
| Unit (`lvqr-cmaf::init::tests`) | +4 tests pinning the audio detector: `detect_audio_codec_string_reports_opus_from_opus_init` (round-trip through the new Opus writer -> detector, expects `"opus"`), `..._reports_mp4a_from_aac_init` (round-trip through the AAC writer, expects `"mp4a.40.2"` for the pinned `[0x12, 0x10]` ASC), `..._returns_none_on_garbage` (empty + invalid bytes), and `..._returns_none_on_video_only_init` (an AVC init must not match the audio detector path). |
| Integration (`lvqr-hls::integration_master`) | +1 test: `master_playlist_reports_opus_codec_when_audio_rendition_has_opus_init`. Pushes a real Opus init segment into `ensure_audio("live/opus-audio", 48_000)` and asserts the master playlist advertises `CODECS="avc1.42001F,opus"` with no `mp4a.40.2` fallback. |
| Proptest | No new proptest slot. Opus payload bytes are opaque at the observer layer (no parser needs a never-panic property); the existing depacketizer proptest continues to cover the codec-agnostic framing. |
| Fuzz | Still deferred. |
| E2E (`lvqr-whep`) | **New**: `tests/e2e_str0m_loopback_opus.rs`. Client Rtc built with `enable_opus(true)` only (no video) and a recvonly audio mid; sanity-asserts the SDP answer contains an "opus" rtpmap substring so a str0m regression is immediately visible; pumps synthetic 10-byte Opus "frames" through `Str0mSessionHandle::on_raw_sample` with track `"1.mp4"` and `MediaCodec::Opus`; waits on the client poll loop for `Event::MediaData` frames decrypted out of str0m's Opus RTP pipeline. Completes in ~0.16s on loopback. |

### Load-bearing decisions made

* **Rename rather than parallel enum**. The alternative (keep
  `VideoCodec` + add `AudioCodec` + widen the observer
  signature again) would have duplicated the H264/H265 vs
  Aac/Opus machinery and required two separate match arms at
  every WHEP route point. One enum with 4 variants is simpler
  to reason about and matches the mental model "this is the
  codec of whatever sample is on this track".
* **Opus is opaque to the packetizer**. Unlike H.264 / H.265
  (which need AVCC -> Annex B conversion before str0m's
  packetizer can find NAL boundaries), Opus RTP is one-packet-
  per-frame (RFC 7587) with no intra-frame framing. WHEP's
  `write_sample` passes the Opus bytes through unchanged.
* **AAC on the WHEP write path drops, not errors**. A
  misconfigured pairing (RTMP publisher with AAC audio and a
  WHEP subscriber that negotiated Opus) would otherwise spam
  the error log. The fix is on the publisher / subscriber
  side; WHEP warns once and drops subsequent AAC samples for
  the session. The video portion of the stream continues to
  work normally.
* **Audio codec string fallback to `"mp4a.40.2"`**. When a
  client hits master.m3u8 before the first audio init
  segment has landed, the fallback string has to be _some_
  syntactically valid AAC codec so players don't reject the
  variant. Pre-session-30 that string was hardcoded; session
  30 keeps it as the fallback because RTMP + AAC is still the
  dominant audio publisher in practice.
* **Shared `SessionMsg::Video` channel carries audio too**.
  Adding a second channel variant would have been cleaner but
  introduced a channel-selection branch in
  `Str0mSessionHandle::on_raw_sample` that bought nothing -- the
  codec tag on the message is already enough to pick the
  right pt / mid / clock in `write_sample`. The name is kept
  for source continuity; a future session can rename it.

### Known gaps explicitly not closed in this session

* **LL-HLS audio media playlist for Opus**. The master
  playlist now advertises `opus` in its CODECS attribute, and
  the audio `HlsServer` serves Opus init bytes through
  `init.mp4`, but the audio media playlist itself still uses
  the existing `PlaylistBuilder` which was tuned for AAC
  partial durations. Opus frames are 20 ms (960 ticks at
  48 kHz) so the `#EXT-X-PART:DURATION` values are still
  correct; no divergence observed yet. A real browser
  interop test would confirm this.
* **WHIP Opus through `lvqr-hls::HlsFragmentBridge`**. The
  WHIP bridge's `push_audio_sample` fires the raw-sample
  observer (WHEP) but still skips the fragment observer (HLS
  + archive). That means a WHIP Opus publisher currently
  reaches WHEP subscribers and MoQ subscribers but not
  LL-HLS; the HLS path still only serves Opus if some other
  producer feeds an Opus-init `ensure_audio` server. The
  integration test exercises the detector via a direct
  `audio.push_init()` call rather than end-to-end from a
  WHIP publisher. Closing this is a small follow-up -- fire
  the fragment observer from `push_audio_sample` and verify
  `HlsFragmentBridge` routes it sensibly.
* **`lvqr-dash` egress**, archive VOD playlist, criterion
  benches, fuzz catch-up, `lvqr-wasm` deletion, CORS
  restrictive default, real-browser interop smoke. All
  unchanged.

### Recommended entry point (session 31)

1. **WHIP Opus -> LL-HLS end-to-end**. Wire the fragment
   observer from `WhipMoqBridge::push_audio_sample` and
   confirm via a combined E2E (WHIP publisher -> LL-HLS
   master.m3u8 + audio.m3u8 + audio segments). This turns
   the current session-30 integration test from "direct
   `ensure_audio` push" to "real WHIP publisher drives the
   audio rendition".
2. **`lvqr-dash` egress** (Tier 2.6). Reuse both
   `detect_video_codec_string` and `detect_audio_codec_string`
   for `<Representation codecs="..."/>`.
3. **Real-browser HEVC + Opus interop smoke**. Unchanged.
4. **Archive VOD playlist rendering**. Unchanged.
5. **Benchmark slots via `criterion`**. Unchanged.
6. **Fuzz slot catch-up**. Unchanged.
7. **`lvqr-wasm` deletion**. Unchanged.
8. **CORS restrictive default**. Unchanged.

### Competitive-matrix delta after session 30

Checkmarks gained:

* **WHIP audio -> WHEP egress (Opus passthrough)**: N -> **Y**.
  Same-codec forwarding with no transcode; a single WHEP
  subscriber can receive Opus from a WHIP publisher.
* **LL-HLS audio codec advertisement**: hardcoded -> **dynamic**.
  Master playlist advertises `opus` or `mp4a.40.<aot>` based
  on the real init segment bytes.
* **WHIP audio ingest (Opus)**: Partial -> **Y** for MoQ +
  WHEP; **Partial** for LL-HLS (master playlist + audio init
  done, end-to-end WHIP -> LL-HLS audio fragment fanout is
  session 31 item 1).

---
## Session 29 (2026-04-15): Opus-native audio through the WHIP bridge

One code commit on top of session 28's `5306977` plus an
interleaved audit commit (`9292208`). Closes session-28
recommended entry-point item 1 (WHIP audio path). WHIP
publishers negotiating Opus now land an audio track on the MoQ
broadcast alongside their video without any transcode, with a
proper `Opus` sample entry + `dOps` init segment and a
sibling `1.mp4` MoQ track. This is option (a) from the
session-25 recommendation -- the cheap path that unlocks
browser audio for every Opus-capable MoQ subscriber without
standing up an AAC encoder.

### What landed

* **`lvqr-cmaf::write_opus_init_segment` + `OpusInitParams`**
  in `crates/lvqr-cmaf/src/init.rs`. Emits the standard
  ISO/IEC 14496-30 `Opus` sample entry with an embedded
  `dOps` box (`mp4-atom` 0.10 supports both). Channel counts
  are constrained to the `channel_mapping_family == 0` path
  (1 or 2 channels); anything else is rejected as
  `InitSegmentError::Encode(Unsupported(..))` because
  `mp4-atom`'s `Dops` encoder only writes the simple mapping.
  Timescale is 48_000 by convention -- Opus always runs at
  that rate internally and MSE players expect the track
  timescale to match.

* **`lvqr-whip::IngestAudioSample`** new public struct in
  `crates/lvqr-whip/src/bridge.rs`: `{ dts_48k, duration_48k,
  payload: Bytes }`. Separate from the video
  [`IngestSample`] because the video type carries
  `VideoCodec` + keyframe flags that have no analog on the
  audio side, and unifying the two would force every sink
  impl to branch.

* **`IngestSampleSink::on_audio_sample`** new trait method
  with a default no-op implementation so existing test
  sinks (the H.264 and HEVC E2E loopback sinks) do not need
  to grow a method. `WhipMoqBridge` overrides this to lazily
  create a `1.mp4` MoQ audio track on the first Opus frame
  that arrives after the broadcast has been initialized via
  a video keyframe. Audio-before-video is dropped silently:
  the broadcast slot doesn't exist yet, and holding audio
  back would grow an unbounded queue.

* **`BroadcastState`** grew three audio fields (`audio_sink`,
  `audio_seq`, `audio_init_emitted`). Also renamed the
  previously-unused `_broadcast` field to `broadcast` since
  `ensure_audio_initialized` calls `create_track("1.mp4")`
  on the existing `BroadcastProducer`. The two tracks share
  one `BroadcastProducer` / one MoQ fanout connection but
  carry independent sequence numbers and init states.

* **`lvqr-whip::str0m_backend`** gained an `audio_mid` slot on
  `IngestCtx`, a `dts_base_48k` rebase anchor (independent
  of `dts_base_90k` so the two tracks have their own zero
  epochs), and a new `forward_audio_sample` path. The
  `handle_event` match arm for `Event::MediaData` now
  branches on `video_mid` vs `audio_mid` and routes to the
  matching forwarder. Non-Opus audio codecs (PCMA / PCMU
  fallback) are dropped with a trace log.
  `MediaTime::rebase(Frequency::FORTY_EIGHT_KHZ)` is the
  boundary-crossing 48 kHz rebase.

* **Observer fanout intentionally stays video-only** for
  audio in session 29. The fragment observer (LL-HLS +
  archive) and raw-sample observer (WHEP) do not fire for
  audio samples. Rationale: LL-HLS audio rendition builder
  in `MultiHlsServer::ensure_audio` is hardcoded to AAC, and
  the master playlist still advertises `mp4a.40.2` for the
  audio portion (session 27 made the video codec
  dynamic but left audio as a follow-up). WHEP negotiates
  Opus on the subscriber side but currently drops ingest
  audio entirely because the RTMP path produces AAC and
  there's no AAC->Opus transcoder. Threading codec-aware
  audio egress through LL-HLS (server advertises `opus` in
  master CODECS and serves an Opus init segment) and WHEP
  (forward Opus through to subscribers without a transcode)
  is **session 30**.

### 5-artifact contract status for session 29

| Slot | Status |
|---|---|
| Unit (`lvqr-cmaf::init::tests`) | +3 tests pinning the Opus writer: `opus_init_segment_starts_with_ftyp_and_contains_moov` (smoke), `opus_init_segment_round_trips_through_mp4_atom` (decode-side verification that `Codec::Opus` round-trips through mp4-atom with the pre_skip / channel_count / timescale the writer emits), `opus_init_segment_rejects_invalid_channel_count` (0, 3, 6 all rejected). |
| Unit (`lvqr-whip::bridge::tests`) | +3 tests for the bridge audio path: `opus_audio_sample_before_video_is_dropped` pins the video-first ordering invariant, `opus_audio_sample_after_video_initializes_audio_track` walks the happy path twice to prove `ensure_audio_initialized` is idempotent, and `opus_empty_payload_is_dropped_silently` pins the empty-frame guard. |
| Integration | No new integration slot. The signaling path is codec-agnostic; audio negotiation is exercised through `str0m::sdp_api::accept_offer` which the E2E already covers. |
| Proptest | No new proptest slot. The Opus frame bytes are opaque at this layer (no parser that needs a never-panic property), and the existing `lvqr-whip::proptest_depack` tests cover the length-prefixed framing. |
| Fuzz | Still deferred. |
| E2E | **New**: `tests/e2e_str0m_loopback_opus.rs`. Client `Rtc` builds an offer carrying **both** a video (H.264) and an audio (Opus) section, server answerer accepts, both poll loops complete ICE + DTLS + SRTP, client writes synthetic H.264 SPS/PPS/IDR through the video writer AND synthetic 10-byte "Opus" packets through the audio writer every 20 ms, and the server's capture sink (impls both `on_sample` and `on_audio_sample`) asserts at least one sample in each slot. Completes in ~0.19s on loopback. |

### Load-bearing decisions made

* **Sibling track on the same `BroadcastProducer`**, not a
  separate broadcast. MoQ subscribers reach the audio track
  by subscribing to `1.mp4` on the existing broadcast; they
  do not need to discover a parallel broadcast name. The
  `BroadcastState.broadcast` (previously `_broadcast`) stays
  alive to host both tracks. One network fanout, two logical
  tracks.
* **Audio rebase to a separate 48 kHz zero epoch**, not the
  video 90 kHz epoch divided down. Mixing them would require
  a common wall-clock anchor and synchronous arrival which
  str0m does not guarantee. MSE / MoQ subscribers align the
  two tracks via presentation-time metadata, not via shared
  DTS, so independent epochs are correct and simpler.
* **`IngestSampleSink::on_audio_sample` default no-op**.
  Keeps the existing three E2E test sinks source-compatible
  without a breaking trait change. Tests that only care
  about video (the H.264 + HEVC loopbacks) inherit the
  default; tests that care about audio (the new Opus
  loopback) override both methods.
* **Video-first ordering invariant**. Audio-before-video
  drops rather than buffers. The alternative (buffer audio,
  replay after video init) would require an unbounded queue
  and a late-dispatch code path; the failure mode of audio
  dropping at the very start of a publish is bounded and
  obvious.
* **Opus frame duration defaulted to 960 ticks (20 ms)**.
  str0m's 0.18 `MediaData` does not expose the per-packet
  duration, and WebRTC Opus defaults to 20 ms PTIME so the
  default is correct for every real publisher. A slightly
  wrong `duration` in the `trun` box is cosmetically wrong
  but functionally harmless -- MSE decoders reconstruct the
  actual Opus frame duration from the packet body itself.

### Known gaps explicitly not closed in this session

* **LL-HLS audio rendition for Opus**. Still AAC-only in the
  master playlist. Session 30.
* **WHEP Opus forwarding**. Session 30. The subscriber-side
  Opus pt exists (session 28 enabled it), but WHEP currently
  drops ingest audio in `Str0mSessionHandle::on_raw_sample`
  with a warn-once on the `1.mp4` track.
* **DASH egress**, archive VOD playlist, criterion benches,
  fuzz catch-up, lvqr-wasm deletion, CORS restrictive
  default. All unchanged.

### Recommended entry point (session 30)

1. **Opus through LL-HLS and WHEP**. Now that the publisher
   side can produce Opus, plumb it through the two remaining
   egress paths: LL-HLS (thread an audio codec string through
   the master playlist + handle Opus init in the audio
   `HlsServer`) and WHEP (forward Opus ingest samples to
   subscribers that negotiated Opus; same-codec passthrough,
   no transcode). Budget: one session.
2. **`lvqr-dash` egress** (Tier 2.6). Unchanged.
3. **Real-browser HEVC + Opus interop smoke**. Unchanged.
4. **Archive VOD playlist rendering**. Unchanged.
5. **Benchmark slots via `criterion`**. Unchanged.
6. **Fuzz slot catch-up**. Unchanged.
7. **`lvqr-wasm` deletion**. Unchanged.
8. **CORS restrictive default**. Unchanged.

### Competitive-matrix delta after session 29

Checkmarks gained:

* **WHIP audio ingest (Opus)**: N -> **Partial**. MoQ
  subscribers receive Opus alongside video; LL-HLS and WHEP
  audio fanout still pending.
* **Codec-honest WHIP publisher surface**: H.264 video,
  H.265 video, and Opus audio all reach MoQ subscribers
  natively without a transcode.

---
## Session 28 (2026-04-15): HEVC through the WHEP egress path

One code commit on top of session 27's `9c69319`. Closes
session-27 recommended entry-point item 1. WHEP subscribers can
now receive HEVC from an HEVC publisher through the same
`Str0mAnswerer` poll loop that H.264 subscribers use, with
per-sample codec-aware payload-type routing.

### What landed

* **`VideoCodec` relocated to `lvqr-ingest::observer`**. Session
  26 put the enum in `lvqr-whip::bridge`; session 28 moved it
  down one layer so `lvqr-ingest` (which both `lvqr-whip` and
  `lvqr-whep` depend on) owns the canonical definition. The
  enum gained a `#[derive(Default)]` with `H264` as the default
  so the RTMP bridge's audio-sample observer call can pass a
  placeholder without a codec match. `lvqr-whip` now
  re-exports `lvqr_ingest::VideoCodec` for backwards
  compatibility with any caller that grabbed the type from the
  whip crate's public API after session 26.

* **`RawSampleObserver::on_raw_sample` signature widened** from
  `(broadcast, track, sample)` to `(broadcast, track, codec,
  sample)`. Every call site updated:
  * `lvqr-ingest::RtmpMoqBridge` stamps `VideoCodec::H264`
    unconditionally (enhanced-RTMP HEVC is a later session).
  * `lvqr-whip::WhipMoqBridge::push_sample` passes
    `sample.codec` and drops its AVC-only guard. The fragment
    observer (HLS + archive) lost its guard in session 27; the
    raw-sample observer (WHEP) lost it here. HEVC WHIP
    publishers now reach every egress.
  * `lvqr-whep::WhepServer`'s `RawSampleObserver` impl forwards
    the codec to each matching session.

* **`lvqr-whep::SessionHandle::on_raw_sample` widened** to
  accept a codec parameter so the session backend can decide
  where to route the sample without sniffing NAL headers. The
  WHEP-internal subtrait mirror of `RawSampleObserver` keeps
  the same shape as the lvqr-ingest trait to avoid two concepts
  of "this is the codec" in the same crate.

* **`lvqr-whep::Str0mAnswerer::create_session`** now builds
  `RtcConfig` with `.enable_h264(true).enable_h265(true)
  .enable_opus(true)`. SDP answers carry both video payload
  types when the client offers both; they carry only the
  negotiated codec when the client offers only one (Safari:
  both; Chrome behind an experimental flag: both; Firefox:
  H264 only today).

* **`SessionCtx`** grew parallel `video_pt_h264` /
  `video_pt_h265` slots (replacing the old single `video_pt`).
  The lazy pt resolution sweep inside `run_session_loop`
  populates both in the same iteration over
  `Writer::payload_params()`, and `write_video_sample`
  receives the incoming sample's codec tag and picks the
  matching pt. A sample whose codec is not in the negotiated
  payload params (e.g. an HEVC publisher but a Firefox
  subscriber) is dropped with a one-shot warn
  (`unmatched_codec_logged` guard) so a wedged pairing does
  not drown the log.

* **`SessionMsg::Video` grew a `codec` field** so the channel
  from `Str0mSessionHandle::on_raw_sample` (running on the
  ingest bridge task) to the poll-loop task (running on its
  own tokio task) carries the per-sample routing tag.

### 5-artifact contract status for session 28

| Slot | Status |
|---|---|
| Unit | `lvqr-whep`'s existing unit tests in `src/str0m_backend.rs` still cover the AVCC -> Annex B converter and the `on_raw_sample` warn-once audio path. They now take a `VideoCodec` parameter but the assertions are unchanged. |
| Integration | `lvqr-whep::tests::integration_signaling` updated for the new observer signature; the 12 existing tests still pass. |
| Proptest | No new proptest slot needed. The AVCC -> Annex B converter is codec-agnostic (HEVC length-prefixed AVCC uses the same 4-byte length header), and the existing proptest cover ensures the converter never panics on arbitrary bytes regardless of codec. |
| Fuzz | Still deferred. |
| E2E | **New**: `tests/e2e_str0m_loopback_hevc.rs`. Builds a client `Rtc` with **only** `enable_h265(true)` so the SDP offer carries exactly one video codec (HEVC); asserts via a string search on the answer text that the negotiation picked H.265 (so the test surfaces a regression in str0m's H265 SDP path immediately); pumps real x265 Main VPS + SPS + PPS NAL bytes plus a synthetic IDR_W_RADL body through `Str0mSessionHandle::on_raw_sample` with `VideoCodec::H265`; waits on the client poll loop for `Event::MediaData` frames decrypted out of RTP packetized by str0m's `H265Packetizer`. Completes in well under a second on loopback. |

### Load-bearing decisions made

* **Codec tag on the observer call, not on `RawSample`**.
  Widening `RawSample` with a codec field would have touched
  12 files with struct literals across the workspace;
  widening the observer trait touches 6 (the trait, its noop
  impl, two call sites, two test stubs). The trait signature
  is also where the type *actually* matters (only `lvqr-whep`
  cares about the codec tag for routing), so localising the
  change keeps the unified-sample model (Bet 3) intact.
* **Dual-pt resolution, not session-level codec state**. str0m
  already multiplexes payload types within a single mid;
  resolving both `video_pt_h264` and `video_pt_h265` lazily in
  the same sweep means a subscriber that offered both codecs
  can receive samples of either type from the same publisher
  through the same WHEP session without renegotiation. A
  future "publisher changes codec mid-stream" scenario is
  unusual but fits this shape naturally.
* **Warn-once on unmatched codec, not error**. A WHEP session
  whose subscriber offered only H.264 but whose publisher is
  HEVC is a real deployment mistake (Firefox subscriber,
  HEVC-only publisher). Dropping samples with a single warn
  is the correct behavior -- the alternative (tear down the
  session) would surface as a confusing MediaData gap on the
  client. The fix is in the subscriber's offer, not the
  server.
* **`RtcConfig::enable_h265(true)` unconditionally**. Leaving
  it behind a feature flag or a config knob would mean
  deployments have to opt in. WHEP already advertises only
  what the client offered, so enabling H.265 server-side is
  lossless: H.264-only clients still get H.264, H.265-capable
  clients get the option. Session 26 already proved str0m's
  H.265 stack is stable enough to ship.

### Known gaps explicitly not closed in this session

* **WHIP HEVC audio path** (Opus-native sibling track). Still
  pending, was session-25 entry point item 1.
* **Real-browser HEVC interop**. The loopback test proves the
  byte pipeline works; running against Safari or a
  flag-enabled Chromium is a deployment validation step, not
  a unit test.
* **HEVC through `lvqr-wasm` / `@lvqr/player`**. The MoQ
  subscriber path delivers `hvc1`-init fMP4 fragments; browser
  decode support varies. Tracked as a follow-up when the
  WASM/TS side gets a codec-dispatch.
* **`lvqr-dash` egress**, archive VOD playlist, criterion
  benches, fuzz catch-up, `lvqr-wasm` deletion, CORS
  restrictive default. All unchanged.

### Recommended entry point (session 29)

With HEVC now closed end-to-end across every LVQR egress path
(MoQ / LL-HLS / WHEP / archive), the strategic-leverage order
pivots back to the items HANDOFF has been deferring since
session 25:

1. **WHIP audio path** (Tier 2.7 follow-on). Opus-native
   sibling `1.mp4` track on `WhipMoqBridge`; new
   `write_opus_init_segment` in `lvqr-cmaf` (needs an Opus
   sample entry + `dOps` box). This is the largest remaining
   gap in the WHIP publisher surface. Budget: one session for
   the core path, one more session if MSE-side playback
   through `@lvqr/player` needs work.

2. **`lvqr-dash` egress** (Tier 2.6). New crate, typed MPD
   generator via `quick-xml`. Can reuse
   `lvqr_cmaf::detect_video_codec_string` from session 27 for
   the `<Representation codecs="...">` attribute. Budget: one
   session.

3. **Real-browser HEVC interop smoke**. Bring up Safari and a
   flag-enabled Chromium against `lvqr serve --whip-port
   1936`, confirm a local `simple-whep-client` HEVC publisher
   reaches a Safari WHEP subscriber. Turns the session-28
   loopback into a deployment-validated story.

4. **Archive VOD playlist rendering**. Unchanged.

5. **Benchmark slots via `criterion`**. Unchanged.

6. **Fuzz slot catch-up**. Unchanged.

7. **`lvqr-wasm` deletion**. Unchanged.

8. **CORS restrictive default**. Unchanged.

### Competitive-matrix delta after session 28

Checkmarks gained:

* **HEVC egress via WebRTC (WHEP)**: N -> **Y**. LVQR now
  serves HEVC publishers to WHEP subscribers natively, with
  no transcode, over the same poll loop that handles H.264.
* **HEVC end-to-end through every egress**: **Y** across MoQ,
  LL-HLS, WHEP, and DVR archive. Only real-browser
  interoperability validation remains.

---
## Session 27 (2026-04-14): codec-aware LL-HLS for HEVC publishers

Two code commits on top of session 26's `9691744`. Closes
session-26 recommended entry-point item 1 for the HLS + archive
half (WHEP is session 28). HEVC WHIP publishers now reach the
LL-HLS master playlist and the DVR archive with the correct
`CODECS="hvc1.1.6.L60.B0"` attribute in the variant stream,
and the existing AVC path keeps its `avc1.PPCCLL` string but
now computes it from the real init segment bytes instead of
the session-13 hardcode. `lvqr-whip` drops its AVC-only
fragment-observer guard; the raw-sample observer (WHEP) stays
AVC-only pending session-28 WHEP H265 packetization.

### Commits

* **`37d243e`** -- `lvqr-codec: reject explicit AAC sample
  rates below 7350 Hz`. Found by a proptest seed that decoded
  `[87,128,0,0,128]` to a valid 1 Hz rate via the explicit 24-bit
  escape path. The parser only rejected zero; anything else
  slipped through and drove nonsense downstream (init segment
  timescale, LL-HLS partial duration reporting). Tightened the
  floor to 7350 Hz, the lowest `AAC_SAMPLE_FREQUENCIES` table
  entry, matching the plausibility invariant `proptest_aac`
  asserts. Also fixed a bit-packing bug in the existing
  `lvqr-cmaf::init::tests::aac_init_rejects_non_indexable_sample_rate`
  fixture that claimed to encode 11468 Hz but actually encoded
  1433 Hz. Strictly a hardening commit -- unrelated to the
  HEVC-through-HLS work, but it was blocking the workspace
  green.

* **`72970c6`** -- `Tier 2.7: codec-aware LL-HLS master
  playlist for HEVC publishers`. The real session delivery.
  See the "What landed" section below for the full breakdown.

### What landed

* **`lvqr-cmaf::detect_video_codec_string`**. New public
  function in `crates/lvqr-cmaf/src/init.rs`. Decodes an
  fMP4 init segment (ftyp + moov) via `mp4-atom`, walks the
  first `Stsd` codec list, and returns the ISO BMFF codec
  string the HLS / DASH master playlist needs to advertise:

  * `Codec::Avc1(avc1)` -> `format!("avc1.{:02X}{:02X}{:02X}",
    avcc.avc_profile_indication, avcc.profile_compatibility,
    avcc.avc_level_indication)`. For the x264 Baseline @L3.1
    fixture this produces `"avc1.42001F"`.
  * `Codec::Hev1(hev1)` -> `hvc1.<space><profile>.<compat_rev
    in hex>.<tier><level>.B0`. For the x265 Main @L2.0 fixture
    this produces `"hvc1.1.6.L60.B0"`. The compatibility flags
    are reverse-bit-ordered per ISO/IEC 14496-15 Annex E.3 --
    the format every real MSE / hls.js / dash.js parser
    expects. The trailing `.B0` abbreviation is the accepted
    shorthand for the "all constraint bits zero" common case;
    a non-zero constraint byte falls back to emitting the
    first non-zero hex byte.
  * Any other sample entry or parse failure -> `None`.

* **`lvqr-hls::HlsServer`** grows a new
  `video_codec_string: RwLock<Option<String>>` slot on
  `HlsServerState`, populated inside `push_init` by a
  `lvqr_cmaf::detect_video_codec_string(&bytes)` call and
  re-read via a new public `video_codec_string()` accessor.
  `handle_master_playlist` reads the video server's cache and
  populates the variant's `CODECS="..."` attribute from it,
  falling back to `avc1.640020` when no init segment has
  arrived yet (so a client hitting master.m3u8 before the
  first fragment still sees a syntactically valid variant).
  Audio stays AAC-only in the string because the only live
  audio path is RTMP + AAC; when WHIP audio lands with an
  Opus sibling track, extend this with a parallel
  `detect_audio_codec_string`.

* **`lvqr-whip::WhipMoqBridge`** drops the AVC-only guard on
  the fragment observer path in both `ensure_initialized`
  and `push_sample`. HEVC publishers now fan through
  `SharedFragmentObserver::on_init` + `on_fragment`, which
  means the `HlsFragmentBridge` and `IndexingFragmentObserver`
  both see HEVC fragments and treat them correctly: the HLS
  bridge builds a CMAF policy matching the 90 kHz track
  timescale and serves the init bytes verbatim through the
  new `push_init` detector, while the archive observer writes
  the fragment payload bytes to disk indifferently (the
  codec has never mattered to archive). The raw-sample
  observer (WHEP) still fires only for
  `VideoCodec::H264`; threading a second H265 packetizer
  through `lvqr-whep::str0m_backend` is session 28.

### 5-artifact contract status for session 27

| Slot | Status |
|---|---|
| Unit (`lvqr-cmaf::init::tests`) | +3 tests pinned against the real x264 Baseline + x265 Main fixtures: `detect_video_codec_string_reports_avc1_from_avc_init` asserts the exact string `"avc1.42001F"`; `detect_video_codec_string_reports_hvc1_from_hevc_init` asserts `"hvc1.1.6.L60.B0"`; `detect_video_codec_string_returns_none_on_garbage` covers the empty / zero / non-mp4 input paths. |
| Integration (`lvqr-hls::integration_master`) | +3 tests driving the real `MultiHlsServer::router` via `tower::ServiceExt::oneshot`. `master_playlist_reports_avc1_codec_for_avc_init` pushes a real AVC init segment built via `write_avc_init_segment` and asserts the master advertises `CODECS="avc1.42001F"`. `master_playlist_reports_hvc1_codec_for_hevc_init` does the same for HEVC and additionally asserts the absence of an `avc1.` substring. `master_playlist_appends_aac_when_audio_rendition_exists` exercises the combined `"avc1.42001F,mp4a.40.2"` path with an audio rendition attached. |
| Proptest | Unchanged; `parse_asc`'s plausibility invariant is now enforced by the parser floor so `proptest_aac` passes deterministically. |
| Fuzz | Unchanged (still deferred for the same nightly-rustc reason). |
| E2E | Not extended in this session: the existing `e2e_str0m_loopback_hevc` E2E proves the HEVC sample reaches the `WhipMoqBridge` capture sink, and the new lvqr-hls integration tests prove the master playlist is codec-aware. A combined "real WHIP HEVC publisher -> real LL-HLS master.m3u8 response" E2E is valuable but requires standing up both the WHIP server and the HLS server in the same test harness; that is a session-28 follow-up when the WHEP codec surface also needs the same end-to-end harness. |

### Load-bearing decisions made

* **Codec string detected at `push_init` time, not at every
  master-playlist request**. Parsing a moov costs more than
  formatting a cached string; doing it once per init segment
  amortises across every client that hits master.m3u8. The
  init segment is small (~400 bytes for AVC, ~450 for HEVC)
  so the one-shot decode is cheap.
* **No trait change to `FragmentObserver`**. The codec
  discovery lives inside `lvqr-hls` via the init-byte decoder,
  not as a new method on `FragmentObserver::on_init`. Keeping
  the trait unchanged preserves Bet 3 (unified fragment model)
  and means every other observer (archive, record, hypothetical
  future consumers) keeps its codec-indifferent shape.
* **ISO/IEC 14496-15 Annex E.3 bit-reversed compatibility
  flags**. The less-common encoding but the one MSE / hls.js /
  dash.js all accept. For the x265 fixture with
  `general_profile_compatibility_flags = 0x60000000`, the
  reverse-bit transform yields `0x00000006`, stringified as
  `"6"` in the middle of `hvc1.1.6.L60.B0`. A DASH test suite
  validator would accept either form; the reversed form is the
  one browsers emit in their `canPlayType` probe responses,
  so we match that.
* **Fallback to `avc1.640020`, not `None`**. When no init has
  been pushed yet, the master playlist still needs a CODECS
  attribute (some players refuse the variant without one).
  `avc1.640020` is the session-13 historical default and is
  the right fallback for the dominant RTMP + AVC ingest path.
  HEVC publishers hit the detector the moment their first
  IRAP lands, so the fallback window is measured in
  milliseconds.

### Known gaps explicitly not closed in this session

* **WHEP HEVC egress**. Same shape as item 1 on the session-27
  recommended list: a parallel H265 packetizer path in
  `lvqr-whep::str0m_backend` + SDP re-negotiation + a second
  observer-tap guard lift in `lvqr-whip`. Budget: one session.
* **Real end-to-end WHIP-HEVC-to-HLS-player harness**. The
  integration tests prove the master playlist is correct; a
  full harness that pipes a real client `Rtc` publisher
  through the ingest bridge into the HLS server and then
  reads master.m3u8 + init.mp4 + a media segment back over
  HTTP is a multi-session test harness project.
* **WHIP audio path (Opus sibling track or Opus->AAC re-
  encode)**. Still pending from session-25 recommended item 1.
* **CORS restrictive default, `lvqr-wasm` deletion, criterion
  benches, fuzz catch-up**. All unchanged.

### Recommended entry point (session 28)

Ordered by strategic leverage against the v1.0 M1 milestone.

1. **WHEP HEVC egress**. Closes the second half of the
   session-27 recommended item 1. Work: a new
   `lvqr-whep::str0m_backend` path for `Codec::H265`, an H265
   Annex B -> AVCC converter (inverse of the HEVC
   depacketizer `lvqr-whip::depack` already has), lifting the
   raw-sample observer AVC-only guard in `WhipMoqBridge`, and
   a loopback E2E. Budget: one session. Safari ships HEVC over
   WebRTC natively; Chrome requires an experimental flag but
   the negotiation path is the same.

2. **WHIP audio path** (Tier 2.7 follow-on). Unchanged.

3. **`lvqr-dash` egress** (Tier 2.6). Unchanged. Note that
   the new `detect_video_codec_string` helper lands the first
   piece of the DASH MPD generator's codec attribute; session
   28 or 29 can share it.

4. **Archive VOD playlist rendering**. Unchanged.

5. **Benchmark slots via `criterion`**. Unchanged.

6. **Fuzz slot catch-up**. Unchanged.

7. **`lvqr-wasm` deletion**. Unchanged.

8. **CORS restrictive default**. Unchanged.

### Competitive-matrix delta after session 27

Checkmarks gained:

* **HEVC ingest via WebRTC** through to LL-HLS + archive:
  Partial -> **Y** (MoQ + LL-HLS + archive). WHEP remains the
  one egress crate where HEVC does not yet traverse; tracked
  as session-28 item 1.
* **Bug / hardening credit**: `lvqr-codec::aac::parse_asc`
  explicit-frequency floor tightened.

---
## Session 26 (2026-04-14): Tier 2.7 HEVC through the WHIP bridge

One code change on top of session 25's `fb9d2ac`. Threads an
HEVC code path through the `WhipMoqBridge` so any WebRTC
publisher negotiating H.265 now lands a MoQ broadcast with a
real `hev1` sample entry, using the same sibling-bridge shape
that WHIP/H.264 uses. Zero lines changed in `lvqr-ingest`,
`lvqr-hls`, `lvqr-whep`, `lvqr-archive`, `lvqr-cmaf`, or
`lvqr-moq`: the HEVC path reuses `write_hevc_init_segment`
(already in `lvqr-cmaf` since session 6) and
`lvqr_codec::hevc::parse_sps` unchanged.

### What landed

* **`lvqr-whip/src/depack.rs`** grew three HEVC NAL-type
  constants (`HEVC_NAL_TYPE_VPS = 32`, `SPS = 33`, `PPS = 34`)
  plus a `hevc_nal_type()` helper that extracts the 6-bit
  `nal_unit_type` field from an HEVC NAL header. `split_annex_b`
  is codec-agnostic and was not modified.

* **`lvqr-whip/src/bridge.rs`** gained a public
  `VideoCodec { H264, H265 }` enum stamped on every
  `IngestSample`, a new `BroadcastState.codec` field, a
  codec-aware `ensure_initialized` dispatch, and two new
  builder helpers `build_avc_init` + `build_hevc_init`. The
  HEVC path walks the first keyframe for VPS + SPS + PPS via
  `extract_hevc_params`, parses the SPS through
  `lvqr_codec::hevc::parse_sps` for pixel dimensions +
  profile/tier/level, and calls
  `lvqr_cmaf::write_hevc_init_segment`. The MoQ track fourcc
  on the `FragmentMeta` is `"hvc1"` for HEVC and `"avc1"` for
  H.264.

* **`lvqr-whip/src/str0m_backend.rs`** now calls
  `RtcConfig::new().enable_h264(true).enable_h265(true).enable_opus(true)`
  so the answerer accepts both video codecs in the SDP offer,
  and `forward_video_sample` reads
  `data.params.spec().codec` to map from str0m's negotiated
  payload type back to our `VideoCodec` tag before handing
  the sample off to the bridge. Non-H26x video codecs are
  skipped at this boundary with a trace-level log.

* **Observer fanout is intentionally AVC-only**. HEVC
  publishers do **not** fire the `SharedFragmentObserver`
  (LL-HLS + archive tee) or the `SharedRawSampleObserver`
  (WHEP) paths in session 26. Both downstream surfaces
  currently hard-code `avc1.640020` in their respective
  master-playlist / packetizer code and would advertise a
  wrong codec string if given HEVC init bytes. The HEVC path
  reaches **MoQ subscribers only** via `origin.create_broadcast
  -> 0.mp4`. Threading codec-aware HLS + WHEP is explicitly
  carried forward as a session-27 follow-up.

* **CLI / test-utils wiring**. No changes needed: the CLI
  already routes HEVC-capable `Str0mIngestAnswerer` through
  the same `WhipMoqBridge`. HEVC publishers reach the same
  `--whip-port` as H.264.

### 5-artifact contract status for lvqr-whip (HEVC delta)

| Slot | Status |
|---|---|
| Unit | +4 tests in `src/bridge.rs::tests` covering VPS/SPS/PPS extraction from a real x265 IRAP, the missing-VPS drop path, happy-path HEVC broadcast initialization, and missing-parameter-set keyframe drop. |
| Integration | Unchanged (10 signaling tests in `tests/integration_signaling.rs`). Signaling is codec-agnostic. |
| Proptest | Unchanged (3 properties in `tests/proptest_depack.rs`). Depacketizer walks arbitrary bytes and HEVC NAL types share the same start-code framing. |
| E2E | **New**: `tests/e2e_str0m_loopback_hevc.rs`. Builds a client `Rtc` with **only** `enable_h265(true)` so the negotiated m=video has exactly one payload type, writes real x265 VPS/SPS/PPS + a synthetic IDR_W_RADL NAL through `Writer::write` on every tick, and asserts the capture sink on the server side receives a keyframe whose `codec == VideoCodec::H265` and whose payload re-parses as HEVC NAL units. This is the assertion that would catch a regression in the `data.params.spec().codec` lookup, the HEVC init builder, or str0m's H265 packetizer/depacketizer pair. |
| Fuzz | Still deferred (same nightly-rustc gate as H.264 path). |

### Load-bearing decisions made

* **One MoQ track per broadcast, codec on the track metadata**.
  HEVC publishers still land on the `0.mp4` track slot so every
  downstream MoQ subscriber keeps the same track naming
  convention. Multi-track publishers (simulcast / AV1 fallback)
  would extend this with `1.mp4`, `2.mp4`; not needed yet.

* **Codec learned from `MediaData.params.spec().codec`, not
  from sniffing the NAL header**. str0m's SDP negotiation
  already pins the payload type to a single codec; trusting
  the negotiated value is cheaper and more robust than
  pattern-matching the NAL header byte layout, which differs
  between H.264 (1-byte header, nal_unit_type in low 5 bits)
  and HEVC (2-byte header, nal_unit_type in bits 6..=1 of the
  first byte).

* **HEVC skips the fragment + raw-sample observers, AVC fires
  them**. Honest scoping: introducing HEVC to LL-HLS / WHEP /
  archive requires threading a codec string through each
  egress crate's sample-entry / master-playlist / packetizer
  paths, which would widen the session-26 blast radius far
  beyond `lvqr-whip`. Leaving the observer taps AVC-only
  preserves session 25's Bet-3 invariant that WHIP changes
  touch zero lines in egress crates.

* **x265 fixture bytes duplicated in `bridge.rs::tests` and
  `tests/e2e_str0m_loopback_hevc.rs`**. Same pattern
  `lvqr-cmaf::init::tests` uses; the conformance fixture at
  `crates/lvqr-conformance/fixtures/codec/hevc-sps-x265-main-320x240.bin`
  is the source of truth, and if x265 output drifts, the
  cmaf conformance test catches it first.

### Known gaps explicitly not closed in this session

* **HEVC through LL-HLS / WHEP / archive**. Carried to session
  27. `lvqr-hls::server.rs` hard-codes `"avc1.640020,mp4a.40.2"`
  in the master playlist codecs attribute; threading a real
  codec string through the master playlist builder is the
  first step. `lvqr-whep::str0m_backend` similarly hard-codes
  H.264 RTP packetization; WHEP egress for HEVC requires a
  second packetizer path and SDP re-negotiation.

* **Audio path for WHIP (Opus/AAC)**. Still deferred (was
  session-25 entry point item 1, now still pending).

* **Session cleanup tearing down the MoQ broadcast**. Same
  as session 25: DELETE tears down the UDP socket but the
  `WhipMoqBridge` entry stays alive until CLI shutdown.

### Recommended entry point (session 27)

Ordered by strategic leverage against the v1.0 M1 milestone.
Items renumber by one where session 26 closed them.

1. **HEVC through LL-HLS + WHEP + archive**. The
   publisher-side piece just landed; the egress-side piece is
   the real competitive win. Scope: thread a codec string
   through `lvqr-hls::MasterPlaylistConfig` and
   `lvqr-whep::str0m_backend` per-broadcast, and remove the
   AVC-only observer guards in `WhipMoqBridge::ensure_initialized`
   + `push_sample`. Budget: one session for HLS, one session
   for WHEP (since WHEP needs a parallel H265 packetizer path
   and browser support varies; Safari supports HEVC-over-WebRTC
   natively, Chrome behind an experimental flag).

2. **Audio path for WHIP** (Tier 2.7 follow-on). Unchanged
   from session-25 recommended item 1. Still Opus-native
   sibling `1.mp4` track as the cheap option; AAC re-encode
   as the expensive alternative.

3. **`lvqr-dash` egress** (Tier 2.6). Unchanged from session
   25 item 3.

4. **Archive VOD playlist rendering**. Unchanged.

5. **Benchmark slots via `criterion`**. Unchanged.

6. **Fuzz slot catch-up**. Unchanged.

7. **`lvqr-wasm` deletion**. Unchanged.

8. **CORS restrictive default**. Unchanged.

### Competitive-matrix delta after session 26

Checkmarks gained:

* **HEVC ingest via WebRTC**: N -> Partial (MoQ subscribers
  only; HLS / WHEP / archive still AVC-only, tracked for
  session 27).

Positions LVQR still cannot defend: HEVC through the egress
chain, AV1, Opus through the bridge audio path, DASH egress,
ABR / transcoding, multi-node cluster, SDK surface beyond
`@lvqr/core`, web admin UI.

---
## Session 25 (2026-04-14): Tier 2.7 `lvqr-whip` ingest crate

One code commit on top of session 24's `c1bf179`. Closes the
single biggest column in the Tier A competitive feature matrix:
"any WebRTC client can publish to LVQR". The WHIP bridge is a
sibling of `RtmpMoqBridge` (not an extension of it), fans
fragments through the existing `SharedFragmentObserver` and
`SharedRawSampleObserver` taps, and every existing egress (MoQ,
LL-HLS, WHEP, disk record, DVR archive) picks up WHIP publishers
with zero changes to the egress side.

### What landed

* **New crate `crates/lvqr-whip/`** (workspace member; fuzz dir
  excluded to match the `lvqr-whep/fuzz` pattern):
  * `server.rs`: `WhipServer`, `SdpAnswerer` + `SessionHandle`
    trait boundary, `WhipError` with `IntoResponse` mapping to
    415/400/404/500, `SessionId` as 32-hex random tokens.
  * `router.rs`: axum router rooted at `/whip/{*path}`. `POST`
    creates a session (201 + `Location` header + SDP answer in
    the body), `PATCH` forwards trickle ICE to the session
    handle (204), `DELETE` tears the session down (200). Same
    catch-all `{*path}` split pattern as `lvqr-whep::router` so
    broadcast names can contain `/` (RTMP `{app}/{key}`
    convention). Content-type validation accepts
    `application/sdp` with parameters and
    `application/trickle-ice-sdpfrag` for PATCH.
  * `depack.rs`: Annex B -> AVCC converter. `split_annex_b` walks
    a byte buffer recognising both 3-byte and 4-byte start-code
    forms and returns NAL body slices. `annex_b_to_avcc` wraps
    each body with a big-endian 4-byte length prefix. This is
    the inverse of the AVCC -> Annex B converter at
    `crates/lvqr-whep/src/str0m_backend.rs:430`; both are
    load-bearing boundary crossings between the WebRTC world
    (Annex B) and the Unified Fragment Model (AVCC).
  * `str0m_backend.rs`: `Str0mIngestAnswerer` implements
    `SdpAnswerer` by building a fresh `Rtc` with
    `enable_h264 + enable_opus`, binding a per-session UDP
    socket on the configured host IP, and spawning a sans-IO
    poll task that runs the same canonical
    `poll_output -> select!(shutdown | recv_from | timeout)`
    cycle `lvqr_whep::str0m_backend::run_session_loop` uses.
    The ingest-specific difference is the event arm: on
    `Event::MediaData` for the video mid, we call
    `data.is_keyframe()`, rebase `data.time` to 90 kHz via
    `MediaTime::rebase(Frequency::NINETY_KHZ).numer()` (first
    sample becomes DTS 0), and hand an `IngestSample` off to an
    `Arc<dyn IngestSampleSink>` pumped in at answerer
    construction time. Audio `MediaData` events are dropped
    silently (Opus to AAC transcode is out of scope). Trickle
    ICE logs once per session and returns success; WHIP clients
    rarely need trickle because the offer typically already
    embeds every host candidate.
  * `bridge.rs`: `WhipMoqBridge` holds an `OriginProducer`, a
    `DashMap<String, BroadcastState>`, and optional
    `SharedFragmentObserver` / `SharedRawSampleObserver`
    handles. On the first sample for a broadcast the bridge
    waits for a keyframe that carries SPS + PPS, parses pixel
    dimensions via `h264-reader`, builds an AVC init segment
    via `lvqr_cmaf::write_avc_init_segment`, creates a MoQ
    broadcast + `0.mp4` track, instantiates a `MoqTrackSink`
    with the init segment seeded on the `FragmentMeta`, and
    fires `FragmentObserver::on_init` exactly once. Every
    subsequent sample is converted to AVCC, wrapped in a
    `RawSample`, tapped through the raw observer, wrapped in a
    `moof+mdat` via `build_moof_mdat`, wrapped in a `Fragment`,
    pushed through the sink, and tapped through the fragment
    observer. Non-keyframes arriving before the first
    parameter-set-bearing IDR are dropped (downstream decoders
    can't do anything with them without init anyway). The
    dashmap entry is dropped before invoking the observer to
    avoid a reentrancy footgun: observers that walk back into
    the bridge would deadlock if the entry's shard lock was
    still held.

* **`lvqr-cli` wiring**. New `--whip-port` / `LVQR_WHIP_PORT`
  flag (default 0 = disabled) in `main.rs`. `ServeConfig`
  grows `whip_addr: Option<SocketAddr>` and `ServerHandle`
  gains `whip_addr()`. When `whip_addr` is set, `start()`:
  (1) builds a `WhipMoqBridge` wired to `relay.origin()`, (2)
  hands it a clone of whatever `SharedFragmentObserver` the
  `RtmpMoqBridge` got (HLS + archive tee) and a clone of the
  `WhepServer` as a `SharedRawSampleObserver` when WHEP is
  also enabled, (3) constructs a `Str0mIngestAnswerer` pointed
  at the bridge as an `Arc<dyn IngestSampleSink>`, (4)
  pre-binds the TCP listener (so test harnesses can read the
  ephemeral port back through `ServerHandle::whip_addr`), and
  (5) serves `lvqr_whip::router_for(server)` inside the shared
  background task under the cli's `CancellationToken`. The
  bridge `Arc` is kept alive inside the spawned task (not the
  outer scope) so it lives as long as the poll loops do.

* **`lvqr-test-utils::TestServer`**. `ServeConfig.whip_addr`
  defaults to `None` in `TestServerConfig`, so every existing
  integration test picks up the new field without any other
  changes.

### 5-artifact contract status for lvqr-whip

| Slot | Status |
|---|---|
| Unit (`#[cfg(test)]` in each module) | 18 tests covering the splitter, the AVCC round trip, SPS/PPS extraction, error status mapping, session id uniqueness, keyframe gating, non-keyframe drop, and the answerer's offer accept + reject paths. |
| Integration (`tests/integration_signaling.rs`) | 10 tests driving the real axum router via `tower::ServiceExt::oneshot` with a stub `SdpAnswerer`. Covers content-type validation, 201/Location/answer body on POST, session lifecycle (POST -> DELETE -> 404 on second DELETE), PATCH forwarding to the handle with a counter assertion, unknown-session 404, and method-not-allowed on GET. |
| Proptest (`tests/proptest_depack.rs`) | 3 properties: `split_annex_b` never panics on arbitrary bytes, `annex_b_to_avcc` never panics on arbitrary bytes, and for well-formed multi-NAL Annex B buffers the AVCC round trip preserves every NAL body exactly. |
| E2E (`tests/e2e_str0m_loopback.rs`) | Real in-process end-to-end. A client `str0m::Rtc` builds a sendonly-video offer, the server `Str0mIngestAnswerer` accepts it, both poll loops exchange packets over loopback UDP, complete ICE + DTLS + SRTP, and the client's `Writer::write` pushes synthetic SPS + PPS + IDR samples. The capture sink installed as the `IngestSampleSink` must receive at least one keyframe whose payload re-parses as Annex B NAL units. **This is the test that would catch a regression where `Event::MediaData` routing, rebase arithmetic, or the bridge sink callback silently broke.** |
| Fuzz | Deferred. `crates/lvqr-whip/fuzz` is excluded in the workspace `exclude` list next to `lvqr-whep/fuzz` and `lvqr-codec/fuzz` for the same reason (libfuzzer-sys requires nightly rustc). The proptest slot already covers the DoS-adjacent parser property on 512-byte arbitrary inputs. |

### Load-bearing decisions made

* **Sibling bridge, not an `RtmpMoqBridge` extension**. The RTMP
  bridge's `ActiveStream` is tightly coupled to FLV-parsed
  `VideoConfig` / `AudioConfig` types; extending `ActiveStream`
  to accept a second ingest source would have been a
  cross-cutting refactor touching every code path the bridge
  touches. The session-25 prompt flagged this as the
  load-bearing unknown with a stop-and-document fallback; the
  sibling-bridge approach resolved it without triggering the
  fallback. The composition pattern is the session-24 tee /
  observer-over-widening choice applied one level up: the two
  bridges share the observer traits but not the state machine.
* **Video-only scope**. `Rtc` is built with `enable_h264 +
  enable_opus` so the server accepts Opus sections in the
  offer, but `forward_video_sample` gates on `ctx.video_mid`
  and audio `MediaData` events never reach the sink. A
  follow-up session will land Opus -> AAC or wire an Opus-
  native track through a separate sink.
* **Dropped non-keyframes until the first SPS + PPS IDR**.
  Matches the LL-HLS precedent and avoids emitting fragments
  without a corresponding init segment. The bridge's
  `ensure_initialized` path is the only code that can insert a
  fresh broadcast into the dashmap; `push_sample` is a no-op
  when the entry is missing.
* **DTS rebase to 0 in the poll loop, not the bridge**. Inbound
  `MediaTime` values carry a random-looking wall-clock offset
  (str0m tracks the peer's RTP timestamp base); subtracting
  the first observed value in the poll task keeps the bridge
  shape identical to the RTMP bridge's `base_dts = timestamp *
  90` path and lets the LL-HLS playlist window start at zero.
* **Bridge arc kept alive inside the spawned task**. Dropping
  the only strong reference at the end of `start()` would tear
  down the `DashMap` out from under the session poll loops.
  The `whip_bridge_keepalive` binding is moved into the
  `tokio::spawn` closure and explicitly `drop`ped after
  `tokio::join!`.

### Known gaps explicitly not closed in this session

* **Audio (Opus -> AAC)**. Deferred; session-26 candidate.
* **Trickle ICE ingestion on PATCH**. Same behavior as WHEP:
  logs once, returns success.
* **Session cleanup tearing down the MoQ broadcast**. DELETE
  removes the session from the `WhipServer` registry and
  closes the UDP socket (via the oneshot drop), but the
  `WhipMoqBridge` entry for that broadcast stays alive until
  the cli-level bridge `Arc` is dropped at process shutdown.
  Fine for v0 single-publisher scenarios; tracked for a
  follow-up when multi-session-per-broadcast lands.
* **Fuzz slot**. Same nightly-rustc gate the rest of the crates
  hit; the proptest slot is the session-25 substitute.

### Recommended entry point (session 26)

Ordered by strategic leverage against the v1.0 M1 milestone. The
session-24 entry-point list is otherwise unchanged; items
renumber by one because Tier 2.7 WHIP closed as item 1.

1. **Audio path for WHIP** (Tier 2.7 follow-on). Two choices:
   (a) wire an Opus-native sibling `1.mp4` track so the bridge
   emits an Opus init segment + per-frame Opus fragments
   through the same MoQ broadcast without any transcode, or
   (b) ship an AAC re-encoder so the WHIP publisher surface
   reaches the same AAC-only egress stack RTMP uses. Option
   (a) is cheaper and unlocks `@lvqr/player` audio for every
   browser that already speaks Opus; option (b) is needed only
   if a consumer on the egress side refuses Opus. Budget: one
   session for (a), two sessions for (b).

2. **HEVC end-to-end through the bridge** (Tier 2.2 / 2.3
   follow-on, same text as session 24 item 2 but with the new
   cost calculus). With WHIP landed, HEVC over WebRTC is the
   cheap path: `str0m`'s `enable_h265` gate and a parallel
   depacketizer + init builder in `lvqr-whip` close the loop
   without touching RTMP. An enhanced-RTMP-HEVC parser remains
   the alternative for FLV clients. Budget: one session either
   way.

3. **`lvqr-dash` egress** (Tier 2.6). Unchanged from session 24.
   Aligned CMAF segments already produced by `lvqr-cmaf`; the
   missing piece is a typed MPD generator via `quick-xml`.

4. **Archive VOD playlist rendering**. Unchanged from session
   24 item 4.

5. **Benchmark slots via `criterion`**. Unchanged from session
   24 item 5. Still zero benches in the workspace.

6. **Fuzz slot catch-up**. Same five in-scope crates:
   `lvqr-record`, `lvqr-moq`, `lvqr-fragment`, `lvqr-cmaf`,
   `lvqr-hls`. `lvqr-whip` inherits the deferred-fuzz pattern
   from `lvqr-whep`.

7. **`lvqr-wasm` deletion**. Unchanged from session 24.

8. **CORS restrictive default**. Unchanged from session 24.
   Now covers the WHIP surface too via the same permissive
   admin layer (WHIP serves on its own port behind its own
   router, so the CORS change only touches the admin layer).

### Strategic-bet validation after session 25

* **Bet 1 (MoQ wins browser-origin live video)**: on track.
* **Bet 2 (Rust memory safety + perf)**: validated. 78 test
  binaries, 334 tests, 0 panics, 1 debt comment (whep trickle),
  all integration tests run real network I/O.
* **Bet 3 (unified fragment model projects cleanly)**: **very
  strongly validated**. WHIP landed a full ingest path touching
  SDP signaling + sans-IO poll loop + RTP depacketization +
  Annex B/AVCC conversion + SPS/PPS init-segment construction
  + MoQ fanout + LL-HLS + DVR archive + raw-sample observer,
  and it changed **zero lines** in `lvqr-ingest`, `lvqr-hls`,
  `lvqr-cmaf`, `lvqr-fragment`, `lvqr-moq`, `lvqr-archive`, or
  `lvqr-whep`. Every downstream egress picks up the new ingest
  source through cloneable observer traits. That is the
  "publisher crates under 500 lines, egress unchanged"
  predicate from `AUDIT-2026-04-13.md` Bet 3, realised on both
  sides of the data plane.
* **Bet 4 (cross-node MoQ relay-of-relays)**: untested. Tier 3.
* **Bet 5 (WASM filters + in-process AI agents)**: not started.

### Competitive-matrix delta after session 25

Checkmarks gained since `AUDIT-2026-04-13.md`:

* **WHEP egress**: N -> Y (session 22).
* **LL-HLS egress**: N -> Y (sessions 13 + 17).
* **DVR scrub**: N -> Y (session 24).
* **Archive index**: N -> Y (sessions 23 + 24).
* **JWT auth**: P -> Y.
* **WHIP ingest**: N -> Y (**session 25**).

Positions LVQR still cannot defend: HEVC / AV1 / Opus through
the bridge audio path, DASH egress, ABR / transcoding,
multi-node cluster, SDK surface beyond `@lvqr/core`, web admin
UI.

---
## Project Status: v0.4-dev -- Tier 2.4 archive real, gated by SharedAuth (session 24 snapshot)

**Tests at session-24 close**: 63 test binaries, 302 individual
tests, 1 ignored doctest. Superseded by session-25 snapshot
above; kept for historical continuity.

## Session 24 (2026-04-14): Tier 2.4 writer, playback endpoints, and auth gate

Four code commits plus two doc commits on top of session 23's
baseline. Closes session-23 recommended items 1 and 2 **and** the
admin-port exposure footgun that opening `/playback/*` would
otherwise create. The archive index now has a real writer feeding
it, a real JSON query surface reading it back, a real `latest`
anchor, a real byte-serving endpoint with a path-traversal guard,
**and** every playback route honors the same `SharedAuth`
subscribe gate the WS relay uses. DVR scrub is end-to-end through
a full RTMP publish under both open and token-protected auth.

### Commits

* **7c84344** -- Tier 2.4: archive writer integration + playback
  HTTP endpoints. `IndexingFragmentObserver` in
  `crates/lvqr-cli/src/archive.rs` captures the track timescale
  from `FragmentObserver::on_init` (already carried since session
  18), then on every `on_fragment` call spawns a blocking task
  that writes the fragment payload to
  `<archive_dir>/<broadcast>/<track>/<seq>.m4s` and records a
  `SegmentRef` against a shared `Arc<RedbSegmentIndex>`.
  `TeeFragmentObserver` composes the archive observer with the
  LL-HLS bridge without widening the ingest bridge's single-
  observer slot. `ServeConfig` grows `archive_dir: Option<PathBuf>`;
  `--archive-dir` / `LVQR_ARCHIVE_DIR` wired through `lvqr-cli::
  main`. `TestServerConfig::with_archive_dir` lets integration
  tests opt in; `loopback_ephemeral` leaves it `None` so the
  pre-existing tests are untouched. `GET /playback/{*broadcast}?
  track=&from=&to=` on the admin axum router returns a JSON array
  of `PlaybackSegment` rows sharing the same `Arc<RedbSegmentIndex>`
  as the writer (avoiding redb's exclusive-file lock), with the
  sync scan running under `spawn_blocking`. `GET /playback/latest/
  {*broadcast}?track=` returns the single most-recent row or 404,
  declared before the catch-all so axum specificity wins. New
  `crates/lvqr-cli/tests/rtmp_archive_e2e.rs` publishes a real
  two-keyframe RTMP stream into a temp archive dir via
  `TestServer` and asserts (1) the range query returns sorted
  non-empty rows whose paths point at real files whose lengths
  match the recorded byte counts and whose first box is `moof`,
  (2) a future window returns an empty array, (3) an unknown
  broadcast returns an empty array, (4) the `latest` endpoint
  matches the last entry of the range scan, and (5) `latest` on
  an unknown broadcast returns 404. After shutdown the test
  reopens the redb file directly for a second round of on-disk
  assertions against `SegmentIndex::latest`.
* **4abc74c** -- docs: session 24 HANDOFF notes (this section's
  initial shape, later extended in-place by the drift-closure
  commit below).
* **94586f9** -- Tier 2.4: playback surface honors SharedAuth
  subscribe gate. `ArchiveState` grows an `auth: SharedAuth`
  field; the three `/playback/*` handlers now extract a bearer
  token via `Authorization: Bearer` (header) or `?token=`
  (query fallback) and call `SharedAuth::check(AuthContext::
  Subscribe{..})`. Denials return 401 and increment
  `lvqr_auth_failures_total{entry="playback"}`. `file_handler`
  derives its broadcast auth key from the first two path
  components of `rel` via a new `broadcast_from_rel` helper
  (layout is `<broadcast>/<track>/<seq>.m4s` where `track`
  matches `N.mp4`), so a JWT with `sub:live/dvr` authorizes
  every archived segment under that stream's subtree without
  authorizing sibling streams. No new CLI flag: with
  `NoopAuthProvider` the archive stays open (matching the LL-HLS
  precedent); with `--subscribe-token` or `--jwt-secret` set,
  the archive inherits the same credential as live subscribe.
  New `playback_surface_honors_shared_auth` integration test
  installs a `StaticAuthProvider` with `subscribe_token =
  Some("s3cr3t")` and asserts: unauthenticated GETs on all
  three routes return 401; authenticated GETs via header return
  200 with populated bodies; the `?token=` fallback works for
  all three routes; a wrong token still 401s. The pre-existing
  `rtmp_publish_populates_archive_index` test runs under the
  default `NoopAuthProvider` and still passes unchanged,
  confirming the "open when auth is open" property.

### Earlier session-24 commits (chronological)

* **5ccfd97** -- Tier 2.4: archive file-serve endpoint with
  traversal guard. `GET /playback/file/{*rel}` on the admin router
  joins `rel` onto the configured archive directory, canonicalizes
  the result, and rejects requests that escape the canonicalized
  archive root with 400 (or 404 when the file simply does not
  exist). `ArchiveState` carries `(dir, canonical_dir, index)` so
  the guard runs in constant time per request; `FromRef<
  ArchiveState>` keeps the pre-existing `find_range` and `latest`
  handler signatures unchanged. `application/octet-stream` response
  body with an explicit `Content-Length`. Integration test gains
  four assertions: valid fetch returns a `moof`-prefixed body,
  missing-file 404, percent-encoded `..` traversal stays inside
  the archive root (400 or 404, never leaks outside bytes), and
  the range + latest routes still work after the state refactor.

### Archive HTTP surface (admin router, when `--archive-dir` is set)

| Route | Behaviour |
|---|---|
| `GET /playback/{*broadcast}?track=&from=&to=` | JSON array of `PlaybackSegment` rows overlapping `[from, to)`, ordered by `start_dts`. Defaults `track=0.mp4`, `from=0`, `to=u64::MAX`. |
| `GET /playback/latest/{*broadcast}?track=` | Single most-recent row or 404. Declared before the catch-all so axum specificity wins. |
| `GET /playback/file/{*rel}` | Raw fragment bytes. Path traversal guarded via canonicalized-root prefix check. |

### Load-bearing investigation resolved

The session-23 HANDOFF flagged a load-bearing unknown: does
`lvqr_fragment::Fragment` already carry `(start_dts, end_dts,
keyframe, timescale)` or does the coalescer need to widen
`FragmentObserver::on_fragment`? Reading
`crates/lvqr-fragment/src/fragment.rs` resolved the question: the
`Fragment` type already carries `dts`, `duration` (→ `end_dts =
dts + duration`), `flags.keyframe`, and `track_id`. `on_init` was
already extended in session 18 to carry the per-track
`timescale`. No observer-signature change was required; the
stop-and-document fallback was not triggered.

### Design decisions

* **Own the on-disk writer inside the observer**, option (a) from
  the pre-coding plan. `lvqr-record` is untouched and keeps its
  MoQ-consumer writer; the archive is a fully self-contained data
  path hanging off the `FragmentObserver` tap. Two independent
  writers, one canonical index. Considered option (b) (have the
  index point at files produced by `lvqr-record`) and rejected it
  because it would have coupled two asynchronous writers that
  already disagree on segmentation boundaries.
* **Tee observer in `lvqr-cli`, not a bridge-API widening**. The
  ingest bridge keeps its single `observer: Option<
  SharedFragmentObserver>` slot. `lvqr-cli::start` composes
  `HlsFragmentBridge` and `IndexingFragmentObserver` into a
  `TeeFragmentObserver` when both are enabled; single observer
  otherwise. No churn in `lvqr-ingest`.
* **Shared `Arc<RedbSegmentIndex>` between writer and HTTP
  handlers**. redb takes an exclusive file lock, so a separate
  read-only open would race the writer. The playback router
  borrows the same Arc the `IndexingFragmentObserver` holds;
  `spawn_blocking` wraps the sync scan so the admin runtime stays
  responsive.
* **Per-fragment granularity, not per-segment**. The bridge emits
  one `moof+mdat` Fragment per video NAL / per AAC access unit,
  so the index granularity matches the smallest addressable media
  unit. Range scans still return rows ordered by `start_dts`. A
  future coalescing writer that packs multiple samples into a
  single file is supported by the `byte_offset` + `length` fields
  on `SegmentRef` but not used today.

### What is still not real

* **Rotation / compaction / quota**. The archive grows unbounded.
  `lvqr-record` has the same property; this is Tier 3 work.
* **S3 / object-store upload and cross-node replication**. Also
  Tier 3; the single-writer `Arc<Database>` shape would need to
  move behind an opaque interface first.
* **Playlist rendering from the archive**. The HANDOFF-23
  suggestion of "JSON `[SegmentRef, ...]` or an LL-HLS playlist
  window over the matched segments" landed only the JSON half.
  A `/playback/{*broadcast}/playlist.m3u8` that renders an HLS
  window over archived rows is a natural follow-up once the
  `lvqr-hls` playlist builder grows a non-live mode (no
  `EXT-X-PLAYLIST-TYPE:VOD` / `EXT-X-ENDLIST` support exists
  today; verified by grep over `crates/lvqr-hls/src/`).
* **Archive CORS**. The playback surface inherits the admin
  router's `CorsLayer::permissive()` default, so browser clients
  on any origin can read DVR segments when auth is open. Tracked
  alongside the admin CORS restrictive default in the session 25
  entry point list.
* **`lvqr-archive` crate-level integration test slot**. The
  real integration test lives in `crates/lvqr-cli/tests/
  rtmp_archive_e2e.rs`, not `crates/lvqr-archive/tests/`. The
  5-artifact contract does not check `lvqr-archive` yet (it is
  still commented out in `scripts/check_test_contract.sh`
  IN_SCOPE), so the placement is forward-compatible; when the
  contract is enabled for the crate, a thin `tests/
  archive_integration.rs` that exercises `RedbSegmentIndex`
  directly will close the slot without duplicating the cli-level
  round trip.

### Cumulative commit range since session 19

Sessions 20 through 24 landed thirteen commits on top of `f50cc4f`:
`580d152`, `db9fd10`, `ddcb599`, `ed9c6e3`, `8f30e8f`, `c0d474f`,
`cbffab9`, `939e743`, `7c84344`, `4abc74c`, `5ccfd97`, `8fa06d6`,
`94586f9`, plus `65cdf64` (session-24 drift closure v2), plus
this closing-audit drift-fix commit. `origin/main` tracks the
latest. Session 25 starts from there.

### Session 24 closing audit (2026-04-14)

A deep audit of the whole repo + tracking plans, driven by a
ground-truth test run and an exhaustive grep for debt markers,
stubs, and strategic drift. Every claim below was verified
against the tree at `65cdf64` immediately before this edit.

**What LVQR can do right now.** Five live egress projections plus
one DVR surface from a single `lvqr serve --archive-dir=/data`:
(1) RTMP -> MoQ QUIC/WebTransport; (2) RTMP -> WebSocket fMP4;
(3) RTMP -> LL-HLS multi-broadcast with master playlist + audio
rendition group; (4) RTMP -> WHEP video via str0m (ICE/DTLS/SRTP,
H.264 RTP packetization, real browser-compatible); (5) RTMP ->
per-track fMP4 disk recording via `lvqr-record`; (6) the new
Tier 2.4 archive path: `IndexingFragmentObserver` populates a
redb segment index, and `GET /playback/{*broadcast}`, `GET
/playback/latest/{*broadcast}`, `GET /playback/file/{*rel}` all
honor the same `SharedAuth::check(AuthContext::Subscribe{..})`
gate used by the WS relay.

**Code-health snapshot.** 19 crates under `crates/`. 63 test
binaries, 302 passing tests, 0 failures, 1 intentionally
`ignore`d doctest. **0** `todo!()` / `unimplemented!()` macros
in the entire tree. **1** `TODO`/`FIXME`/`XXX`/`HACK` comment
total: `crates/lvqr-whep/src/str0m_backend.rs:459` referring to
trickle-ICE ingestion, which is correctly deferred. `cargo
clippy --workspace --all-targets -- -D warnings` clean. `cargo
fmt --all --check` clean. MSRV pinned to `rust-version = "1.85"`
at the workspace level and inherited by every crate.

**Tier completion (measured against `tracking/ROADMAP.md`):**

| Tier | Status |
|---|---|
| Tier 0 (audit-finding fixes) | Closed, pre-session 16 |
| Tier 1 (test infrastructure) | ~70%. 5-artifact contract script ships soft-fail; 5 in-scope crates missing fuzz slots (`lvqr-record`, `lvqr-moq`, `lvqr-fragment`, `lvqr-cmaf`, `lvqr-hls`); no `lvqr-loadgen` / `lvqr-chaos` crates; no MediaMTX comparison harness; no soak rig; no `benches/` anywhere in the workspace. |
| Tier 2 (unified data plane + protocol parity) | ~60% by surface area, ~70% by user-visible capability. 2.1 `lvqr-moq` facade + `lvqr-fragment` real. 2.2 `lvqr-codec` real for HEVC SPS + AAC ASC, missing VP9 / AV1 / Opus. 2.3 `lvqr-cmaf` real for AVC init+media and AAC init+media; HEVC init writer real (`write_hevc_init_segment`, x265-captured tests) but no bridge path produces HEVC fragments because RTMP carries AVC. 2.4 `lvqr-archive` + playback routes + auth gate -- **landed this session**. 2.5 `lvqr-hls` LL-HLS real. 2.6 `lvqr-dash` **not started**. 2.7 `lvqr-whep` real for video; `lvqr-whip` **not started**. 2.8 `lvqr-srt`, 2.9 `lvqr-rtsp` **not started**. 2.10 single-binary default real except for the missing protocols. |
| Tier 3 (cluster / DVR UI / operational) | ~5%. Only the DVR scrub primitive (archive index + JSON endpoint) exists; everything else is untouched. |
| Tier 4 (differentiation moats) | 0%, correctly deferred. |
| Tier 5 (ecosystem / SDKs / docs site) | 0%, correctly deferred. |

**Strategic bet validation (from `tracking/AUDIT-2026-04-13.md`
section "The Five Strategic Bets"):**

* **Bet 1 (MoQ wins browser-origin live video)**: on track; facade
  real, upstream churn absorbed, Cloudflare + Twitch remain the
  only other production consumers.
* **Bet 2 (Rust memory safety + perf is worth the ergonomic hit)**:
  validated. 0 panic stubs, 1 debt comment across the tree, all
  integration tests exercise real network I/O, no mocks.
* **Bet 3 (unified fragment model projects cleanly)**: **strongly
  validated by this session.** The archive observer consumes the
  same `FragmentObserver::on_fragment` hook LL-HLS uses, with zero
  modifications to the bridge, the observer signature, or any
  other crate. That is the "publisher crates each end up under 500
  lines" predicate from AUDIT section Bet 3. Four egress
  projections (MoQ, WS fMP4, LL-HLS, WHEP) plus the archive
  observer now share one producer side with zero special cases.
* **Bet 4 (cross-node MoQ relay-of-relays)**: untested. Tier 3.
* **Bet 5 (WASM filters + in-process AI agents)**: not started.
  Tier 4, correctly deferred.

**Competitive-matrix delta since `AUDIT-2026-04-13.md` was
written** (pre-session 16). LVQR checkmarks gained:

* **WHEP egress**: N -> Y (session 22).
* **LL-HLS egress**: N -> Y (sessions 13 + 17).
* **DVR scrub**: N -> Y (session 24).
* **Archive index**: N -> Y (sessions 23 + 24).
* **JWT auth**: P -> Y (feature-gated; wired through every entry
  point including the new playback routes).

Positions LVQR still cannot defend: any WebRTC ingest (no WHIP),
HEVC / AV1 / Opus in the bridge path, DASH egress, ABR /
transcoding, multi-node cluster, SDK surface beyond `@lvqr/core`,
and a web admin UI.

**Drift caught and either fixed or filed.**

1. Session-24 HANDOFF + README both claim "73 test binaries, 298+
   tests". Ground truth on `65cdf64` is **63 binaries, 302
   tests**. Fixed in this commit. This is the second drift event
   of session 24 (the first was the footgun-warning staleness
   that `65cdf64` already closed); future sessions should re-run
   `cargo test --workspace 2>&1 | grep -c "^running [0-9]\+ tests$"`
   after any crate-level change and treat both the binary and
   test counts as load-bearing in the status header.
2. **No `benches/` anywhere in the workspace** despite Roadmap
   decision naming `criterion` as validated and Tier 1 listing
   benchmarks as a deliverable. Filed as a session-25 entry point
   item. A single `lvqr-fragment::MoqTrackSink::push` bench + a
   `lvqr-cmaf::build_moof_mdat` bench would catch 80 % of the
   data-plane regression classes for near-zero effort.
3. **`lvqr-archive` test contract placement**: the integration
   test lives at `crates/lvqr-cli/tests/rtmp_archive_e2e.rs`, not
   inside `crates/lvqr-archive/tests/`. When
   `scripts/check_test_contract.sh` flips to strict mode and adds
   `lvqr-archive` to `IN_SCOPE`, it will warn about a missing
   integration slot. Forward-compatible risk; not urgent.
4. **Playwright `tests/e2e/test-app.spec.ts`** is shell-only (66
   lines, asserts page load + nav tab visibility). The file
   comment pins "Tier 2 will extend the config to spawn a real
   lvqr binary"; Tier 2 is largely done and the extension never
   happened. Filed for a later session.
5. `tracking/AUDIT-2026-04-13.md` feature matrix predates sessions
   16-24 and is missing at least five checkmarks LVQR has gained
   since. A refresh-in-place (not a rewrite) is a short docs
   session when the next Tier 2 protocol lands.

**Honest v1.0 gap.** The M1 milestone from Roadmap Tier 2.10 -- "a
fresh user can `cargo install lvqr-cli`, run `lvqr serve --demo`,
ingest from OBS via RTMP/WHIP/SRT/RTSP, and play back via HLS /
LL-HLS / DASH / WHEP / MoQ in a browser" -- is blocked on
`lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, `lvqr-dash`, and a `--demo`
mode that self-signs certs and prints a public URL. Every other
M1 component is real. Session 25's highest-leverage pick is
`lvqr-whip` because it unblocks WebRTC ingest (the single
largest column of N's in the feature matrix) and is a mirror of
the existing `lvqr-whep` shape: str0m is already in the build,
the fragment producer side is a few hundred lines.

### Recommended entry point (session 25)

Ordered by strategic leverage against the v1.0 M1 milestone, not
by risk. The first three items each close a concrete hole in the
competitive matrix; items 4-8 are infrastructure and hygiene
that can slot in between feature sessions or when blocked.

1. **`lvqr-whip` ingest (Tier 2.7)**. The single biggest hole in
   the competitive matrix: "any WebRTC client can publish to
   LVQR". `str0m` is already in the build and the existing
   `lvqr-whep` `Str0mAnswerer` gives a shape to mirror. The
   session should build a `WhipServer` axum router on
   `--whip-port`, accept `POST /whip/{broadcast}` SDP offers,
   run an `Rtc` through the same sans-IO poll loop, and turn
   inbound RTP into `Fragment` values the RTMP bridge can
   swallow via the same `FragmentObserver` / `RawSampleObserver`
   surface that every other ingest uses. Budget: one full
   session. Side effect: once WHIP is live, HEVC and Opus over
   WebRTC automatically flow through the pipeline, so item 2
   below partially closes for free.

2. **HEVC end-to-end through the bridge (Tier 2.2 / 2.3
   follow-on)**. `lvqr-codec::hevc` parses SPS and
   `lvqr-cmaf::write_hevc_init_segment` writes a real `hev1`
   init segment (x265 captures are pinned in the test suite).
   The gap is that no ingest path emits HEVC `Fragment` values:
   RTMP's FLV tag parser is AVC-only. Two viable closures: (a)
   add enhanced-RTMP-HEVC support in `lvqr-ingest/remux` and
   ship a matching FLV parser extension, or (b) wait for
   `lvqr-whip` (item 1) and accept HEVC via WebRTC. Option (b)
   is cheaper if item 1 lands first. Budget: one session if
   item 1 has already landed; two if not.

3. **`lvqr-dash` egress (Tier 2.6)**. Aligned CMAF segments are
   already produced by `lvqr-cmaf`; the missing piece is a
   typed MPD generator via `quick-xml`. Closes the last
   mainstream egress protocol. Budget: one long session or two
   short ones per the Roadmap's 1.5-week estimate. Unblocks a
   real feature-matrix column.

4. **Archive VOD playlist rendering**. `GET /playback/{*broadcast}/
   playlist.m3u8?from=&to=` that walks the archive rows and
   renders a VOD HLS playlist with `EXT-X-PLAYLIST-TYPE:VOD`,
   `EXT-X-MAP`, one `EXTINF` per row, and `EXT-X-ENDLIST`.
   Requires `lvqr-hls` to grow a non-live builder (no
   `PLAYLIST-TYPE:VOD` / `EXT-X-ENDLIST` path exists today; the
   session-24 audit `grep` confirmed this). Budget: one session
   for the VOD builder + one for the integration. Closes the
   HANDOFF-23 "or an LL-HLS playlist window" suggestion.

5. **Benchmark slots via `criterion`**. Add
   `crates/lvqr-cmaf/benches/build_moof_mdat.rs` and
   `crates/lvqr-fragment/benches/moq_track_sink.rs` as the
   first two benches in the workspace. Roadmap decision lists
   `criterion` as validated; no crate currently uses it. The
   session-24 audit caught this drift; a 30 % throughput
   regression on the data-plane hot path would currently go
   unnoticed. Budget: half a session.

6. **Fuzz slot catch-up** for the five in-scope crates that
   `scripts/check_test_contract.sh` reports missing: `lvqr-record`,
   `lvqr-moq`, `lvqr-fragment`, `lvqr-cmaf`, `lvqr-hls`. Each
   needs one `fuzz_targets/*.rs` seeded from the conformance
   corpus. Budget: one session to add all five plus a CI job
   update.

7. **`lvqr-wasm` deletion**. Mechanical one-commit removal of
   the deprecated crate + its workspace member + any CI wasm
   job. Unblocks a cleaner crate table in `README.md` and
   reduces the "what ships" confusion the session-19 audit
   sweep flagged. Budget: half a session.

8. **CORS restrictive default**. Replace
   `CorsLayer::permissive()` in `crates/lvqr-cli/src/lib.rs`
   with an allow-list default (admin origin + localhost) plus
   a `--cors-allow-origin` flag. Breaking change; verify the
   playwright test in `tests/e2e/test-app.spec.ts` does not
   depend on permissive before flipping the default. Ship with
   a release note. Now also covers the archive surface since
   `/playback/*` sits under the same permissive layer.

**Deferred (not yet ready to start)**: `lvqr-srt` (libsrt FFI,
weeks of work, gated on Tier 1 load infrastructure), `lvqr-rtsp`
(hand-rolled state machine, weeks), cluster (Tier 3.1, four
weeks, needs `lvqr-loadgen` first), all Tier 4 items (WASM
filters, C2PA, federation, AI agents).

## Session 23 (2026-04-14): Tier 2.4 start -- lvqr-archive segment index

One commit on top of session 22's baseline. Opened Tier 2.4 by
landing the load-bearing primitive every DVR feature will build
on: a redb-backed segment index that answers "give me every
segment for (broadcast, track) whose decode extent overlaps
[query_start, query_end)" in sorted order.

### Commits

* **c0d474f** -- New `crates/lvqr-archive/` crate. `SegmentRef`
  value type (broadcast, track, segment_seq, start_dts, end_dts,
  timescale, keyframe_start, byte_offset, length, path).
  `SegmentIndex` trait with `record`, `find_range`, `latest`.
  `RedbSegmentIndex` impl using redb 2.6 with a hand-rolled
  compound key layout ([broadcast_len u16_be][broadcast]
  [track_len u16_be][track][start_dts u64_be]) so byte order
  equals numeric order within a (broadcast, track) prefix and
  range scans hit a single contiguous sweep. Value body is a
  hand-rolled binary format to avoid pulling bincode or postcard
  into the workspace. 20 unit tests covering round-trip
  encoding, rejection of corrupt / truncated / oversized inputs,
  key ordering within and across streams, range lookup with
  leading-segment inclusion when the query window starts inside
  a segment, track/broadcast boundary isolation, database reopen
  persistence, and duplicate-key idempotent overwrite.
* **cbffab9** -- this HANDOFF entry.

### Cumulative commit range since session 19

Sessions 20 through 23 landed seven commits on top of `f50cc4f`:
`580d152`, `db9fd10`, `ddcb599`, `ed9c6e3`, `8f30e8f`, `c0d474f`,
`cbffab9`. `origin/main` is at `cbffab9`. Session 24 starts from
there.

### Scope deliberately kept out of this session

* **Writer integration**. `lvqr-record` still writes segments
  without populating the index. The next session wires the
  `FragmentObserver::on_fragment` hook on the bridge to emit
  `SegmentRef` rows alongside the existing segment writes.
* **HTTP playback endpoint**. The `GET /playback/{broadcast}?
  from=&to=` surface is a follow-up once the writer integration
  has populated real rows.
* **Rotation / compaction / S3 upload / cross-node replication**.
  All follow-ups once the data model survives the writer
  integration.

### Recommended entry point (session 24)

1. **Writer integration**: extend `lvqr-record` or add an
   `IndexingFragmentObserver` in `lvqr-cli` that reads
   `FragmentObserver::on_fragment` and calls
   `RedbSegmentIndex::record` with the segment's filesystem path,
   dts range, and keyframe status. Load-bearing question: does
   `lvqr_fragment::Fragment` already carry the start_dts +
   end_dts of its samples, or does the coalescer need to surface
   them on the observer API? Budget: one session.
2. **HTTP playback endpoint** landing on the existing admin
   axum router: `GET /playback/{broadcast}?from={dts}&to={dts}`
   returning JSON `[SegmentRef, ...]` or an LL-HLS playlist
   window over the matched segments. Budget: one session.
3. Continue with the remaining three highest-leverage items from
   the session-22 maturity report: HEVC+Opus end-to-end through
   the bridge, WHIP ingest, and DASH egress.



## Session 22 (2026-04-14): str0m-backed WHEP end-to-end

Closed the entire WHEP media write arc in four commits on top of
session 19's baseline. By the end of the session the
RTMP -> WHEP -> WebRTC client path is real: ICE, DTLS, SRTP all
complete, video samples from the ingest bridge flow through
`Str0mSessionHandle::on_raw_sample` into the sans-IO poll loop,
get AVCC-to-Annex-B converted, handed to
`str0m::media::Writer::write` with the negotiated H.264 `Pt`, and
arrive as decoded `Event::MediaData` events on a str0m-based
client driven in-process by the E2E test.

### Commits (origin/main)

1. **580d152** -- `Str0mAnswerer` implementing `SdpAnswerer` behind
   the session-16 trait boundary; sans-IO poll loop per session
   spawned as a tokio task owning `Rtc` + `tokio::net::UdpSocket`;
   `oneshot` shutdown on handle Drop; ICE + DTLS completes
   against real browsers. `--whep-port` CLI flag (env
   `LVQR_WHEP_PORT`, default 0 = disabled) wired into
   `lvqr-cli::start` with `WhepServer` clone attached as
   `SharedRawSampleObserver` on the bridge builder before the
   `Arc` freeze. `TestServer` sets `whep_addr: None` so the 200+
   existing integration tests are untouched.
2. **db9fd10** -- `cargo fuzz` slot for `SdpOffer::from_sdp_string`
   (the first untrusted-input entry point on the WHEP POST path).
   Initial media-write design note with four load-bearing
   decisions. Decision 2 in that initial note was wrong and is
   corrected inline by commit ddcb599.
3. **ddcb599** -- Video media write via `str0m::media::Writer`.
   `SessionMsg::Video` pumped from `on_raw_sample` over an
   `mpsc::UnboundedSender`; `SessionCtx` captures `video_mid`
   (from `Event::MediaAdded`), `video_pt` (lazy via
   `Writer::payload_params` filtered on `Codec::H264`), and a
   `connected` flag (from `Event::Connected`). AVCC -> Annex B
   converter at the boundary because str0m's `H264Packetizer`
   scans for Annex B start codes and silently drops AVCC input;
   six unit tests cover single NAL, multi NAL, empty, truncated,
   overrun, and zero-length entries.
4. **ed9c6e3** -- In-process str0m loopback E2E test
   (`crates/lvqr-whep/tests/e2e_str0m_loopback.rs`) spinning up a
   client `Rtc` + `Str0mAnswerer` over loopback UDP, completing
   ICE + DTLS + SRTP in-process, pushing synthetic SPS + PPS +
   IDR samples into the server via `on_raw_sample`, and asserting
   `Event::Connected` + at least one `Event::MediaData` on the
   client side. Runs in ~0.15-0.18s wall time, 10/10 green on a
   local flakiness smoke run. Slot 4 (E2E) of the 5-artifact test
   contract for lvqr-whep is now closed. `tests/CONTRACT.md`
   updated to reflect proptest + fuzz + integration + E2E all
   shipped; only conformance (cross-impl against
   `simple-whep-client`) remains open.

### Important session-22 finding

The session-21 design note (`crates/lvqr-whep/docs/media-write.md`)
claimed str0m's `H264Packetizer` accepts AVCC passthrough. Reading
`src/packet/h264.rs` in step 1 of the execution plan revealed the
opposite: it scans for Annex B start codes via `next_ind`, and an
AVCC buffer has none, so the whole buffer gets handed to `emit`,
where the length-prefix high byte is read as a NAL header of type
0 and silently dropped. A naive build would complete ICE + DTLS +
SRTP and then emit zero packets with no error anywhere. The
boundary converter `avcc_to_annex_b` lives at
`crates/lvqr-whep/src/str0m_backend.rs`; the design note is
corrected in place.

### What is real after session 22

* **RTMP -> fMP4 -> MoQ -> browser**: real, unchanged.
* **RTMP -> CMAF -> LL-HLS -> browser**: real, unchanged.
* **RTMP -> RawSample -> `Str0mAnswerer` -> WebRTC client**: real
  end-to-end. The in-process E2E test exercises every byte of
  this path except the public internet and a real browser's
  H.264 decoder. A real browser connecting to `--whep-port`
  should see video decode once it negotiates a compatible H.264
  profile, though the real-browser leg is not yet automated in
  CI (gated on `simple-whep-client` packaging).

### What is still not real

* **Audio over WHEP**. RTMP ingest carries AAC, WHEP negotiated
  Opus. No in-tree AAC -> Opus transcoder. One-shot warn on first
  audio sample, then silent drop. Permanently deferred; see
  `crates/lvqr-whep/docs/media-write.md` for rationale.
* **Trickle ICE ingestion**. `Str0mSessionHandle::add_trickle`
  still one-shot warns and returns success. WHEP rarely needs
  trickle once the offer embeds every host candidate.
* **Real-browser E2E in CI**. Gated on packaging a WHEP client
  binary (`simple-whep-client` or a `webrtc-rs` thin wrapper)
  into the CI image.
* **CORS restrictive default** and **`lvqr-wasm` deletion**:
  still correctly deferred audit items.

### Recommended entry point (session 23)

1. **Real-browser E2E via `simple-whep-client`** soft-skip,
   landing slot 5 (conformance) of the test contract for
   lvqr-whep. Closes every slot of the 5-artifact contract for
   this crate. Requires the CI image to carry the binary.
2. **Tier 2.4 start: archive + redb index**. Gate for Tier 3.
3. **CORS restrictive default** (`crates/lvqr-cli/src/lib.rs`
   `CorsLayer::permissive()` replacement). One-commit breaking
   change with a release note.
4. **`lvqr-wasm` deletion**. One-commit mechanical removal.
5. **Keyframe request handling**. WebRTC PLI / FIR feedback from
   the client should eventually trigger an upstream keyframe
   request on the ingest side. Low priority until a real browser
   client surfaces the need.

## Session 19 (2026-04-14): audit sweep + README refresh

Session 19 is a bookkeeping + drift-closure session. No code
changes, one doc commit. The eight sessions before it had
accumulated enough state drift in the top-level `README.md` that
a new contributor reading the repo cold would get a materially
wrong picture of what ships. The audit sweep also re-verified the
tracked debt backlog so session 20 inherits an honest status
table rather than a stale "maybe closed" hint.

### Audit findings

A full sweep over the tree + the tracking docs surfaced one real
drift case and several already-closed items whose status was not
reflected in the docs:

1. **`README.md` was stale by approximately 40 test binaries and
   139+ individual tests**. The Status section still claimed "29
   test binaries workspace-wide, 130+ individual tests including
   2560 generated proptest cases" from what looks like a
   Tier 1-era snapshot. Reality at session 18 close is 69
   binaries and 269 tests. The feature list explicitly said "No
   HLS, LL-HLS, DASH, WHIP, WHEP, SRT, or RTSP egress or ingest
   yet", which is now false: LL-HLS with multi-broadcast routing
   and an audio rendition group has been on `main` since
   session 13, and the WHEP signaling router has been on `main`
   since session 16. The crate table omitted `lvqr-fragment`,
   `lvqr-cmaf`, `lvqr-hls`, `lvqr-codec`, `lvqr-moq`, and
   `lvqr-whep` entirely -- six of the seven Tier 2.x data-plane
   crates were invisible. The CLI reference omitted `--hls-port`
   (default `8888`). The architecture diagram showed only the
   original RTMP -> MoQ -> Browser path with no LL-HLS or WHEP
   fork. All four drift vectors addressed in this commit.

2. **`AUDIT-INTERNAL-2026-04-13.md` "Fix Plan for This Session"
   is 100% closed.** Session 17 closed the admin auth-failure
   metric; earlier sessions had silently closed the other four.
   The HANDOFF session 17 entry already lists this, but the
   top-of-file status line lacked a pointer.

3. **`AUDIT-INTERNAL` "Deferred" items**. Status re-verified
   against the current tree:

   | Item | Status |
   |---|---|
   | Delete `lvqr-core::{Registry, RingBuffer, GopCache}` | CLOSED (types gone from source; README fixed session 17) |
   | Delete `lvqr-wasm` | OPEN (scheduled for v0.5; crate is still marked `# DEPRECATED` in its own lib.rs header) |
   | Admin auth-failure metric | CLOSED (session 17) |
   | CORS restrict | OPEN. `CorsLayer::permissive()` at `crates/lvqr-cli/src/lib.rs:438`. Breaking change; scope as its own commit with a release note. |
   | Rate limits on every auth surface | OPEN (Tier 3) |
   | `lvqr-signal` input validation | CLOSED. `is_valid_peer_id`, `is_valid_track`, `MAX_PEER_ID_LEN`, `MAX_TRACK_LEN` all in `crates/lvqr-signal/src/signaling.rs`. |
   | `lvqr-record` integration test via event bus | CLOSED. `crates/lvqr-record/tests/record_integration.rs`. |
   | fMP4 `esds` multi-byte descriptor length | CLOSED. `write_mpeg4_descriptor` in `lvqr-ingest::remux::fmp4` uses the 4-byte variable-length encoding. |

4. **Test-count drift in the HANDOFF top-of-file status line**.
   Session 18's HANDOFF entry reported "269 individual tests";
   the actual accounting is "269 passing + 1 `ignore`d doctest"
   because `cargo test --workspace` shows `1 ignored` in a
   `lvqr-fragment` doc block marked `ignore` (a code example
   that intentionally does not compile). Cosmetic but worth
   being accurate. Top line refined here.

5. **`docs/architecture.md` + `docs/quickstart.md`**. Still
   stale per the `AUDIT-READINESS-2026-04-13.md` findings --
   architecture.md references `tokio::select!` (pre-Tier 0
   shape), quickstart.md references a `your-server:8080/watch/my-stream`
   URL that does not exist. `AUDIT-READINESS` deliberately gated
   these on a "Tier 5 docs site pass", so they stay out of scope
   for session 19. `README.md` is the authoritative public
   surface for now.

### What session 19 landed

One logical change, one commit, docs-only:

* **`README.md`**: Rewrote the Status section to reflect Tier 2.3
  closure (`lvqr-ingest` -> `lvqr-cmaf` -> `lvqr-hls` /
  `lvqr-whep`, real RTMP-with-audio E2E, retired hand-rolled
  video writer, WHEP signaling router shipped behind an
  `SdpAnswerer` trait boundary, audio timescale fix). Refreshed
  the test counts to the authoritative 69 binaries / 269 tests.
  Replaced the "No HLS, LL-HLS, DASH, WHIP, WHEP, SRT, or RTSP"
  line with an honest list of remaining limitations (no str0m
  backend yet, no DASH / WHIP / SRT / RTSP, mesh is topology
  only, CORS is still permissive, HEVC / AV1 / Opus surface
  untested in the full ingest path). Added `lvqr-fragment`,
  `lvqr-cmaf`, `lvqr-hls`, `lvqr-codec`, `lvqr-moq`, `lvqr-whep`
  to the crate table with one-line descriptions matching the
  actual crate surface. Updated the architecture diagram to
  fork from a single bridge output into four egress paths
  (MoQ / WebSocket fMP4 / LL-HLS / WHEP). Added `--hls-port`
  to the CLI reference. Added `tracking/HANDOFF.md` to the
  "Read before contributing" list as the canonical source of
  truth for current state, pointing new contributors at the
  session-by-session entries rather than the frozen three-audit
  snapshot.

* **`tracking/HANDOFF.md`** (this file): Top status line refined
  to distinguish passing tests from the `ignored` doctest.
  Added this session-19 section.

### Verification run (session 19)

* `cargo test --workspace` -- 69 binaries, 269 passing + 1
  ignored doctest, 0 failures.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.
* Audit-item status table above re-verified against the current
  tree via `grep` / `ls` / source reads, not by trusting earlier
  HANDOFF claims.

### Recommended entry point (session 20)

Unchanged from session 18's handoff. The code-work picking list:

1. **str0m-backed `Str0mAnswerer`**. Full session of its own;
   session 20 should start by reading str0m's crate docs in
   the cargo registry cache before writing any code. Expect
   offer -> answer to require binding a UDP socket for ICE
   candidates, and expect `Rtc::sdp_api().accept_offer` to need
   explicit media direction + codec configuration (H264 is not
   always default enabled in str0m).
2. **`--whep-addr` flag in `lvqr-cli`** + `RawSampleObserver`
   attachment on the bridge. Small follow-up once item 1 is
   real. Should not ship without item 1 because an `--whep-addr`
   flag that returns 501 on every POST is worse than no flag.
3. **Fuzz slot for the SDP offer parser**
   (`crates/lvqr-whep/fuzz/fuzz_targets/parse_offer_sdp.rs`).
   Lands naturally with item 1 because the fuzz corpus needs
   the real offer parser to target.

Optional low-risk cleanup items that do not require str0m:

* **Delete `lvqr-wasm`**. Scheduled for v0.5, crate is marked
  `# DEPRECATED`, no consumers. One-commit mechanical deletion
  + workspace Cargo.toml + CI `wasm` job removal.
* **CORS restrictive default**. Scope: replace
  `CorsLayer::permissive()` in `crates/lvqr-cli/src/lib.rs:438`
  with a tight default allowing only the configured admin
  origin plus localhost. Add a `--cors-allow-origin` flag for
  opt-in. Breaking change; ship with a release note.

## Session 18 (2026-04-14): fix LL-HLS audio partial duration reporting

Session 18 is a one-commit session (3058ee3) closing the cosmetic
follow-up that session 14 flagged: the LL-HLS audio playlist was
rendering `#EXT-X-PART:DURATION` values scaled by
48_000 / 44_100 ≈ 1.088 for 44.1 kHz AAC content because session
13 hardcoded `audio_config_from` to `timescale: 48_000`. For a
typical 1024-sample AAC-LC frame the playlist reported
`DURATION=0.021333` (1024 / 48000) instead of the correct
`DURATION=0.023220` (1024 / 44100). Routing and serving were
always correct -- only the rendered duration was wrong.

### What changed (six files touched)

1. **`crates/lvqr-cmaf/src/policy.rs`**. New
   `CmafPolicy::for_timescale(timescale: u32)` constructor that
   builds a policy scaled to any timescale using the standard
   LL-HLS targets (200 ms partials, 2 s segments).
   `VIDEO_90KHZ_DEFAULT` and `AUDIO_48KHZ_DEFAULT` are the
   specialised shapes this constructor returns for 90_000 Hz and
   48_000 Hz respectively -- both constants kept for source-level
   compatibility with the proptest / segmenter / coalescer test
   suites that already name them.

2. **`crates/lvqr-ingest/src/observer.rs`**. `FragmentObserver::on_init`
   signature grows a `timescale: u32` parameter carrying the
   track's native sample rate. Docstring explains why: downstream
   consumers need the real denominator to render wall-clock
   durations from tick counts. `NoopFragmentObserver` impl
   updated.

3. **`crates/lvqr-ingest/src/bridge.rs`**. Video on_init fire
   passes `90_000` (hardcoded because `video_init_segment_with_size`
   writes `mvhd.timescale = 90000` unconditionally). Audio on_init
   fire captures `config.sample_rate` into a local `audio_timescale`
   before the `AudioConfig` moves into `stream.audio_config`, then
   passes it through. No other bridge semantics change.

4. **`crates/lvqr-hls/src/server.rs`**.
   `MultiHlsServer::ensure_audio(broadcast, timescale)` now takes
   the audio track timescale as a second argument. The derived
   `audio_config_from(video, timescale)` swaps the hardcoded
   48_000 for the passed value so
   `PlaylistBuilderConfig::timescale` on the audio rendition
   reflects the real sample rate. The session-13 TODO comment on
   `audio_config_from` called for exactly this.

5. **`crates/lvqr-cli/src/hls.rs`**. `HlsFragmentBridge::on_init`
   builds the per-track `CmafPolicy` via
   `CmafPolicy::for_timescale(timescale)` and passes the same
   `timescale` into `ensure_audio`. `on_fragment`'s audio and
   video branches switch from `ensure_video` / `ensure_audio`
   (producer-side side-effects) to pure `self.multi.video()` /
   `self.multi.audio()` lookups that skip cleanly if the init
   has not landed yet -- a defensive branch since the FLV
   sequence header always arrives before any raw frame.

6. **`crates/lvqr-hls/tests/integration_master.rs`**. Session-13
   test updated to pass `48_000` as the audio timescale, matching
   the test's pre-existing `audio_chunk(..., 96_000, ...)`
   duration assumptions.

### Verification

`rtmp_hls_e2e` now prints the exact expected value in its audio
playlist body: `#EXT-X-PART:DURATION=0.023220` for both audio
partials (the test publishes two AAC frames at 44.1 kHz). That
is `1024 / 44100 = 0.0232199...` rounded to six decimals by the
playlist renderer's `{:.6}` format specifier. `cargo test
--workspace` passes 269 tests. `cargo clippy --workspace
--all-targets -- -D warnings` clean. `cargo fmt --all --check`
clean.

### Recommended entry point (session 19)

With the audio timescale follow-up closed, the remaining open
threads are unchanged from the session 17 handoff:

1. **str0m-backed `Str0mAnswerer`**. The full WHEP signaling →
   transport integration. Still a full session of its own.
2. **`--whep-addr` flag in `lvqr-cli`** + `RawSampleObserver`
   attachment. Small follow-up after item 1.
3. **Fuzz slot for the SDP offer parser**. Lands with item 1.

Session 18 did not touch any of these.

## Session 17 (2026-04-14): close deferred audit findings

Session 17 is a small bookkeeping session that closes two items
from the `AUDIT-INTERNAL-2026-04-13.md` deferred list which had
been tracked for multiple sessions without landing. One logical
commit (9f1c3e0), four files touched:

1. **`crates/lvqr-admin/Cargo.toml`** + **`crates/lvqr-admin/src/routes.rs`**.
   Added `metrics` as a dep and emits
   `lvqr_auth_failures_total{entry="admin"}` from the admin
   middleware on every `AuthDecision::Deny`. Before this commit,
   the admin surface was the only LVQR auth entry point that
   denied silently -- RTMP, MoQ, WS ingest, and WS subscribe all
   already emitted the same counter with different `entry`
   labels. Brute-force attempts against the admin token are now
   visible to Prometheus scrapers with the exact same query shape
   operators already use for the other entry points.

2. **`crates/lvqr-core/README.md`**. Replaced the stale crate
   overview which still documented `Registry`, `RingBuffer`, and
   `GopCache` as shipping API. Those types were deleted from the
   source tree at the Tier 2.1 fragment-model landing; the Rust
   lib.rs module doc already reflected the new reality but the
   README did not. The refreshed README lists the actual
   remaining surface (EventBus, RelayEvent, TrackName, Frame,
   RelayStats, CoreError) with a working usage example.

### Audit sweep verifying closed items

While scoping session 17 I re-verified every item on the
`AUDIT-INTERNAL-2026-04-13.md` "Deferred" list against the current
tree. The items tagged closed during earlier sessions without a
HANDOFF note are:

| Item | Status found in tree |
|---|---|
| Delete `lvqr-core::{Registry, RingBuffer, GopCache}` | CLOSED in the Rust sources already; only README drift remained. Fixed this session. |
| `lvqr-signal` input validation | CLOSED. `is_valid_peer_id`, `is_valid_track`, `MAX_PEER_ID_LEN`, `MAX_TRACK_LEN` all present in `crates/lvqr-signal/src/signaling.rs`. |
| `lvqr-record` integration test via event bus | CLOSED. `crates/lvqr-record/tests/record_integration.rs` exists. |
| fMP4 esds multi-byte descriptor length encoding | CLOSED. `write_mpeg4_descriptor` in `lvqr-ingest::remux::fmp4` uses the 4-byte variable-length encoding. |
| Admin auth-failure metric | CLOSED (this commit). |

Items still correctly deferred:

* **Delete `lvqr-wasm`**. Scheduled for v0.5 per the original
  audit. The crate is marked `# DEPRECATED` in its own `lib.rs`
  header and the browser client now lives in TypeScript. No
  consumers; deletion is safe but mechanical and can land
  whenever a session wants the bookkeeping.
* **CORS restrict in `lvqr-cli`**. `CorsLayer::permissive()` is
  still applied at `crates/lvqr-cli/src/lib.rs:438`. The audit
  recommended a restrictive default allowing only the configured
  admin origin plus localhost. Deferred from this session because
  changing the CORS default is potentially a breaking change for
  any existing browser client that depends on the wide-open
  policy; should land as its own commit with a matching release
  note, not piggybacked.
* **Rate limits on every auth surface**. Tier 3 hardening; a
  `tower::limit::RateLimit` layer on the admin router plus a
  per-IP accept budget on WS and MoQ. Still blocked on the full
  Tier 3 gate.

## Session 16 (2026-04-14): `lvqr-whep` signaling router + integration slot

Session 16 closed the second artifact slot of the 5-artifact
contract for `lvqr-whep` by landing the full HTTP signaling
surface. No `str0m` yet; the router talks to the WebRTC side
through a clean trait boundary (`SdpAnswerer` + `SessionHandle`)
so a concrete `str0m`-backed answerer drops in later as a single
type swap at construction time. One commit (3b1433b), three new
files plus `lib.rs` + `Cargo.toml` updates:

1. **`crates/lvqr-whep/src/server.rs`** (new). `SessionId` (32-char
   random hex via `rand::thread_rng().fill_bytes`), `WhepError`
   enum with four variants (`UnsupportedContentType`,
   `MalformedOffer`, `SessionNotFound`, `AnswererFailed`) and an
   `IntoResponse` impl mapping each onto 415 / 400 / 404 / 500,
   the `SdpAnswerer` and `SessionHandle` traits that form the
   plug point for a real WebRTC stack, `WhepServer` (cheap
   `Clone` around `Arc<WhepState>` so one instance lives in both
   the axum router and the ingest bridge's `RawSampleObserver`
   slot), and a `RawSampleObserver` impl that fans each upstream
   sample out to every session whose `broadcast` field matches.
   Three unit tests covering session-id entropy and error status
   mapping.

2. **`crates/lvqr-whep/src/router.rs`** (new). axum `Router` built
   on a `/whep/{*path}` catch-all with `post(handle_offer).patch(handle_trickle).delete(handle_terminate)`
   method routing. The catch-all exists because broadcast names
   follow the RTMP `{app}/{stream_key}` convention and therefore
   carry a `/` (e.g. `live/test`), and axum path parameters only
   match single URL segments. On POST, the captured `path` is
   the broadcast name verbatim; the handler mints a random
   `SessionId`, registers it in the state's `DashMap<SessionId,
   SessionEntry>`, and returns 201 Created with a `Location:
   /whep/{broadcast}/{session_id}` header plus the SDP answer
   body. On PATCH and DELETE the handler splits the captured
   path on the last `/` via a `split_session_path` helper to
   recover `(broadcast, session_id)`. POST content-type accepts
   `application/sdp` with or without parameters; PATCH also
   accepts `application/trickle-ice-sdpfrag` per the WHEP draft.

3. **`crates/lvqr-whep/src/lib.rs`**. Added `pub mod router; pub
   mod server;` plus re-exports for `router_for`, `SdpAnswerer`,
   `SessionHandle`, `SessionId`, `WhepError`, `WhepServer`.

4. **`crates/lvqr-whep/Cargo.toml`**. Runtime deps: `axum`,
   `dashmap`, `lvqr-cmaf`, `lvqr-ingest`, `rand`, `thiserror`,
   `tracing`. Dev-deps: `tokio` (features `macros`, `rt`) and
   `tower` for `ServiceExt::oneshot`.

5. **`crates/lvqr-whep/tests/integration_signaling.rs`** (new).
   The integration slot of the 5-artifact contract. Twelve tests
   driving the real axum router via `tower::ServiceExt::oneshot`
   with two stub answerers: `StubAnswerer` (shared atomic
   counters for trickle + sample call counts) and
   `TaggingAnswerer` (tags handles by broadcast so the fanout
   test can assert which broadcast saw each sample). Coverage:

   * `post_offer_returns_created_with_location_and_answer` --
     full happy path with 201 + Location header format assertion
     + `Content-Type: application/sdp` + SDP answer body + session
     count increment.
   * `post_offer_without_content_type_returns_415`
   * `post_offer_with_wrong_content_type_returns_415`
   * `post_offer_accepts_content_type_with_parameters` --
     `application/sdp; charset=utf-8` must be accepted.
   * `post_offer_with_empty_body_returns_400` -- `MalformedOffer`
     path; session must not be registered.
   * `delete_unknown_session_returns_404`
   * `session_lifecycle_post_then_delete` -- POST -> DELETE ->
     second-DELETE, asserts the session count roundtrips to zero
     and the second delete is 404.
   * `patch_unknown_session_returns_404`
   * `patch_existing_session_forwards_to_handle` -- PATCH body
     actually reaches `SessionHandle::add_trickle` via the
     shared atomic counter.
   * `patch_with_wrong_content_type_returns_415`
   * `raw_sample_observer_routes_only_to_subscribed_sessions` --
     subscribes one session per broadcast, pushes samples for
     `live/one` (2x), `live/two` (1x), and `live/three`
     (unsubscribed). Asserts per-broadcast counters land on
     2 / 1 / 0. This is the load-bearing correctness property for
     the fanout design.
   * `unknown_route_returns_404` -- actually asserts 405 since
     GET matches the catch-all path but no GET handler is
     registered. Test name is slightly stale; assertion is
     correct.

### Routing bug caught by the integration slot

Session 16's first-pass router used axum's
`/whep/{broadcast}` + `/whep/{broadcast}/{session_id}` two-route
shape, which compiled clean and passed clippy but failed 9 of 12
tests on first run with 405s. The bug: `{broadcast}` only matches
single URL segments, so `/whep/live/test` was matching the
two-segment `/whep/{broadcast}/{session_id}` route with
broadcast = `live` and session_id = `test`, leaving POST without
a handler (hence 405). Fixed by flipping to the `/whep/{*path}`
catch-all with manual splitting on the last `/` inside each
handler -- the exact pattern `lvqr-hls::MultiHlsServer::router`
already uses for the same problem. Without the integration slot
landing alongside the code, session 17 would have inherited a
dead router that returns 405 for every real client; the
integration slot paid for itself on its first CI run.

### Design decisions answered (session 16)

The session-11 `lvqr-whep` design note lists four open questions.
Session 16 answered them and the answers are baked into the
router and the trait boundary:

1. **Packetizer home**: private module `lvqr-whep::rtp`. Promote
   to a standalone `lvqr-rtp` crate later when `lvqr-whip` needs
   the inverse depacketizer. No speculative abstraction.
2. **Socket strategy**: one UDP socket per session for v0.x.
   Simpler control flow, no ICE-lite demux to write. Shared
   sockets are a perf-driven refactor later.
3. **WHEP bind**: `Option<SocketAddr>` under a future
   `--whep-addr` flag on `lvqr-cli`, default disabled. Users
   opt in during the v0.x cycle. The flag itself lands with the
   first concrete `SdpAnswerer` so the route stops returning
   "not yet implemented" bodies.
4. **Token transport**: `Authorization: Bearer <token>` on the
   offer POST. The WS surface's query-param and subprotocol
   fallbacks stay WS-specific. Not wired yet; the router already
   reads `HeaderMap` so plumbing `SharedAuth` through
   `WhepServer` is a small local diff in the CLI integration
   session.

### Contract slot status after session 16

`lvqr-whep` is now at **2 of 5 contract slots closed**:

| Slot | Status |
|---|---|
| proptest | CLOSED (session 15, `tests/proptest_packetizer.rs`) |
| integration | CLOSED (session 16, `tests/integration_signaling.rs`) |
| fuzz | OPEN (offer SDP parser lives in str0m; lands with str0m) |
| e2e | OPEN (`lvqr-cli/tests/rtmp_whep_e2e.rs` once str0m + webrtc-rs client subprocess is available) |
| conformance | OPEN (cross-implementation against `simple-whep-client`, not yet installed in CI) |

### Recommended entry point (session 18)

With session 16 closing signaling and session 17 closing the
audit debt, the next block of work is the str0m integration
itself. The picks for session 18:

1. **Bring in `str0m` as a workspace dep and implement
   `Str0mAnswerer`**. Replace the stub answerers in
   integration tests with a real one for at least the offer ->
   answer path. Expect the first implementation to need UDP
   socket binding at construction time (str0m needs a local
   host ICE candidate to include in the answer) and session-
   scoped state that stores the `Rtc` state machine. Leave
   `add_trickle` and `on_raw_sample` as TODO with tracing
   warnings -- driving the `Rtc` state machine forward and
   packetizing samples into RTP is a separate follow-up.
2. **Wire `--whep-addr` in `lvqr-cli::ServeArgs`** and
   construct the `WhepServer` with `Str0mAnswerer` in
   `lvqr_cli::start`, attach it to the bridge via
   `RtmpMoqBridge::with_raw_sample_observer`, and mount the
   router on the configured binding. Small follow-up once item
   1 is real.
3. **Fuzz slot for the SDP offer parser** under
   `crates/lvqr-whep/fuzz/fuzz_targets/parse_offer_sdp.rs`
   seeded from captured browser offers. Lands naturally
   alongside item 1.

Item 1 is the full session. Items 2 and 3 are follow-ups that
assume str0m is actually producing answers. E2E (`rtmp_whep_e2e.rs`)
and conformance (`simple-whep-client` soft-skip) slots are
session 19 or later.

### Audio timescale follow-up (tracked from session 14)

Session 14 flagged a cosmetic bug where `HlsFragmentBridge`
pushes audio chunks through the `AUDIO_48KHZ_DEFAULT` policy
while the bridge itself emits audio at the AAC sample rate
(44100 Hz via `audio_init_segment` + `audio_segment`), so the
`#EXT-X-PART:DURATION` values reported in the LL-HLS audio
playlist are scaled by 48000 / 44100 (a 1024-sample AAC frame
reports 0.021333 s instead of the true 0.023220 s). Still open.
Candidate fix: either retire `audio_segment` alongside the
session-14 `video_segment` deletion by routing AAC through
`lvqr_cmaf::build_moof_mdat` with a proper audio-timescale
policy, or have the bridge construct a per-broadcast
`CmafPolicy` with the right timescale at init time. Routing
and serving are correct today; only the reported duration is
off. Not blocking WHEP work.

## Session 15 (2026-04-14): begin `lvqr-whep` implementation

Session 15 closed item 2 from the session-14 entry-point list by
starting the WHEP egress implementation. No networking yet; this
session lands the two pieces that have no dependency on `str0m` or
axum so future sessions can iterate on signaling against a stable
packetizer. Three files added, three files touched:

1. **`crates/lvqr-ingest/src/observer.rs`**. New `RawSampleObserver`
   sibling trait alongside the existing `FragmentObserver`, plus
   `SharedRawSampleObserver = Arc<dyn RawSampleObserver>` and
   `NoopRawSampleObserver`. The observer takes a
   `&lvqr_cmaf::RawSample` and is fired from the bridge's video and
   audio callback paths **before** the sample is muxed into an
   fMP4 fragment. Consumers that need per-NAL AVCC or raw AAC bytes
   subscribe here instead of re-parsing `CmafChunk` mdat bodies
   downstream. The dep on `lvqr-cmaf` was already normal-dep via
   session 14's deletion of the hand-rolled writer, so importing
   `RawSample` into observer.rs is free.

2. **`crates/lvqr-ingest/src/bridge.rs`**. `RtmpMoqBridge` gained a
   `raw_observer: Option<SharedRawSampleObserver>` field plus
   `with_raw_sample_observer` / `set_raw_sample_observer` builder
   methods matching the existing `FragmentObserver` builders. In
   the video callback, the already-constructed `lvqr_cmaf::RawSample`
   (pre-`build_moof_mdat`) is handed to the observer as
   `(broadcast, "0.mp4", &sample)`. In the audio callback, a fresh
   `RawSample { track_id: 2, payload: aac_data.clone(), keyframe:
   true, ... }` is built for the observer only (the existing
   `audio_segment` mux path is unchanged); `aac_data.clone()` is a
   `Bytes` refcount bump, not an allocation.

3. **New crate `lvqr-whep`**. Registered as a workspace member in
   `Cargo.toml` and exposed through the standard
   `lvqr-whep = { version = "0.3.1", path = "crates/lvqr-whep" }`
   workspace-dep entry. The crate ships:
   * `Cargo.toml` with `bytes` as the only runtime dep and
     `proptest` as a dev-dep. No `str0m`, no `axum`, no `tokio`
     yet -- those land with the networking layer.
   * `src/lib.rs` -- module-level doc note pointing at
     `crates/lvqr-whep/docs/design.md` plus re-exports of
     `H264Packetizer` and `H264RtpPayload` from the new `rtp`
     module. STAP-A aggregation is explicitly called out as a v0.x
     non-goal.
   * `src/rtp.rs` -- stateless `H264Packetizer { mtu }` that walks
     AVCC length-prefixed NAL sequences and emits RFC 6184 RTP
     payloads (the bytes placed after the RTP fixed header; the
     sender writes the header itself). Single-NAL-unit mode (§5.6)
     for NALs that fit within the MTU budget; FU-A fragmentation
     (§5.8) for oversized NALs with correct Start / End bit
     handling across fragments and `is_start_of_frame` /
     `is_end_of_frame` flags tracked across multi-NAL inputs so a
     sender can map end-of-frame onto the RTP marker bit. The MTU
     is clamped to a minimum of `FU_HEADER_SIZE + 1` so a
     single-byte fragment is always representable; the default is
     `DEFAULT_MTU = 1200` to match the `str0m` / Pion / libwebrtc
     safe Ethernet budget.
   * `split_avcc` helper that walks `[u32-be length][body]` tuples
     and skips malformed entries silently: truncated length
     prefixes, zero-length bodies, and length fields that overrun
     the buffer all stop the walker cleanly without panicking. The
     proptest slot below pins the never-panic property.
   * `tests/proptest_packetizer.rs` -- the proptest slot of the
     5-artifact contract. Four properties: (1) `packetize` never
     panics on arbitrary bytes with arbitrary MTUs, (2) on
     well-formed AVCC input every payload respects the MTU budget
     and the start-of-frame / end-of-frame flags land on the first
     and last packet only, (3) FU-A fragments round-trip back to
     the original NAL body byte-for-byte after header
     reconstruction, (4) single-NAL-unit mode emits a single
     verbatim payload. 512 cases per property plus a persisted
     regression file under
     `tests/proptest_packetizer.proptest-regressions` pinning the
     one degenerate case proptest found during initial development
     (length-1 output slicing into `out[1..0]`).

### Design decisions answered

The session-11 design note at `crates/lvqr-whep/docs/design.md`
lists four open questions. Session 15 picks:

1. **Packetizer home**: private module `lvqr-whep::rtp`. Promote to
   a standalone `lvqr-rtp` crate later when `lvqr-whip` needs the
   inverse (depacketizer). No speculative abstraction.
2. **Socket strategy**: one UDP socket per session for v0.x.
   Simpler control flow over `str0m`'s sans-IO state machine, no
   ICE-lite demux to write. Shared sockets become a perf-driven
   refactor later.
3. **WHEP bind**: new `--whep-addr` flag mirroring `--hls-port`,
   default disabled (`Option<SocketAddr>`). Users opt in during the
   v0.x cycle. Wiring the flag is deferred to the networking
   session; session 15 does not touch `lvqr-cli`.
4. **Token transport**: `Authorization: Bearer <token>` on the
   offer POST only. The WS surface's query-param + subprotocol
   fallbacks stay WS-specific; WHEP takes the standards-track
   header path.

### Verification run

* `cargo test --workspace` -- 68 binaries, 254 individual tests,
  0 failures. New binary: `lvqr-whep::tests::proptest_packetizer`
  (4 proptest cases). The `lvqr-whep::rtp::tests` unit block adds
  8 tests to the `lvqr-whep` lib binary.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.

### What session 15 did NOT land

* **No networking**. No `str0m` dep, no SDP offer/answer parser,
  no axum router, no `WhepServer` state, no UDP socket task, no
  ICE / DTLS handshake wiring, no `--whep-addr` CLI flag. The
  signaling layer lands in the next session once the packetizer is
  proven as a building block.
* **Fuzz / integration / e2e / conformance slots**. Four of the
  five contract slots for `lvqr-whep` are still open. They land
  alongside the signaling layer so each closed slot has something
  real to exercise. `crates/lvqr-whep/docs/design.md` §5 has the
  full plan.
* **HEVC / AV1 packetizer**. AVC-only for the first WHEP release,
  matching the design-note non-goals.
* **RawSampleObserver wiring in `lvqr-cli`**. The trait is
  registered and the bridge fires it, but no consumer is attached
  yet. A future WHEP server constructor calls
  `RtmpMoqBridge::with_raw_sample_observer` to subscribe.
* **Audio byte-sharing**. The raw-sample observer sees the AAC
  access unit and the HLS path sees the same access unit re-muxed
  into an fMP4 fragment; the two views are reference-counted
  `Bytes` clones (cheap), not literally the same buffer. Not a
  problem in v0.x; flagged here in case a future session tries
  to share a single allocation across both paths.

### Recommended entry point (session 16)

The session-14 entry-point list is now closed: item 1 (deletion)
and item 3 (audio E2E) landed in session 14; item 2 (WHEP start)
landed in session 15. The natural session-16 picks are:

1. **Bring up the WHEP signaling layer**. Add `str0m` as a
   workspace dep, land `lvqr-whep::server::WhepServer` as an
   `Arc<WhepState>` wrapping `DashMap<SessionId,
   ActiveSubscriber>` and a handle to the `RawSample` tap, mount
   an axum router under `/whep/{broadcast}` with the POST /
   PATCH / DELETE handlers the design note specifies, and land
   the integration slot
   (`crates/lvqr-whep/tests/integration_signaling.rs`) via
   `tower::ServiceExt::oneshot` against a synthetic SDP offer.
   This is the entire session.
2. **Wire `--whep-addr` in `lvqr-cli` and attach the
   `RawSampleObserver`**. Small second commit once item 1 lands:
   add the flag to `ServeArgs`, construct the `WhepServer` in
   `lvqr_cli::start` when the flag is set, pass it as a
   `RawSampleObserver` into `RtmpMoqBridge::with_raw_sample_observer`,
   and mount the router on the configured axum binding.
3. **Fuzz slot for the offer SDP parser**. Can land in the same
   session as item 1 or immediately after. Seeds from the
   webrtc-rs / Pion offer fixtures plus captured Chrome devtools
   offers.

Item 1 is the full session. Items 2 and 3 are follow-ups that
assume item 1 landed first. E2E and conformance slots
(`rtmp_whep_e2e.rs` + cross-implementation test against
`simple-whep-client`) are session 17 or later: the E2E slot
requires a working webrtc-rs client dep and the conformance slot
needs `simple-whep-client` installed in CI, which today is not.

### AUDIT-INTERNAL-2026-04-13 "Fix Plan for This Session" status

All five items verified closed on main as of session 15:

| Item | Status |
|---|---|
| 1. Validator for broadcast names in `lvqr-relay::parse_url_token` | **CLOSED** (`is_valid_broadcast_name` + unit tests in `server.rs`) |
| 2. Defensive old-parent cleanup in `lvqr-mesh::reassign_peer` | **CLOSED** (live rebalance path retains old-parent children list correctly) |
| 3. `--jwt-secret` CLI flag wired to `JwtAuthProvider` | **CLOSED** (`lvqr-cli::main::ServeArgs` + integration test in `crates/lvqr-cli/tests/auth_integration.rs`) |
| 4. `lvqr-mesh/src/lib.rs` topology-planner disclaimer comment | **CLOSED** (lines 1-19) |
| 5. Heartbeat theatrical test | **CLOSED** (`heartbeat_keeps_peer_alive` uses a real 1 s timeout) |

Tracked-for-later items (still correctly deferred):

* Delete `lvqr-core::{Registry, RingBuffer, GopCache}` dead code.
  Needs verification that nothing in the Tier 2.3 data plane started
  consuming them transitively. Low-risk cleanup; session 16 or 17.
* Delete `lvqr-wasm`. Scheduled for v0.5.
* Admin auth-failure metric / CORS restrict / rate limits. Tier 3.
* `lvqr-signal` peer_id input validation. Tier 2 hardening, can
  land opportunistically; the scope is one `validate_peer_id`
  helper enforcing `^[A-Za-z0-9_-]{1,64}$` plus a 1-cap on
  registrations per connection.
* `lvqr-record` integration test via EventBus. Tier 1 follow-up;
  non-trivial because the WS ingest handler is private in the
  binary crate.

## Session 14 (2026-04-14): delete hand-rolled fMP4 writer + RTMP audio E2E

Session 14 closed items 1 and 3 from the session-13 entry-point
list. Two logical landings in one commit (6d86214):

### Item 1: retire `lvqr_ingest::remux::fmp4::video_segment`

The hand-rolled video media-segment writer, its `VideoSample`
adapter, its `build_video_segment` dispatch wrapper, and all its
surrounding test + feature-flag scaffolding are gone. The
`cmaf-writer` feature was default-on for a full release cycle
(sessions 12.2 -> 13), the parity gate caught every drift in the
transition, and the legacy path can be removed without risk.

Files touched:

* **`crates/lvqr-ingest/src/remux/fmp4.rs`**. Deleted: the
  `VideoSample` struct, `video_segment` (the ~80-line hand-rolled
  writer), `build_video_segment` (the feature-flag dispatch
  wrapper), `build_video_segment_via_cmaf` (the cmaf-writer
  branch), and the four unit tests (`video_segment_structure`,
  `video_segment_data_offset_correct`, `video_segment_multiple_samples`,
  `empty_samples_returns_empty`). Kept: `video_init_segment`,
  `video_init_segment_with_size`, `audio_init_segment`,
  `audio_segment`, `patch_trun_data_offset` (used by
  `audio_segment`), `write_mpeg4_descriptor`, all the box-writing
  helpers. The audio path is untouched; the handoff directive
  explicitly carved out `audio_segment` as unrelated.

* **`crates/lvqr-ingest/src/remux/mod.rs`**. Re-export list
  pruned to `audio_init_segment, audio_segment,
  video_init_segment, video_init_segment_with_size`.

* **`crates/lvqr-ingest/src/bridge.rs`**. Video callback now
  constructs `lvqr_cmaf::RawSample { track_id: 1, dts: base_dts,
  cts_offset: cts * 90, duration: duration_ticks, payload: nalu_data,
  keyframe }` and calls `lvqr_cmaf::build_moof_mdat(stream.video_seq,
  1, base_dts, &[sample])` directly. No dispatch wrapper.

* **`crates/lvqr-ingest/Cargo.toml`**. `default = ["rtmp"]`,
  `cmaf-writer` feature removed, `legacy-fmp4` marker feature
  removed, `lvqr-cmaf` flipped from optional dev-dep to normal
  dep, `mp4-atom` dev-dep removed (was only used by the parity
  gate).

* **`crates/lvqr-cli/Cargo.toml`**. `lvqr-ingest` reverts from
  the inline path dep (session 12.2's default-features escape
  hatch) back to workspace inheritance. `cmaf-writer` forward
  flag and the `full` feature definition both drop to
  `["rtmp", "quinn-transport"]`.

* **`crates/lvqr-cli/src/lib.rs`**. WS ingest handler's two
  `remux::build_video_segment` call sites (keyframe branch +
  delta branch) swapped for direct `lvqr_cmaf::RawSample` +
  `lvqr_cmaf::build_moof_mdat` construction.

* **Deleted**: `crates/lvqr-ingest/tests/parity_avc_init.rs`
  (205 lines), `crates/lvqr-ingest/tests/parity_avc_segment.rs`
  (220 lines), the fixture
  `crates/lvqr-ingest/tests/fixtures/golden/video_segment_keyframe.mp4`.

* **Pruned**: `crates/lvqr-ingest/tests/golden_fmp4.rs` dropped
  the `video_keyframe_segment_matches_golden` test and rewrote
  `ffprobe_accepts_concatenated_cmaf` to feed the init segment
  plus a `lvqr_cmaf::build_moof_mdat`-produced media segment to
  ffprobe. The audio conformance test
  (`ffprobe_accepts_audio_init_and_frame`) is unchanged; it
  still exercises the AAC path end-to-end.

* **Pruned**: `crates/lvqr-ingest/tests/proptest_parsers.rs`
  dropped the `video_segment_is_well_formed` proptest target and
  the `video_sample_strategy` helper. The `video_init_segment_is_well_formed`
  proptest is unchanged and still pins the init writer's
  structural invariants.

* **`.github/workflows/ci.yml`**. The `test-legacy-fmp4-path`
  job is deleted wholesale. The main `test` matrix is now the
  only test pipeline; there is no second feature-flag axis to
  maintain.

`cargo tree -p lvqr-cli -e normal` before and after confirms the
dep graph stays sound: `lvqr-ingest` still reaches `lvqr-cmaf` as
a normal dep, and `lvqr-cli` keeps its own direct dep on
`lvqr-cmaf` for the WS ingest fallback.

### Item 3: real RTMP-publish-with-audio E2E

`crates/lvqr-cli/tests/rtmp_hls_e2e.rs` gained two FLV audio
helpers (`flv_audio_seq_header`, `flv_audio_raw`) and a new test
`rtmp_publish_with_audio_reaches_master_playlist` that:

1. Spins up a `TestServer` with HLS enabled.
2. Publishes a single broadcast (`live/av`) via real `rml_rtmp`:
   video seq header, AAC seq header (AAC-LC 44100 stereo), first
   keyframe at t=0, raw AAC frame at t=0, second keyframe at
   t=2100 ms, second raw AAC frame at t=2100 ms.
3. Fetches `/hls/live/av/master.m3u8` and asserts `#EXTM3U`,
   `#EXT-X-MEDIA:` with `TYPE=AUDIO`, `#EXT-X-STREAM-INF` with
   `AUDIO="audio"`.
4. Fetches `/hls/live/av/audio.m3u8` and asserts `#EXTM3U` plus
   `#EXT-X-MAP:URI="audio-init.mp4"`.
5. Fetches `/hls/live/av/audio-init.mp4` and asserts the body
   starts with `ftyp`.
6. Fetches `/hls/live/av/playlist.m3u8` and asserts the video
   playlist still references `init.mp4`.

Passed first run. Closes the session-13 gap where the audio
bridge was only exercised through a router oneshot
(`integration_master.rs`), not through a real RTMP publish.

### Known cosmetic issue flagged for follow-up

The bridge's audio path writes the media segment at the AAC
sample rate (44100 Hz via `audio_init_segment` + `audio_segment`)
but `HlsFragmentBridge` pushes the chunk through the
`AUDIO_48KHZ_DEFAULT` policy, so the emitted `#EXT-X-PART:DURATION`
values are scaled by 48000 / 44100. The test output shows
`DURATION=0.021333` for a 1024-sample AAC frame, which is
`1024 / 48000` rather than the true `1024 / 44100`. Cosmetic
only: routing and serving are correct, only the reported
duration is off. A future session should either pick the audio
policy from the actual sample rate or retire `audio_segment`
alongside the video writer by routing AAC through
`lvqr_cmaf::build_moof_mdat` with an audio-timescale policy.
Tracked here so the next session catches it.

### Verification run (session 14)

* `cargo test --workspace` -- 67 binaries, 0 failures.
* `cargo clippy --workspace --all-targets -- -D warnings` clean.
* `cargo fmt --all --check` clean.
* `cargo tree -p lvqr-cli -e normal` confirms `lvqr-cmaf`
  reachable through both `lvqr-cli` directly and via
  `lvqr-ingest`.

## Session 13 (2026-04-13): audio rendition group + master playlist

Session 13 closed item 2 from the session-11 work list: audio
rendition group in HLS, including the master-playlist generation
that was its prerequisite. Five files touched (one new):

1. **`crates/lvqr-hls/src/master.rs`** (new). Pure-library
   `MasterPlaylist` + `VariantStream` + `MediaRendition` +
   `MediaRenditionType` types and a `render()` method that emits a
   minimal HLS multivariant playlist: `#EXTM3U`, `#EXT-X-VERSION:9`,
   `#EXT-X-INDEPENDENT-SEGMENTS`, one `#EXT-X-MEDIA` per rendition,
   and one `#EXT-X-STREAM-INF` (with optional `RESOLUTION` and
   `AUDIO=` attributes) followed by the variant URI per variant.
   Six unit tests cover the empty case, the single-rendition audio
   case, language attribute presence/absence, and variant lines
   without an audio group or a resolution. Exported from
   `lvqr_hls::lib` alongside the existing media-playlist exports.

2. **`crates/lvqr-hls/src/server.rs`**. `MultiHlsServer` was
   single-rendition per broadcast (one `HlsServer` per broadcast
   key); session 13 turned the inner map into `HashMap<String,
   BroadcastEntry>` where `BroadcastEntry { video: HlsServer,
   audio: Option<HlsServer> }`. The session-12 `ensure_broadcast` /
   `get_broadcast` API renamed to `ensure_video` / `video`; new
   `ensure_audio` / `audio` accessors create the audio rendition on
   demand using a derived `audio_config_from(template)` that swaps
   the timescale to 48 kHz, the `map_uri` to `audio-init.mp4`, and
   the `uri_prefix` to `audio-` so audio chunks never collide with
   video chunks in either the cache or the wire. The
   `/hls/{*path}` catch-all dispatch now matches `master.m3u8`
   (synthesizes a master playlist, including the audio rendition
   declaration when the broadcast has called `ensure_audio`),
   `audio.m3u8` and `audio-init.mp4` (audio HlsServer's playlist
   and init), URIs prefixed `audio-` (audio HlsServer's chunk
   cache), and falls through to the video HlsServer for everything
   else. The session-12 video routes (`playlist.m3u8`, `init.mp4`,
   chunk URIs) are unchanged. Unknown broadcasts and unknown audio
   renditions return 404 instead of empty 200s. The session-12
   `rtmp_hls_e2e.rs` test still passes against the renamed API.

3. **`crates/lvqr-cli/src/hls.rs`**. `HlsFragmentBridge` now keeps
   two policy state maps (video keyed by broadcast, audio keyed by
   broadcast) and dispatches fragments by track id: video samples
   (`0.mp4`) go to `multi.ensure_video(broadcast)` with the
   `VIDEO_90KHZ_DEFAULT` policy, audio samples (`1.mp4`) go to
   `multi.ensure_audio(broadcast)` with the `AUDIO_48KHZ_DEFAULT`
   policy. Tracks other than `0.mp4` and `1.mp4` are still
   ignored. The `dispatch_init` / `dispatch_chunk` /
   `classify` / `reset` helpers are factored out so the video and
   audio code paths share the same Tokio task-spawning shape.

4. **`crates/lvqr-hls/tests/integration_master.rs`** (new). Three
   integration tests driving `MultiHlsServer::router` via
   `tower::ServiceExt::oneshot`:

   * `master_playlist_includes_audio_rendition_when_both_tracks_present`:
     pushes a video init + segment chunk and an audio init +
     segment chunk into a `live/test` broadcast, fetches
     `/hls/live/test/master.m3u8` and asserts it contains the
     audio `EXT-X-MEDIA` line, an `EXT-X-STREAM-INF` line with
     `AUDIO="audio"`, and the variant URI on the next line. Also
     fetches `/hls/live/test/playlist.m3u8`, `/hls/live/test/audio.m3u8`,
     `/hls/live/test/init.mp4`, and `/hls/live/test/audio-init.mp4`
     and asserts each is served correctly with the right body and
     `Content-Type`.
   * `master_playlist_omits_audio_when_only_video_has_published`:
     same flow but only video is published. Master playlist must
     not contain `EXT-X-MEDIA` or `AUDIO=`. The audio playlist
     and audio init both 404.
   * `master_playlist_returns_404_for_unknown_broadcast`: the
     happy-path 404 case for a broadcast that has no renditions
     at all.

5. **`crates/lvqr-hls/src/lib.rs`**. New `master` module exported
   alongside the existing `manifest` and `server` modules.

### Verification run

* `cargo test --workspace` -- 67 binaries, 251 individual tests,
  0 failures. The new binary is `integration_master` (3 tests);
  the other 6 new tests are the `master::tests` unit tests in
  `lvqr-hls`'s lib binary.
* `cargo test -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --test rtmp_hls_e2e --test rtmp_ws_e2e` --
  both legacy-path E2E tests still green.
* `cargo clippy --workspace --all-targets -- -D warnings` clean
  under default.
* `cargo clippy -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --all-targets -- -D warnings` clean under
  the legacy fMP4 path.
* `cargo fmt --all --check` clean.

### What session 13 did NOT land

* **Audio in the RTMP-driven E2E test**. The
  `rtmp_hls_e2e.rs` test still publishes video only. Extending
  it to publish FLV audio requires AAC sequence-header plumbing
  and an audio-aware FLV fixture. The audio bridge code path is
  exercised by the new `integration_master.rs` test through the
  `MultiHlsServer` API directly. A full RTMP-publish-with-audio
  E2E lands in a later session, ideally bundled with the
  `rtmp_ws_e2e.rs` audio extension that the WS handler already
  expects.
* **Real bandwidth and resolution in the master playlist**.
  Session 13 emits a hardcoded `BANDWIDTH=2500000`,
  `CODECS="avc1.640020,mp4a.40.2"`, no `RESOLUTION`. Real values
  will come from the producer-side catalog once the codec
  parsers feed back into the bridge -- tracked as part of the
  Tier 2.2 codec wiring.
* **Per-rendition mediastreamvalidator coverage**. The
  `lvqr-hls` conformance slot still soft-skips when Apple's
  `mediastreamvalidator` is not on PATH. Adding a master-playlist
  conformance test is a follow-up once the validator is
  installed in CI.
* **Deletion of the hand-rolled fMP4 writer**. Session 12.2 set
  the `legacy-fmp4` marker and flipped the default; deletion is
  still a session 14 candidate (the cmaf-writer matrix has now
  been green on main for one session of additions on top, which
  is the soonest a "release cycle" can be argued).

### Recommended entry point (session 14)

The session-11 work list is now closed apart from item 3 (WHEP).
The natural session-14 picks:

1. **Delete the hand-rolled fMP4 writer behind `legacy-fmp4`**.
   Removes `lvqr_ingest::remux::fmp4::video_segment` plus its
   unit tests, the golden tests at `tests/golden_fmp4.rs` that
   reference it, the proptest target at
   `tests/proptest_parsers.rs::video_segment_is_well_formed`,
   the parity tests at `tests/parity_avc_segment.rs` +
   `tests/parity_avc_init.rs`, the `legacy-fmp4` feature on
   `lvqr-ingest/Cargo.toml`, and the `test-legacy-fmp4-path`
   CI job. Mechanical, half-session.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`. Full session by itself.
3. **Real RTMP-publish-with-audio E2E**. Extend
   `rtmp_hls_e2e.rs` and `rtmp_ws_e2e.rs` to publish FLV audio
   alongside the existing video sequence so the audio path
   through the bridge gets covered end-to-end. Half-session
   bundled with item 1.

My recommendation: pair **(1) deletion + (3) audio E2E**. Both
are mechanical, both compound the session 12 + 12.2 + 13 work
into a coherent "Tier 2.3 closed" milestone. Item 2 (WHEP) is
the right pick for session 15 once Tier 2.3 is fully closed.

## Session 12.2 (2026-04-13): `cmaf-writer` default-on + `legacy-fmp4` marker

Session 12.2 closed item 4 from the session-11 work list: flip
`lvqr-ingest`'s `cmaf-writer` feature to default-on and move the
in-crate hand-rolled fMP4 writer into retirement under a
`legacy-fmp4` marker feature. No code was gated out this session;
the retirement is a bookkeeping + dispatch flip, and a CI job
continues to exercise the hand-rolled path end-to-end until it is
deleted in a later session. Four files touched:

1. **`crates/lvqr-ingest/Cargo.toml`**. `default = ["rtmp"]` ->
   `default = ["rtmp", "cmaf-writer"]`. Added `legacy-fmp4 = []`
   as a marker feature. `cmaf-writer`'s docstring updated to
   reflect the new default state; `legacy-fmp4`'s docstring
   names the code slated for deletion in the next session and
   points at the CI matrix job that still exercises it.

2. **`crates/lvqr-cli/Cargo.toml`**. Added `cmaf-writer` to
   `default` and `full`. Changed the `lvqr-ingest` dep from
   `workspace = true` to an inline path dep with
   `default-features = false` so the `--no-default-features
   --features rtmp,quinn-transport` CI invocation actually
   cascades through and disables `cmaf-writer` transitively.
   Cargo disallows overriding a workspace dep's
   `default-features` when a crate inherits via `workspace =
   true`, so the path dep is inlined here with the same version
   pin to keep the graph consistent. Workspace `Cargo.toml`
   stays untouched.

3. **`crates/lvqr-cli/src/lib.rs`** (WS ingest handler). The
   WS ingest path was calling `remux::video_segment` directly,
   bypassing the `build_video_segment` dispatch. Switched both
   call sites (keyframe + delta) to `remux::build_video_segment`
   so the WS-ingest bridge honors the feature flag exactly like
   the RTMP-ingest bridge does. Under the new default this
   routes through `lvqr_cmaf::build_moof_mdat`.

4. **`.github/workflows/ci.yml`**. Renamed `test-cmaf-writer` to
   `test-legacy-fmp4-path`. The job now runs
   `cargo build -p lvqr-cli --no-default-features --features
   rtmp,quinn-transport`, `cargo test -p lvqr-ingest
   --no-default-features --features rtmp`, and `cargo test -p
   lvqr-cli --no-default-features --features rtmp,quinn-transport`
   so both the bridge dispatch and the two E2E integration tests
   exercise the legacy writer on every PR. The job cache-prefix
   was updated to `legacy-fmp4-v1`.

### Verification run

* `cargo test --workspace` -- 66 binaries, 0 failures (default:
  cmaf-writer on).
* `cargo test -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --test rtmp_hls_e2e --test rtmp_ws_e2e` --
  both E2E tests green under the legacy writer path (verified the
  legacy dispatch branch is active by inspecting `cargo tree
  --no-default-features --features rtmp,quinn-transport` and
  confirming `lvqr-ingest` no longer pulls `lvqr-cmaf` as a
  normal dep under this config).
* `cargo test -p lvqr-ingest --test parity_avc_init --test
  parity_avc_segment` -- both parity gates green (they run under
  the default config, which has both writers available: the
  hand-rolled one is unconditionally compiled for this cycle,
  and `lvqr-cmaf` is on as a dev-dep).
* `cargo clippy --workspace --all-targets -- -D warnings` clean
  under the default feature set.
* `cargo clippy -p lvqr-cli --no-default-features --features
  rtmp,quinn-transport --all-targets -- -D warnings` clean on the
  legacy path.
* `cargo fmt --all --check` clean.

### What session 12.2 did NOT land

* **Deletion of the hand-rolled `video_segment` writer.** That is
  the follow-up session's job, now that the feature flag is in
  place and the cycle clock has started. Deletion removes
  `remux::fmp4::video_segment` plus its unit tests, the golden
  file tests at `tests/golden_fmp4.rs` that still reference it,
  the proptest target at `tests/proptest_parsers.rs::video_segment_is_well_formed`,
  the parity tests at `tests/parity_avc_init.rs` +
  `tests/parity_avc_segment.rs`, the `legacy-fmp4` feature on
  `lvqr-ingest/Cargo.toml`, and the `test-legacy-fmp4-path` CI
  job. That is a cohesive single-commit change once a future
  session decides the retirement cycle is over.
* **Audio rendition group in HLS.** Still deferred; see the
  session 12 entry below.
* **`lvqr-whep` implementation.** Still scoping-doc only.

### Recommended entry point (session 13)

With session 12.2's flip landed, the session 11 work list has
shrunk to two remaining candidates:

1. **Audio rendition group in HLS** (was item 2). Scope
   unchanged. Forces `lvqr-hls` to learn master-playlist
   (`EXT-X-STREAM-INF`) generation. Full-session item.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`. Full-session item.

Plus the natural follow-up from this session:

3. **Delete the hand-rolled fMP4 writer**. Scoped above. The
   risk here is purely "did we miss a caller?"; the CI job has
   been guarding the dispatch for multiple sessions and the
   parity gate has caught every drift during the transition. A
   half-session task if it is bundled with one of items 1 or 2.

My recommendation: pair **(3) deletion + (1) audio rendition
group start**. Deletion is mechanical enough to fit in the first
half of a session; the remaining time covers adding master-playlist
scaffolding to `lvqr-hls` and a single-rendition master-playlist
rendering test. If (1) blows the time budget, item (3) can be
deferred to the next cycle with no harm done -- the flag is
already in place and the CI job already exists.

## Session 12 (2026-04-13): multi-broadcast LL-HLS routing

Session 12 closed item 1 from the session-11 work list
("Multi-broadcast HLS routing"). One logical change landed across
four files:

1. **`MultiHlsServer` in `lvqr-hls`**
   (`crates/lvqr-hls/src/server.rs`). New type that owns a
   `std::sync::Mutex<HashMap<String, HlsServer>>` keyed by broadcast
   name plus a template `PlaylistBuilderConfig` for lazily creating
   per-broadcast state. Exposes `ensure_broadcast(name) -> HlsServer`
   for the producer side, `get_broadcast(name) -> Option<HlsServer>`
   for the consumer side (so unknown broadcasts return 404 instead
   of an empty 200), `broadcast_count()` for tests, and
   `router()` which mounts a single `/hls/{*path}` catch-all.
   The catch-all exists because broadcast names contain a slash
   today (the RTMP bridge names broadcasts `{app}/{key}`, e.g.
   `live/test`), so a simple `/hls/{broadcast}/...` path param
   would not capture them. A `split_broadcast_path` helper splits
   the tail off the path, matches it against `playlist.m3u8`,
   `init.mp4`, or a chunk URI, and dispatches to one of three
   new shared `render_*` helpers extracted from the old free
   handlers. The single-broadcast `HlsServer::router()` still
   exists and still works; the `render_playlist` / `render_init`
   / `render_uri` helpers are the only rendering path, so the
   blocking-reload semantic lives in one place.

2. **`HlsFragmentBridge` in `lvqr-cli`**
   (`crates/lvqr-cli/src/hls.rs`). Rewritten around
   `MultiHlsServer`. Removed the "first broadcast wins" logic;
   every broadcast that publishes a video track now gets its own
   per-broadcast `CmafPolicyState` keyed by broadcast name in a
   `Mutex<HashMap<String, CmafPolicyState>>`. A fresh
   `VIDEO_90KHZ_DEFAULT` entry is installed the first time a
   broadcast publishes its init segment; a new init on the same
   broadcast resets the entry so a mid-stream codec change starts
   from a clean slate. Audio is still ignored here; audio
   rendition groups land separately when `lvqr-hls` grows
   master-playlist support.

3. **`lvqr-cli::start()`** (`crates/lvqr-cli/src/lib.rs`). Swapped
   the single `HlsServer::new(...)` construction for
   `MultiHlsServer::new(...)`. The axum serve task still just
   calls `server.router()`; the import line is the only other
   change. `ServerHandle::hls_url` stays unchanged (still a base-
   URL helper); tests compose `/hls/{broadcast}/...` paths
   explicitly.

4. **`crates/lvqr-cli/tests/rtmp_hls_e2e.rs`**. Renamed to
   `rtmp_publish_reaches_multi_broadcast_hls_router`. Extracted
   the publish-two-keyframes sequence into a
   `publish_two_keyframes(addr, app, key)` helper and the
   playlist-fetch-and-parse check into a
   `fetch_playlist_and_part_uris(hls_addr, app, key)` helper.
   The test now publishes two concurrent RTMP broadcasts
   (`live/one` and `live/two`) to the same `TestServer`, fetches
   `/hls/live/one/playlist.m3u8` and `/hls/live/two/playlist.m3u8`,
   asserts each playlist is well-formed LL-HLS (starts with
   `#EXTM3U`, carries `#EXT-X-VERSION:9`, names `init.mp4` via
   `#EXT-X-MAP`, and references at least one `#EXT-X-PART:` URI),
   fetches one part from each broadcast and asserts both bodies
   start with a `moof` box, fetches `/hls/live/one/init.mp4` and
   `/hls/live/two/init.mp4` and asserts both start with `ftyp`,
   and finally fetches `/hls/live/ghost/playlist.m3u8` and
   asserts it returns 404. Passed first run under both the
   default feature set and `--features cmaf-writer`.

### What session 12 did NOT land

* **`cmaf-writer` flipped to default-on.** The session 11
  directive called for at least one release cycle on main before
  flipping; session 12 honored that by leaving the feature
  default-off. Candidate for session 13 if the matrix stays
  green.
* **Hand-rolled `video_segment` retirement behind `legacy-fmp4`.**
  Same gating. Parity test at
  `crates/lvqr-ingest/tests/parity_avc_segment.rs` still owns
  the correctness property.
* **Audio rendition group in HLS.** Still deferred. Forces
  `lvqr-hls` to learn master-playlist / `EXT-X-STREAM-INF`
  generation; scoped as a full session by itself in the session
  11 handoff.
* **`lvqr-whep` implementation.** Still scoping-doc only at
  `crates/lvqr-whep/docs/design.md`.

### Recommended entry point (session 13)

The four candidates from session 11 minus the one that landed:

1. **Audio rendition group in HLS** (was item 2). Scope
   unchanged; forces master-playlist generation in `lvqr-hls`.
2. **Begin `lvqr-whep` implementation** (was item 3). Needs a
   `RawSampleObserver` hook on `RtmpMoqBridge` plus answers to
   the four open questions in
   `crates/lvqr-whep/docs/design.md`.
3. **Flip `cmaf-writer` to default-on + retire the hand-rolled
   writer behind `legacy-fmp4`** (was item 4). Session 11's
   gating language ("at least one release cycle on main") is
   now satisfiable: the matrix shipped in session 11, session 12
   added to the surface it exercises, and both writer paths
   stayed green.

The safest single-session pair is (3) plus a start on (1):
flipping `cmaf-writer` is mechanical once the release-cycle
clock is up, and master-playlist work in `lvqr-hls` is
incremental enough that even landing just the master-playlist
type plus a single-rendition rendering test is forward
progress toward (1).

## Session 11 (2026-04-13): CLI HLS composition + `cmaf-writer` feature flag

Session 11 closed every item from the "Recommended Tier 2.3 entry
point (session 11)" work list below. Two commits land on top of
`f83a280` (the session-10 audit + handoff refresh):

1. **Dev-dep cycle broken** (item 1). Both `parity_avc_init.rs` and
   `parity_avc_segment.rs` moved out of `crates/lvqr-cmaf/tests/`
   and into `crates/lvqr-ingest/tests/`. `lvqr-cmaf` no longer
   dev-deps `lvqr-ingest`; `lvqr-ingest` now dev-deps `lvqr-cmaf`
   plus `mp4-atom = "0.10"` for the `Moof::decode` calls the parity
   tests need. The dep direction is one-way, which is what session
   11 item 3 requires. Both parity tests still pass byte-for-byte
   identically to the session-10 baseline (sizes equal at 600 bytes,
   bytes intentionally differ, structural fields match).

2. **`lvqr-cli serve` composes HLS** (item 2). Three pieces:

   * **`FragmentObserver` hook in `lvqr-ingest`**
     (`crates/lvqr-ingest/src/observer.rs`). New trait with
     `on_init(&self, broadcast, track, init: Bytes)` and
     `on_fragment(&self, broadcast, track, fragment: &Fragment)`.
     The bridge gets a builder method
     `RtmpMoqBridge::with_observer(SharedFragmentObserver)` plus a
     `set_observer` mutator. Both the video and audio paths fire
     `on_init` when an init segment becomes available and
     `on_fragment` after each `MoqTrackSink::push`. The bridge stays
     HLS-agnostic; the trait is the only wire between RTMP ingest
     and any non-MoQ consumer.

   * **`HlsFragmentBridge` in `lvqr-cli`**
     (`crates/lvqr-cli/src/hls.rs`). Implements
     `FragmentObserver`. Uses `lvqr_cmaf::CmafPolicyState` directly
     (re-exported from `lvqr-cmaf` this session) to classify each
     fragment as `Partial` / `PartialIndependent` / `Segment`, then
     spawns a tokio task per push that forwards the resulting
     `CmafChunk` into a shared `HlsServer`. Single-rendition
     today: the first broadcast that publishes a video track wins
     and subsequent broadcasts have their fragments dropped (with a
     `tracing::info!` at attach time so production operators see
     it). Multi-broadcast routing is a follow-up; the integration
     test publishes one broadcast so the limit is invisible at the
     contract layer.

   * **`ServeConfig.hls_addr` + `ServerHandle::hls_addr` /
     `hls_url`** (`crates/lvqr-cli/src/lib.rs`). New optional
     `hls_addr: Option<SocketAddr>` field on `ServeConfig`. When
     set, `start()` builds an `HlsServer`, attaches an
     `HlsFragmentBridge` observer to the RTMP bridge, pre-binds the
     HLS TCP listener, and spins up a fourth `axum::serve` task
     under the same shutdown token as the relay / RTMP / admin
     subsystems. `ServerHandle` grows `hls_addr() ->
     Option<SocketAddr>` and `hls_url(path: &str) -> Option<String>`
     accessors. `lvqr-cli serve` gains a `--hls-port` /
     `LVQR_HLS_PORT` flag (default `8888`, set to `0` to disable).
     `TestServer` enables HLS by default and exposes `hls_addr` /
     `hls_url` helpers; `TestServerConfig::without_hls()` turns the
     surface off for tests that do not need it.

   * **`crates/lvqr-cli/tests/rtmp_hls_e2e.rs`**. Real end-to-end
     test: spin up a `TestServer`, RTMP-publish two keyframes
     spaced 2.1 s apart through `rml_rtmp` so the segmenter's
     default `VIDEO_90KHZ_DEFAULT` policy closes one full segment,
     then drive a 30-line raw-TCP HTTP/1.1 client against
     `/playlist.m3u8`, assert the body contains an `EXTM3U` header,
     `EXT-X-VERSION:9`, `EXT-X-MAP`, and at least one
     `#EXT-X-PART` URI, fetch one of those URIs and assert the
     body starts with a `moof` box, then fetch `/init.mp4` and
     assert the body starts with `ftyp`. Passed first run. The
     intentional zero-new-deps choice (raw HTTP/1.1 vs. pulling
     `reqwest` or `hyper-util`) keeps the dev-dep budget small.

3. **`cmaf-writer` feature flag on `lvqr-ingest`** (item 3). New
   default-off feature `cmaf-writer` on `lvqr-ingest` that pulls in
   `lvqr-cmaf` as an optional normal dep. When the feature is on,
   the bridge's per-frame video media segment is built via a new
   `lvqr_ingest::remux::fmp4::build_video_segment` helper that
   delegates to `lvqr_cmaf::build_moof_mdat` instead of the
   hand-rolled `video_segment`. The hand-rolled path stays in
   place under the default feature set so the parity gate keeps
   working. `lvqr-cli` exposes a passthrough `cmaf-writer` feature
   so `cargo test -p lvqr-cli --features cmaf-writer` flips both
   crates in one shot. New `test-cmaf-writer` CI matrix job in
   `.github/workflows/ci.yml` that builds `lvqr-cli` with the
   feature on, runs `cargo test -p lvqr-ingest --features
   cmaf-writer`, and runs `cargo test -p lvqr-cli --features
   cmaf-writer` so both `rtmp_ws_e2e` and `rtmp_hls_e2e` exercise
   the alternate writer end-to-end on every PR. Both E2E tests
   pass under both writers locally.

4. **`lvqr-whep` scoping doc** (item 4).
   `crates/lvqr-whep/docs/design.md`. **No code, no Cargo.toml,
   not a workspace member.** The directory contains exactly one
   markdown file. Covers what WHEP needs from `CmafChunk` (path A:
   consume `RawSample` via `SampleStream`, preferred; path B: parse
   `mdat` on the wire, transitional shim), how the offer / trickle /
   terminate signaling maps onto axum routes in the same shape as
   `HlsServer::router`, the existing crates that get reused
   (`lvqr-cmaf::SampleStream`, `lvqr-fragment`, the Fragment
   Observer pattern, `lvqr-hls::server` as a routing template,
   `lvqr-auth`, `lvqr-core::EventBus`), the new external dep
   (`str0m`), the 5-artifact plan with concrete test file paths,
   sequencing constraints (waits on bridge raw-sample emission via
   either the `cmaf-writer` cutover or a sibling `RawSampleObserver`
   hook), and four open questions for the implementation session.

### What session 11 did NOT land

* **Multi-broadcast HLS routing.** `HlsFragmentBridge` is
  single-rendition / single-broadcast today. The router serves
  one playlist; subsequent RTMP broadcasts are tracked but their
  fragments are silently dropped. Adding a `/hls/{broadcast}/...`
  prefix or one `HlsServer` per broadcast is a session-12 follow-up.
* **Audio in HLS.** The `FragmentObserver::on_fragment` hook fires
  for both `0.mp4` (video) and `1.mp4` (audio), but
  `HlsFragmentBridge` only consumes video. Multi-track HLS (audio
  rendition group) lands when multi-track HLS lands.
* **`cmaf-writer` flipped to default-on.** Per the directive, the
  feature is default-off this session. The CI matrix exercises
  both paths; the default flips after the matrix has been green on
  main for a few release cycles.
* **Hand-rolled `video_segment` deletion.** Same gating as the
  default flip. The parity gate at
  `crates/lvqr-ingest/tests/parity_avc_segment.rs` keeps both
  writers honest until the deletion lands.
* **WHEP implementation.** Scoping only this session, per the
  directive.

### Contract slot status as of session 11

| Crate          | proptest | fuzz | integration | E2E              | conformance |
| lvqr-ingest    | yes      | yes  | yes         | yes              | yes         |
| lvqr-codec     | yes      | yes  | yes         | via rtmp_ws_e2e  | yes (multi-sub-layer covered) |
| lvqr-cmaf      | yes      | open | yes         | via rtmp_ws_e2e  | yes (AVC + HEVC + AAC init, AVC + AAC coalescer) |
| lvqr-hls       | yes      | open | yes         | via oneshot + lvqr-cli rtmp_hls_e2e | soft-skip (mediastreamvalidator) |
| lvqr-record    | yes      | open | yes         | workspace e2e    | yes         |
| lvqr-moq       | yes      | open | yes         | via rtmp_ws_e2e  | n/a         |
| lvqr-fragment  | yes      | open | yes         | via rtmp_ws_e2e  | n/a         |

`lvqr-hls` E2E slot grew from "via oneshot" alone to "via oneshot
plus lvqr-cli rtmp_hls_e2e". The `oneshot` test still runs every
HLS handler over the axum service trait; the new `rtmp_hls_e2e`
runs the same router over a real loopback TCP socket end-to-end
from RTMP publish to HTTP GET.

### Dependency graph snapshot (post-session-11)

* `lvqr-cli` normal-deps `lvqr-cmaf`, `lvqr-hls`, `lvqr-fragment`
  (added this session for the HLS bridge).
* `lvqr-hls` normal-deps `lvqr-cmaf`.
* `lvqr-ingest` normal-deps `lvqr-cmaf` ONLY when the `cmaf-writer`
  feature is on (optional dep). The default feature set leaves the
  dep edge absent.
* `lvqr-ingest` dev-deps `lvqr-cmaf` + `mp4-atom` for the parity
  tests (one-way, no cycle).
* `lvqr-cmaf` no longer dev-deps anything in the producer side.
* No other dep edges changed. Graph is still acyclic.

## Sessions 6-10 audit (2026-04-13, pre-session-11)

Ran a structural audit before writing the session 11 kickoff
prompt. Findings:

1. **Tree health**. `cargo fmt --all --check`, `cargo clippy
   --workspace --all-targets -- -D warnings`, and `cargo test
   --workspace` all pass cleanly. 241 tests across 60 binaries.
   Local HEAD matches `origin/main`. Working tree is clean.

2. **Contract script drift**. `scripts/check_test_contract.sh`
   had `lvqr-hls` commented out in the "will be enabled as they
   land" section. Session 7 landed the crate and sessions 7-8
   closed 4-of-5 slots. Fixed in the audit commit: `lvqr-hls`
   moved into the `IN_SCOPE` list. The script now reports one
   crate-level warning per session 10 expected open slot (fuzz
   on lvqr-cmaf, lvqr-hls, lvqr-record, lvqr-moq, lvqr-fragment)
   and nothing unexpected.

3. **CONTRACT.md staleness**. `tests/CONTRACT.md` did not list
   `lvqr-hls`, still named the `mediastreamvalidator` wrapper
   and the kvazaar fixture as open items (both closed in
   sessions 7 and 8 respectively), and did not mention the
   coalescer conformance or the sample-segmenter integration
   test. Rewritten in the audit commit to reflect the real
   session 10 contract-slot state.

4. **Producer wiring gap (intentional)**. `TrackCoalescer`,
   `RawSample`, `SampleStream`, and `CmafSampleSegmenter` have
   zero consumers in `lvqr-ingest` / `lvqr-cli` / any producer
   crate. The raw-sample pipeline is fully tested inside
   `lvqr-cmaf` via scripted `VecDeque`-backed streams but does
   not yet drive a real ingest. This is expected; session 11 is
   where the wiring lands. Flagged so future sessions do not
   assume the pipeline is in production use.

5. **Expect/unwrap in coalescer production paths**. Five
   `.expect()` / `.unwrap()` calls in `crates/lvqr-cmaf/src/coalescer.rs`:
   three are state-machine precondition enforcement (pending
   implies partial_start / segment_start / pending_dts) and two
   are `mp4-atom` encoder calls that can only fail on
   structurally invalid `Moof` inputs. All are invariant-
   protected; none take untrusted input. No action required
   today but worth noting for future hardening.

6. **CI coverage**. `.github/workflows/ci.yml` installs ffmpeg on
   both Linux and macOS runners so every `ffprobe_bytes` check
   runs for real. It does NOT install kvazaar (not needed, the
   multi-sub-layer HEVC fixture is pinned bytes now) or
   `mediastreamvalidator` (soft-skip handles absence). Contract
   script still runs in Tier 1 educational mode; strict mode
   flips on when the remaining fuzz slots are either closed or
   documented as intentionally open.

7. **Dependency graph snapshot**. `lvqr-hls` normal-deps
   `lvqr-cmaf`. `lvqr-cmaf` dev-deps `lvqr-ingest` (for the
   parity tests only). `lvqr-ingest` does NOT depend on
   `lvqr-cmaf` in either direction today. Clean acyclic graph.
   Session 11 item 2 (feature flag retirement) requires
   `lvqr-ingest` to normal-dep `lvqr-cmaf`, which creates a
   cycle with the current dev-dep direction. The fix is to
   move `parity_avc_init.rs` and `parity_avc_segment.rs` out of
   `lvqr-cmaf/tests/` and into a top-level workspace test
   crate (or into `lvqr-ingest/tests/`) before flipping the
   normal-dep direction. Documented here rather than in the
   individual commits so session 11 does not trip on it.

## Session 10 follow-ups (2026-04-13): TrackCoalescer pipeline

Two commits landed between session 9 and the session 10 audit,
closing the remaining tier-2.3 items that did not require touching
the CLI composition root:

1. **Coalescer parity gate against the hand-rolled writer**
   (`crates/lvqr-cmaf/tests/parity_avc_segment.rs`, commit
   `6d41c5a`). Drives the same six-sample batch through both
   `lvqr_cmaf::TrackCoalescer` and
   `lvqr_ingest::remux::fmp4::video_segment`, decodes both
   `moof` boxes via `mp4_atom::Moof::decode`, and asserts every
   playback-critical field matches: `mfhd` sequence number,
   `tfhd` track id, `tfdt` base_media_decode_time, `trun`
   entry count, per-sample duration/size/flags/cts offset, and
   `data_offset` landing inside its own buffer. The two writers
   produce media segments of **identical total size** for this
   input (`cmaf=600, ingest=600, delta=0`), though not identical
   bytes. A second test pins the intentional non-equality so a
   future session cannot silently replace the structural gate
   with a byte-equality assertion.

2. **AAC audio coalescer ffprobe round trip**. Extended
   `crates/lvqr-cmaf/tests/conformance_coalescer.rs` with an
   AAC variant: 20 synthetic AAC frames (1024 ticks each, 128
   bytes of zero payload) through a `TrackCoalescer` +
   `CmafPolicy::AUDIO_48KHZ_DEFAULT`, concatenated with the
   `write_aac_init_segment` output and fed to ffprobe 8.1.
   ffprobe accepts. Both video and audio paths through the
   coalescer now have real-encoder validation on top of the
   structural unit tests.

3. **SampleStream trait + CmafSampleSegmenter**
   (`crates/lvqr-cmaf/src/sample.rs`, `crates/lvqr-cmaf/src/coalescer.rs`,
   `crates/lvqr-cmaf/tests/integration_sample_segmenter.rs`,
   commit `c24fe51`). Closes session 10 item 1 from the prior
   HANDOFF: a pull-based trait `SampleStream` with
   `next_sample()` returning `Pin<Box<dyn Future<Output =
   Option<RawSample>> + Send>>` (boxed future instead of
   async-fn-in-trait because Send bounds still require GAT
   plumbing this crate does not need), and a
   `CmafSampleSegmenter` type that owns a
   `HashMap<TrackId, TrackCoalescer>`, routes incoming samples
   into the right coalescer, queues the resulting chunks into a
   ready buffer, and drains every coalescer's trailing pending
   batch on stream exhaustion before returning `None`.

   Integration tests cover a single-track ffprobe round trip
   through the full pipeline (init segment + chunks concatenated
   and accepted by ffprobe 8.1) and a multi-track routing test
   that interleaves video (track 1) and audio (track 2) and
   asserts both tracks produce chunks tagged with the correct
   `"{track_id}.mp4"` string.

### What session 10 did NOT land

* **`lvqr-cli` HLS composition** -- the CLI serve path does not
  yet expose an HLS axum binding. Blocker for `TestServer`
  growing a real HLS address and for the loopback TCP E2E.
* **Hand-rolled `video_segment` retirement** behind a feature
  flag. Blocked on the dev-dep cycle surfaced by the audit
  (item 7 above).
* **First non-HLS egress crate** (WHEP / DASH / WHIP). Per the
  audit list, this waits until the CLI composition lands so the
  egress crates can be validated against a real end-to-end
  pipeline rather than a standalone router harness.

## Session 9 additions (2026-04-13): raw-sample TrackCoalescer

Session 8 closed every item from the session-8 work list; session 9
inherits the remaining Tier 2.3 items. This session lands the
largest of those three: the raw-sample coalescer scoped by the
session-7 design note in `lvqr-cmaf::segmenter`.

1. **`RawSample` type** (`crates/lvqr-cmaf/src/sample.rs`). Minimal
   producer-side value type carrying `track_id`, `dts`, `cts_offset`,
   `duration`, `payload`, and `keyframe`. The payload layout is
   codec-defined: AVCC length-prefixed for AVC/HEVC, raw AU for
   AAC. The producer is authoritative for every field; the
   coalescer never re-parses the payload to infer keyframe status
   or re-derives DTS from PTS. `RawSample::keyframe` and
   `RawSample::delta` constructors cover the common AVC Baseline
   and audio cases without a struct literal.

2. **`TrackCoalescer` state machine**
   (`crates/lvqr-cmaf/src/coalescer.rs`). Per-track pure state
   machine that accumulates `RawSample` values and flushes them on
   partial / segment boundaries as `CmafChunk` values. State
   transitions mirror the session-7 design note exactly: on push,
   if the pending batch exists and the new sample crosses a
   partial boundary OR a segment-window keyframe, flush the batch
   and return the chunk; otherwise append. The returned chunk
   carries the `pending_kind` that was fixed when the batch was
   opened, so later samples inside the same partial window cannot
   change the chunk's kind retroactively. A trailing `flush` at
   end-of-stream drains whatever is still pending.

3. **`build_moof_mdat` writer**. The coalescer's `flush_pending`
   builds a wire-ready `moof + mdat` pair via `mp4-atom`'s
   `Moof` / `Mfhd` / `Traf` / `Tfhd` / `Tfdt` / `Trun` types. The
   `trun.data_offset` field is computed via a two-pass encode: the
   first pass populates `data_offset = 0` to measure the moof
   size; the second pass re-encodes with `data_offset = moof_size
   + 8`. Every field in the moof is fixed-width so the total size
   is stable across the two encodes. The mdat header is written
   by hand (4 bytes size + 4 bytes `"mdat"`) rather than through
   `mp4-atom::Mdat` so the per-sample payload `Bytes` blobs are
   extended into the buffer without an intermediate `Vec<u8>`
   copy. Sample flags use the same `0x02000000` sync / `0x01010000`
   non-sync layout the hand-rolled writer at
   `lvqr-ingest::remux::fmp4::video_segment` ships today, so
   byte-level diffs against the hand-rolled path see identical
   sample-flag fields.

4. **ffprobe-validated round trip**
   (`crates/lvqr-cmaf/tests/conformance_coalescer.rs`). New
   integration test that builds a real AVC init segment via
   `write_avc_init_segment`, pushes 10 AVCC-wrapped synthetic
   samples (one IDR + nine P-slices) through a `TrackCoalescer`,
   concatenates the init segment with every chunk's payload, and
   runs the whole thing through ffprobe 8.1 via the soft-skip
   helper. **ffprobe accepts the output.** This is the first
   real-encoder-validated proof that the mp4-atom-backed
   coalescer produces sound CMAF output and that the two-pass
   `data_offset` patch lands at the right byte offset.

5. **Lib-level unit tests**. Five new tests in
   `coalescer.rs::tests` cover: first sample does not flush,
   partial boundary flushes pending, segment boundary fires on
   keyframe past window, `flush` drains pending at end-of-stream,
   and the moof structure round-trips through mp4-atom's own
   decoder (asserting sequence number, track id, tfdt DTS, trun
   entry count and sizes, and the data_offset placeholder
   position).

### What the coalescer is NOT yet wired into

* **`CmafSegmenter::from_sample_stream`** constructor. The
   design note scheduled it as part of this session's deliverable.
   Deferred because the existing `CmafSegmenter::new` consumes a
   `FragmentStream` (pre-muxed) and the `TrackCoalescer` operates
   at the `RawSample` level; unifying the two under one segmenter
   type requires a `SampleStream` trait and the producer side
   does not yet emit raw samples. Session 10 wires it when the
   first producer migrates.
* **The RTMP bridge**. `lvqr-ingest::remux::fmp4::video_segment`
   still ships the media segments for the `rtmp_ws_e2e` path.
   Retirement behind a feature flag requires flipping the
   `lvqr-ingest` -> `lvqr-cmaf` dep direction (currently
   `lvqr-cmaf` dev-deps `lvqr-ingest` for the parity test); the
   cleanest migration is to move the parity test out of
   `lvqr-cmaf` and into a top-level workspace test, or to accept
   the test-only dev-dep cycle. Deferred to session 10.
* **Audio coalescing**. The state machine is codec-agnostic but
   the ffprobe round-trip test only exercises video. Audio works
   by construction (every sample is a keyframe, so every chunk
   fires a partial boundary cleanly), but no test covers it yet.

### Contract slot status as of session 9

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC init + coalescer) |
| lvqr-hls | y | open (no parser surface) | y | via router oneshot | soft-skip (mediastreamvalidator) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-cmaf`'s conformance slot grew from "AVC + HEVC + AAC init"
to "AVC + HEVC + AAC init + coalescer". The coalescer test is the
first one in the crate that exercises both the init writer and
the media writer through a single ffprobe check, which is the
minimal shape of a real segmenter->consumer handshake.

## Session 8 additions (2026-04-13)

## Session 8 additions (2026-04-13)

Session 8 took the `lvqr-hls` crate from 2-of-5 to 4-of-5 contract
slots in a single run. Two commits landed on main:

1. **`lvqr-hls` axum router with LL-HLS blocking reload**
   (`crates/lvqr-hls/src/server.rs`). Adds `HlsServer` on top of the
   session-7 `PlaylistBuilder` so real HLS clients can GET a
   playlist, the init segment, and every part / segment URI the
   manifest references. Four routes: `GET /playlist.m3u8`,
   `GET /init.mp4`, `GET /{uri}` catch-all. The playlist handler
   honors `_HLS_msn=N` and `_HLS_msn=N&_HLS_part=M` query parameters
   via `tokio::sync::Notify::notify_waiters()` on every push, with
   a three-target-duration hold-back ceiling so a stalled producer
   cannot hang subscribers indefinitely. Producer API:
   `HlsServer::push_init(bytes)` (idempotent), `push_chunk_bytes`
   (pushes into `PlaylistBuilder` and caches the payload under the
   URI the builder generated), `close_pending_segment`
   (end-of-stream hook). `HlsServer` wraps an `Arc<HlsState>` and
   is cheap to clone; the same handle lives on both the producer
   side and the router side. Shared state uses
   `tokio::sync::RwLock + HashMap` rather than dashmap so no new
   transitive dep lands for a footprint with zero lock contention
   worth tuning.

   Integration coverage in `tests/integration_server.rs`: four test
   cases driving real HTTP requests through the router via
   `tower::ServiceExt::oneshot + http-body-util` so the whole
   handler surface is exercised end-to-end, just over the axum
   service trait instead of a loopback TCP socket. Cases cover
   playlist + init + segment round trip, `/init.mp4` 404 before
   push, unknown URI 404, and `_HLS_msn=1` blocking reload with a
   real parked future that only resolves after a second publish
   wakes the `Notify`.

2. **`mediastreamvalidator` soft-skip helper and lvqr-hls
   conformance slot**
   (`crates/lvqr-test-utils/src/lib.rs`,
   `crates/lvqr-hls/tests/conformance_manifest.rs`). Adds
   `lvqr_test_utils::mediastreamvalidator_playlist`, a soft-skip
   wrapper around Apple's `mediastreamvalidator` tool following the
   same pattern as the existing `ffprobe_bytes` helper. The wrapper
   writes a rendered playlist plus its `(uri, bytes)` segment map
   into a tempdir and invokes the validator against the playlist
   path. When the tool is not on PATH the helper returns `Skipped`;
   when it is installed locally the caller gets real validator
   output and `assert_accepted()` panics on a non-zero exit with
   the validator's stdout attached.

   Apple's `mediastreamvalidator` is part of a free Developer
   download that is not on Homebrew, so it is not installed in CI
   either. The soft-skip path is the common case today; the test
   upgrades to a real validator run automatically the moment the
   binary appears on PATH. The helper itself builds unconditionally.

   The new `conformance_manifest.rs` test builds a minimal
   two-segment manifest via `HlsServer`, harvests the rendered
   playlist through a `tower::oneshot` call against the router
   (so the bytes exactly match what a real HTTP client would see),
   and hands the playlist plus stub segment bodies to the new
   helper. Stub bodies are intentional: the `TrackCoalescer`
   design note in `lvqr-cmaf::segmenter` schedules real producer
   bytes for a later session, and the soft-skip path keeps the
   test green until then.

### Contract slot status as of session 8

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer now covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-hls | y | open (no parser surface) | y | via router oneshot | soft-skip (mediastreamvalidator) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-hls` is now 4-of-5. The fuzz slot stays intentionally open
because the crate has no parser attack surface (the router only
reads structured input produced by the `PlaylistBuilder`). The E2E
slot is filled by the `router oneshot` path rather than a loopback
TCP socket; a real TCP E2E lands when `lvqr-cli` composes HLS into
its serve path and `lvqr-test-utils::TestServer` grows an HLS
address.

### What session 8 did NOT land

* **`TrackCoalescer` implementation**. Still the largest deferred
  Tier 2.3 item. Design note lives in
  `crates/lvqr-cmaf/src/segmenter.rs`; session 9 implements the
  `RawSample` / `SampleStream` trait pair, the `TrackCoalescer`
  state machine, `CmafSegmenter::from_sample_stream`, and the
  round-trip test against `lvqr-ingest::remux::fmp4::video_segment`
  output.
* **`write_avc_init_segment` feature-flag migration in
  `rtmp_ws_e2e`**. Deferred because the cleanest migration requires
  `lvqr-ingest` to normal-dep `lvqr-cmaf` (not the reverse, which
  is the current direction via the session-7 parity test dev-dep).
  Session 9 resolves the dep direction and flips the feature flag
  through the CI matrix.
* **Real `TestServer` HLS address**. Blocked on `lvqr-cli` growing
  an HLS axum bind in the serve path. Session 9 or later.

## Session 7 additions (2026-04-13)

## Session 7 additions (2026-04-13)

Session 7 closed three of the four follow-up items from the session-6
HANDOFF work list in a single run. Only the `lvqr-hls` scaffold is
deferred; every other session-7 priority landed.

1. **AVC init parity gate** (`crates/lvqr-cmaf/tests/parity_avc_init.rs`).
   New test that runs both writers on the same SPS / PPS / dimensions
   triple and structurally compares the decoded Moov trees via
   `mp4_atom::Moov::decode`. The assertion set is the playback
   contract: ftyp brands, mvhd timescale + next_track_id, trak count
   and track_id, mdhd timescale, hdlr type, stsd codec kind, Avc1
   width / height / depth, avcC length_size, avcC SPS and PPS byte
   sequences, mvex.trex track_id + default_sample_description_index.
   Every one of those fields matches across writers. The total byte
   length differs (cmaf=698, ingest=662, delta=+36 bytes) because the
   two writers pick different defaults for fields that do not affect
   playback (creation timestamps, default volume, matrix values,
   stsz/stsc/stco table shapes, stts entry counts, hdlr name
   strings). A second test (`avc_init_parity_byte_equality_is_not_required`)
   pins the intentional-non-equality invariant so a future session
   cannot accidentally replace the structural-match test with a
   byte-equality assertion. Dev-dep cycle check: `lvqr-ingest` is now
   a dev-dep of `lvqr-cmaf` (test-only); no normal-dep cycle because
   `lvqr-ingest` does not depend on `lvqr-cmaf` in any direction.
   This is the first byte-level proof the mp4-atom writer is a
   drop-in replacement for the hand-rolled path. When the Tier 2.3
   migration retires the hand-rolled writer, the parity test becomes
   the migration gate.

2. **Real multi-sub-layer HEVC fixture via kvazaar**
   (`crates/lvqr-conformance/fixtures/codec/hevc-sps-kvazaar-main-320x240-gop8.{bin,toml}`).
   `brew install kvazaar` (kvazaar 2.3.2) plus a `ffmpeg 8.1 -f lavfi
   -i testsrc2=320x240:rate=30 -t 1 -f yuv4mpegpipe | kvazaar --input
   - --input-res 320x240 --input-fps 30 --gop 8` pipeline produces an
   HEVC Annex-B bytestream whose SPS has
   `sps_max_sub_layers_minus1 = 1`. x265 refused to emit this under
   every session-5 configuration tried; kvazaar's `--gop 8` flag
   flips it from the low-delay-P default into a real
   temporal-scalability GOP, which is what the multi-sub-layer SPS
   path is for.

   The SPS NAL payload (no 2-byte header) is pinned in the codec
   fixture corpus with a sidecar carrying every decoded field
   (profile_idc = 1 Main, level_idc = 186 i.e. HEVC level 6.2,
   compat flags 0x60000000, chroma 4:2:0, 320x240). The existing
   `lvqr-codec::tests::conformance_codec.rs` harness picked it up
   with zero code changes the first time it ran, validating the
   session-5 "drop a .bin + .toml in and coverage extends
   automatically" design choice. **The multi-sub-layer HEVC parser
   path now has real-encoder coverage** on top of the synthetic
   bit-writer fixtures that were the session-5 canonical truth. The
   session-5 HANDOFF note "If no maintained encoder on homebrew
   emits multi-sublayer SPSes, document that in HANDOFF and leave
   the synthetic coverage as the canonical truth" is now obsolete.

3. **CmafSegmenter raw-sample coalescer design note**
   (`crates/lvqr-cmaf/src/segmenter.rs` crate doc comment). Extended
   the existing segmenter-module doc with a nine-section design note
   covering: `RawSample` input shape, per-track state
   (`TrackCoalescer`), boundary decision flow (Append / FlushPartial
   / FlushSegment), `moof + mdat` construction via `mp4-atom`,
   init-segment lifecycle with a new `CmafSegmenter::init_segment`
   public method, interaction with the existing pass-through path
   during the transition, and a concrete session-7-or-7.5
   deliverable list. Treat the note as a living spec that the first
   implementation PR is allowed to rewrite. Pinning it in the
   segmenter source rather than a separate markdown file means the
   note travels with the code it describes and does not rot in
   isolation.

4. **`lvqr-hls` crate scaffold**. First egress protocol to land on
   top of `lvqr-cmaf::CmafChunk`. Pure-library day-one scope:
   * `Manifest` + `Segment` + `Part` + `ServerControl` types
     modelling an RFC 8216 media playlist plus the LL-HLS draft
     extensions.
   * `PlaylistBuilder` pure state machine that consumes `CmafChunk`
     values, enforces strict DTS monotonicity and non-zero
     duration, and produces an updated `Manifest` on every push.
   * `Manifest::render` text renderer emitting `#EXTM3U`,
     `#EXT-X-VERSION:9`, `#EXT-X-TARGETDURATION`,
     `#EXT-X-SERVER-CONTROL` (with `CAN-BLOCK-RELOAD=YES`,
     `PART-HOLD-BACK`, `HOLD-BACK`), `#EXT-X-PART-INF`,
     `#EXT-X-MAP`, `#EXT-X-MEDIA-SEQUENCE`, per-segment `#EXTINF`,
     and per-part `#EXT-X-PART` with `INDEPENDENT=YES` on
     keyframes.
   * Day-one 2-of-5 contract slots: 5 unit tests, 2 integration
     tests (`tests/integration_builder.rs`), 3 proptest properties
     (`tests/proptest_manifest.rs` -- never-panic, well-formed
     output, strictly monotonic media sequences). Fuzz, E2E, and
     conformance slots are open and land with the axum router +
     Apple `mediastreamvalidator` wrapper in a later session.
   * No axum router yet. The router lands when a real HTTP
     consumer (browser, `hls.js`, `mediastreamvalidator`) arrives.
     Day-one scope is the manifest library only.
   * Explicitly out of scope for now: multivariant master
     playlists, byte-range delivery, encryption, discontinuity
     handling, rendition groups, byte-level `mediastreamvalidator`
     conformance.

### What session 7 did NOT land

Nothing from the session 7 work list carried over. Session 8 picks
up the remaining Tier 2.3 items (raw-sample coalescer implementation,
lvqr-hls axum router, retiring the hand-rolled fmp4 writer behind a
feature flag).

### Contract slot status as of session 7

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y (multi-sublayer now covered) |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-hls | y | open (no parser surface) | y | open (axum router pending) | open (mediastreamvalidator pending) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-hls` joins the table at 2-of-5 on day one. The multi-sub-layer
fixture strengthens the existing `lvqr-codec` conformance slot
without adding a new slot.

## Session 6 additions (2026-04-13): HEVC + AAC init segment writers

Session 6 tackled priority item 1 from the "Recommended Tier 2.3 entry
point" work list: grow `lvqr-cmaf` beyond AVC. The AVC-only
`write_avc_init_segment` from session 5 is now joined by HEVC and AAC
siblings, both built on `mp4-atom` and both covered by the same
ffprobe conformance harness.

1. **`write_hevc_init_segment` + `HevcInitParams`**. New public API in
   `crates/lvqr-cmaf/src/init.rs`. Takes VPS / SPS / PPS NAL unit byte
   blobs (each including the 2-byte HEVC NAL header so they can be
   written verbatim into the `hvcC` arrays) plus a decoded
   `lvqr_codec::hevc::HevcSps` view used to populate the `hvcC`
   header (profile, tier, level, chroma format) and the `tkhd` /
   `visual` dimensions. `general_constraint_indicator_flags` ships
   zeroed because the SPS parser does not surface them yet; that is
   fine for the 8-bit Main profile streams LVQR supports today but
   becomes a real gap the moment a Main10 or an HDR stream enters the
   picture. Comment at the call site flags the limitation.

2. **`write_aac_init_segment` + `AudioInitParams`**. Feeds raw
   `AudioSpecificConfig` bytes through `lvqr_codec::aac::parse_asc`
   and builds an `mp4a` sample entry plus an `esds` box using
   `mp4-atom`'s descriptor writer. mp4-atom's `DecoderSpecific` only
   supports the 4-bit `sampling_frequency_index` encoding and the
   compact (<32) AOT form, so the writer refuses:
   * sample rates that do not map to one of the 13 indexable
     frequencies in ISO/IEC 14496-3 Table 1.16
     (`InitSegmentError::UnsupportedAacSampleRate`),
   * AOT >= 32 / escape-encoded object types
     (`InitSegmentError::InvalidAsc` wrapping a `CodecError::MalformedAsc`
     with a descriptive message).

   Both errors are proptest-friendly and exercised by a new unit test
   using a hand-built explicit-frequency ASC. This is tighter than
   the existing hand-rolled `lvqr-ingest::remux::fmp4::esds` path,
   which silently produced malformed descriptors for ASCs longer than
   127 bytes pre-session-5.

3. **Real x265 HEVC NAL units captured**. Session 5 bootstrapped the
   fixture corpus with a real x265 SPS (post-NAL-header payload) but
   not VPS / PPS. Session 6 captured a full VPS + SPS + PPS triple
   from a `ffmpeg 8.1 -c:v libx265 -preset ultrafast` encode of a 1 s
   320x240 testsrc2 clip and pinned the bytes inline in the new
   lvqr-cmaf unit and conformance tests. The capture was a one-shot
   Python walker over the hvcC box; adding it to the corpus proper as
   a named fixture is deferred until the fixture loader grows a
   multi-NAL-per-fixture variant.

4. **ffprobe conformance expanded to HEVC and AAC**. New tests in
   `crates/lvqr-cmaf/tests/conformance_init.rs`:
   * `ffprobe_accepts_hevc_init_segment`: loads the
     `hevc-sps-x265-main-320x240` conformance-corpus fixture,
     constructs an `HevcSps` view from the sidecar metadata (so a
     drift between `parse_sps` output and the sidecar would fail
     `lvqr-codec`'s `conformance_codec.rs` harness first), builds an
     HEVC init segment using the captured x265 VPS / SPS NAL / PPS
     blobs, and feeds the result to ffprobe 8.1. **ffprobe accepts
     the output.** This is the first proof in the repo that the
     `mp4-atom`-backed HEVC writer produces bytes a real validator
     will take.
   * `ffprobe_accepts_aac_init_segment`: loads the
     `aac-asc-aaclc-44100hz-stereo` fixture, feeds the raw ASC
     through the new `write_aac_init_segment`, and asserts ffprobe
     accepts the resulting init segment. Same story for AAC.

5. **Unit-level round trips**. Three new lib-level tests cover the
   new writers without the conformance corpus dev-dep:
   * `hevc_init_segment_starts_with_ftyp_and_contains_moov`
   * `hevc_init_segment_round_trips_through_mp4_atom` (asserts the
     three `HvcCArray` entries match the input VPS / SPS / PPS
     byte-for-byte after mp4-atom decode)
   * `aac_init_segment_round_trips_through_mp4_atom` (asserts
     channel_count, sample_size, AOT, freq_index, chan_conf)
   * `aac_init_rejects_non_indexable_sample_rate` (explicit-frequency
     11468 Hz ASC must be refused with the typed error variant)

6. **Public API surface grew**. `lvqr_cmaf::{HevcInitParams,
   AudioInitParams, write_hevc_init_segment, write_aac_init_segment}`
   are now re-exported from the crate root. `InitSegmentError` gained
   two variants (`InvalidAsc`, `UnsupportedAacSampleRate`) so callers
   can distinguish a parse failure from a sample-rate-out-of-table
   rejection. Existing `VideoInitParams` / `write_avc_init_segment`
   signatures are unchanged; this is purely additive.

### What session 6 did NOT land (deferred to session 7)

The "Recommended Tier 2.3 entry point" work list named four follow-ups
after the HEVC / AAC writers. Only item 1 closed this session. The
remaining three are still open:

1. **`rtmp_ws_e2e` migration and AVC byte-diff** (priority 2 in the
   prior handoff). Wiring `lvqr-cmaf::write_avc_init_segment` into
   `rtmp_ws_e2e` alongside the hand-rolled
   `lvqr-ingest::remux::fmp4::video_init_segment` and diffing the
   byte outputs is the first real drop-in-replacement proof. Not
   landed this session; the HEVC / AAC writers were the higher
   leverage item because they unblock every future egress crate that
   needs non-AVC codec support.
2. **Multi-sub-layer HEVC fixture capture** (priority 3). Still not
   attempted; x265 will not produce `max_sub_layers_minus1 > 0` in
   any configuration tried so far, and kvazaar has not been
   installed. Synthetic-only coverage remains the canonical truth.
3. **CmafSegmenter raw-sample coalescer** (priority 4). Not started.
   The segmenter remains a pass-through that annotates pre-muxed
   fragments; the load-bearing raw-sample coalescer is a design-note
   item for the next session, not an implementation item.

### Contract slot status as of session 6

| Crate | proptest | fuzz | integration | E2E | conformance |
|---|---|---|---|---|---|
| lvqr-ingest | y | y | y | y | y |
| lvqr-codec | y | y | y | via rtmp_ws_e2e | y |
| lvqr-cmaf | y | open (no parser surface) | y | via rtmp_ws_e2e | y (AVC + HEVC + AAC) |
| lvqr-record | y | open | y | workspace e2e | y |
| lvqr-moq | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |
| lvqr-fragment | y | open | y | via rtmp_ws_e2e | n/a (pure value type) |

`lvqr-cmaf`'s conformance coverage is now AVC + HEVC + AAC against
ffprobe 8.1. Fuzz remains intentionally open for the same reason as
the prior sessions: the crate consumes `Bytes` from trusted producers
and writes mp4-atom structures, so there is no parser attack surface.

## Session 5 part 2 additions (2026-04-13)

Directly after the part-1 commit landed and pushed, two follow-ups
from the "Recommended Tier 2.3 entry point" work list in this file
closed in the same session:

1. **`lvqr-conformance` fixture corpus bootstrapped**. The session-3
   Tier 1 item that had been BLOCKED since session 3 on "no ffmpeg
   in the dev env" unblocked as soon as ffmpeg 8.1 was installed.
   Captured:
   - `fixtures/codec/hevc-sps-x265-main-320x240.{bin,toml}` -- the
     real x265 SPS already pinned in the parser's unit test, now
     sitting in the corpus with a sidecar naming every expected
     decoded field including `general_level_idc = 60` (HEVC level
     2.0, which x265 picks for 320x240 at 30 fps).
   - `fixtures/codec/aac-asc-aaclc-{44100,48}khz-stereo.{bin,toml}`
     -- the two canonical AAC-LC ASC byte blobs LVQR already relies
     on elsewhere, pinned with their decoded values.
   - `fixtures/fmp4/cmaf-h264-baseline-360p-1s.{mp4,toml}` -- a 1 s
     fragmented CMAF H.264 Baseline 3.1 capture from ffmpeg, seed
     for future lvqr-ingest and lvqr-cmaf consumer tests.
   - `fixtures/rtmp/h264_aac_1s.{flv,toml}` -- a 1 s H.264 + AAC-LC
     FLV, first real RTMP test vector in the repo.

   `lvqr-conformance::codec::{list, load, CodecFixture,
   CodecFixtureMeta, HevcSpsExpected, AacAscExpected}` exposes a
   typed loader: consumers call `list()` to iterate every fixture
   on disk, and each fixture comes with parsed sidecar metadata so
   adding a new byte blob + TOML pair automatically extends
   coverage without touching test code. Sidecar parsing runs
   through `toml` + `serde`, already in the workspace dep set.

2. **Conformance slot closed for `lvqr-codec`**. New
   `crates/lvqr-codec/tests/conformance_codec.rs` iterates the
   codec corpus via `lvqr_conformance::codec::list()` and asserts
   `parse_sps` / `parse_asc` decode every blob to the expected
   values from the sidecar. **The contract mechanism paid for
   itself on its first run**: my initial hand-computed sidecar
   guessed `general_level_idc = 93` for the 320x240 x265 SPS
   (copying the value from the synthetic `codec_string_format` unit
   test), and the conformance test failed loudly on the first run
   because the real encoder output is level 60. The fixture sidecar
   and the hand-rolled x265 unit test are now both pinned to the
   real value. This is exactly the "catches silent drift between
   hand-written synthetic tests and real encoder output" story the
   5-artifact contract exists for.

   `lvqr-codec` is now the second crate (after `lvqr-ingest`) to
   hit **5/5 contract slots**. The only remaining open slots
   workspace-wide are the fuzz slots on `lvqr-record`, `lvqr-moq`,
   `lvqr-fragment`, `lvqr-cmaf` (all low-marginal-value per prior
   session decisions) and the conformance slots on `lvqr-moq` and
   `lvqr-fragment` (pure value types with no external validator
   target).

## Session 5 additions (2026-04-13): Tier 2.2 closure + Tier 2.3 scaffold

Five work items landed in a single session, closing Tier 2.2 and
opening Tier 2.3 on top of the `mp4-atom` box writer.

1. **HEVC SPS parser now handles multi-sub-layer streams**. Replaced
   the session-4 `Unsupported` bail at `sps_max_sub_layers_minus1 > 0`
   with a real `parse_ptl_sublayers` helper that walks the sub-layer
   profile/level present flag loop (2 bits per sub-layer), the
   reserved-zero-2-bits padding for layers in `max_sub_layers_minus1..8`,
   and the per-sub-layer 88-bit PTL body plus optional 8-bit level_idc.
   LVQR does not surface per-sub-layer data; the bits are consumed so
   the reader ends up at the right position for the SPS fields that
   follow. Three positive decode tests land alongside: synthetic
   single-sub-layer, synthetic two-sub-layer, and synthetic
   max-sub-layer (`max_sub_layers_minus1 = 6`), all built via a tiny
   test-only bit writer.

   Plus a **real encoder fixture**: an SPS captured from
   `ffmpeg -c:v libx265` encoding a 320x240 testsrc2 clip, pinned in
   `parse_sps_decodes_real_x265_single_sublayer`. This is the first
   time the parser is pinned against an independent encoder's bit
   layout rather than the LVQR test writer. Multi-sub-layer *real*
   fixtures are deferred: neither x265's `--temporal-layers` nor
   b-pyramid modes produced a `max_sub_layers_minus1 > 0` SPS in any
   configuration tried, so the multi-sub-layer path is currently
   synthetic-only. Not ideal; honest.

2. **`lvqr-ingest::remux::fmp4::esds` migrated to
   `lvqr_codec::aac::parse_asc`**. Closes the internal audit finding
   "fMP4 esds descriptor uses single-byte length encoding". The
   hand-rolled `parse_audio_specific_config` in `flv.rs` now
   delegates to the hardened parser, so every FLV AAC sequence
   header benefits from the 5-bit + 6-bit object-type escape, the
   15-index explicit-frequency escape, and HE-AAC SBR/PS signalling
   that the v0.3 writer silently truncated. The descriptor length
   encoding in the `esds` box is now a new `write_mpeg4_descriptor`
   helper that always emits the 4-byte MPEG-4 variable-length form
   (tag byte + 4 length bytes, MSB continuation), replacing the
   previous single-byte prefix that would malform on any
   DecoderSpecificInfo larger than 127 bytes. The hardened path is
   exercised by a new conformance test
   `ffprobe_accepts_audio_init_and_frame` in `golden_fmp4.rs` which
   feeds the AAC init segment plus a one-frame media segment to
   ffprobe 8.1, and by a new unit test
   `mpeg4_descriptor_length_encoding_round_trips_large_payloads` that
   writes a 200-byte payload through `write_mpeg4_descriptor` and
   asserts every byte of the emitted length field.

3. **`lvqr-cmaf` crate scaffolded, built on `mp4-atom` 0.10.1**. New
   workspace member opening Tier 2.3. Four modules:

   * `chunk.rs`: `CmafChunk` (wire-ready `moof+mdat` bytes, DTS,
     duration, track id) plus `CmafChunkKind`
     (`Partial` / `PartialIndependent` / `Segment`) so egress crates
     get HLS/DASH/MoQ boundary classification in one enum.
   * `policy.rs`: `CmafPolicy` tuning (partial + segment durations)
     and `CmafPolicyState`, a pure state machine that classifies
     each fragment by keyframe flag + DTS. Defaults land for 90-kHz
     video (200 ms partial, 2 s segment) and 48-kHz audio. Pure, no
     I/O, no async, trivially proptest-able.
   * `init.rs`: working `write_avc_init_segment` using `mp4-atom`'s
     `Ftyp`, `Moov`, `Mvhd`, `Trak`, `Tkhd`, `Mdia`, `Mdhd`, `Hdlr`,
     `Minf`, `Vmhd`, `Dinf`, `Dref`, `Stbl`, `Stsd`, `Codec::Avc1`,
     `Avcc`, `Visual`, `Mvex`, `Trex`. Encodes directly into a
     `BytesMut` via the crate's `bytes` feature. Round-trips through
     `mp4-atom` decode and is accepted by ffprobe 8.1.
   * `segmenter.rs`: `CmafSegmenter<S: FragmentStream>` with pull-
     based `next_chunk()`. Thin today because every `Fragment` from
     the RTMP bridge is already a pre-muxed `moof+mdat`; the
     segmenter annotates with boundary info and passes through. The
     real sample-coalescer grows additively when ingest begins
     emitting raw samples instead of pre-muxed fragments.

   4-of-5 contract slots on day one: proptest (`tests/proptest_policy.rs`,
   4 properties x 200 cases), integration (`tests/integration_segmenter.rs`,
   3 scenarios driving a scripted `FragmentStream`), conformance
   (`tests/conformance_init.rs`, ffprobe accepting the mp4-atom init
   segment), e2e via the workspace `rtmp_ws_e2e` path. Fuzz slot
   intentionally open: the segmenter has no parser attack surface.

4. **cargo-fuzz targets for `lvqr-codec`**. New `crates/lvqr-codec/fuzz/`
   with three targets: `parse_hevc_sps`, `parse_aac_asc`, and
   `read_ue_v` (which uses the input's first byte as a bit offset so
   the exp-Golomb decoder is fuzzed across every starting alignment,
   bounded to 64 iterations per input so libfuzzer terminates).
   Excluded from the workspace members list because `libfuzzer-sys`
   needs nightly. `.github/workflows/fuzz.yml` migrated from a single
   `target` matrix axis to an `include`-style matrix carrying
   `(target, fuzz_dir)` pairs so the ingest and codec fuzz crates
   share one job definition. Closes the fuzz slot for `lvqr-codec`.

5. **Conformance slot for `lvqr-record`**. New
   `tests/record_conformance.rs` builds a real AVC init segment via
   `lvqr_cmaf::write_avc_init_segment`, drives it through a MoQ
   origin + broadcast + track + group publisher, records it with
   `BroadcastRecorder::record_broadcast`, reads the init file back
   from disk, runs it through `ffprobe_bytes`, and asserts
   byte-for-byte equality with the bytes fed to the publisher. This
   is the first test in the repo that exercises `lvqr-cmaf` from a
   different crate, and the first that chains mp4-atom -> MoQ ->
   recorder -> disk -> ffprobe end-to-end. Closes the last open
   contract slot on `lvqr-record` (fuzz stays open per the session-3
   decision that pure helpers are already proptest-covered and fuzz
   is low-marginal-value).

### Library research decision (session 5)

Before writing any new codec parser code this session, verified that
the Rust ecosystem still has no maintained, pure-Rust, MIT/Apache
alternative for the narrow "codec string + sample-entry fields"
niche that `lvqr-codec` owns:

* No `h265-reader` / `h26x-reader` crates exist.
* `hevc-parser` (quietvoid) is a Dolby-Vision-focused tool,
  self-described "incomplete", pulls `nom 8` + `bitvec_helpers` +
  `matroska-demuxer` + `regex-lite`. Not a drop-in.
* Mozilla `mp4parse` is MPL-2.0, read-only, last release May 2023.
* `symphonia`'s AAC ASC parser is private behind MPL-2.0 and not
  exposed as a standalone API.
* `bitstream-io` (Matt Brubeck) is actively maintained but does not
  ship exp-Golomb, so replacing LVQR's ~250-line BitReader would
  save <200 lines and still require Golomb on top.
* `mp4-atom` 0.10.1 (kixelated, MIT/Apache, pure Rust, actively
  maintained) is the right call for `lvqr-cmaf` and already wired
  in.

Decision: keep `lvqr-codec` hand-rolled, build `lvqr-cmaf` on
`mp4-atom`. Revisit when a maintained pure-Rust HEVC/ASC parser
appears or symphonia factors its ASC code out.

## Session 4 part 2 additions (2026-04-13): Tier 2.2 `lvqr-codec` scaffold

The first Tier 2.2 deliverable landed directly after Tier 2.1 was
committed and pushed: a `lvqr-codec` crate with a shared MSB-first
forward bit reader (including H.26x exp-Golomb decoders and
EBSP->RBSP emulation-prevention byte stripping), an HEVC NAL unit
type classifier + minimal SPS parser (profile / tier / level /
chroma-format / resolution, enough to build an `hev1` sample entry
and emit a codec string), and a hardened AAC `AudioSpecificConfig`
parser that correctly handles the 5-bit + 6-bit escape encoding for
object types in the 32..=63 range, the 15-index explicit-frequency
escape, and HE-AAC (SBR) / HE-AAC v2 (PS) signalling.

4-of-5 artifact coverage on day one: proptest never-panic harnesses
for HEVC and AAC, an integration test that wires the parsers to
expected codec-string outputs, 19 unit tests covering the bit
reader + both codec modules. Fuzz is deferred because cargo-fuzz
harnesses want their own nightly-only crate, and conformance is
deferred until real encoder fixtures are captured and checked in.

The HEVC SPS parser intentionally only supports
`sps_max_sub_layers_minus1 == 0` (every consumer HEVC stream LVQR
has encountered in practice). Multi-sublayer streams return
`CodecError::Unsupported` so callers know to plug in a more complete
parser. Full scaling-list / VUI / HRD parsing is explicitly out of
scope: LVQR does not decode HEVC, it only needs enough metadata to
build an fMP4 init segment.

The AAC parser is ready to replace the 2-byte ASC assumption baked
into `lvqr-ingest::remux::fmp4::esds`. That migration will land
alongside the HEVC RTMP support in a follow-up commit.

## What a new session must read first

1. `CLAUDE.md` (project rules, hard hard rules)
2. `tracking/ROADMAP.md` (authoritative 18-24 month plan, 10 load-bearing decisions)
3. `tracking/AUDIT-2026-04-13.md` (competitive audit, 5 strategic bets, what NOT to ship)
4. `tracking/AUDIT-INTERNAL-2026-04-13.md` (dead-code, bug, hardening inventory + Fix Plan)
5. `tracking/AUDIT-READINESS-2026-04-13.md` (CI + supply chain + doc drift + Tier 1 progress)
6. `tracking/HANDOFF.md` (this file)
7. `tests/CONTRACT.md` (5-artifact test contract)

The single most important architectural decision in the entire roadmap
is the Unified Fragment Model (`lvqr-fragment`) plus the `lvqr-moq`
facade crate, Tier 2.1. As of session 4 both have landed, the RTMP
bridge has migrated to produce Fragments through `MoqTrackSink`, and
the dead code in `lvqr-core` (Registry, RingBuffer, GopCache, Gop)
has been deleted in the same commit. Tier 2.2 (lvqr-codec, HEVC
scaffold) is the next target.

## Session 4 (2026-04-13) additions -- Tier 2.1 landing

Seven bullets. All of Tier 2.1 as scoped in the roadmap plus one
follow-up fix for a Tier 1 latent issue that surfaced under ffprobe
8.1.

1. **`crates/lvqr-moq/` facade crate**. Re-exports the moq-lite types
   every LVQR crate uses (`Track`, `Origin`, `OriginProducer`,
   `BroadcastProducer`, `BroadcastConsumer`, `TrackProducer`,
   `TrackConsumer`, `GroupProducer`, `GroupConsumer`) under one module
   so upstream churn has a single point of impact. `MOQ_LITE_VERSION`
   const pins the version the facade was built against. The lib.rs
   doc is explicit that this is a re-export layer today and that
   newtypes will be introduced at the facade when downstream crates
   need behavioral hooks -- honest scoping instead of 500 lines of
   mechanical wrappers with no current value.

2. **`crates/lvqr-fragment/` Unified Fragment Model**. Core types
   (`Fragment { track_id, group_id, object_id, priority, dts, pts,
   duration, flags: FragmentFlags, payload: Bytes }`, `FragmentFlags`
   with `KEYFRAME` / `AUDIO` / `DELTA` / `DELTA_DISCARDABLE` presets,
   `FragmentMeta` with lazy `set_init_segment` for the late-binding
   RTMP sequence-header case) plus the `FragmentStream` trait (an
   async `next_fragment() -> Option<Fragment>` + a `meta()` accessor,
   intentionally without `async_trait` since the future is always
   borrowed from `self`).

3. **`MoqTrackSink` adapter** inside `lvqr-fragment`. The first
   concrete projection from Fragment into a wire format: holds a
   `TrackProducer` plus an optional current `GroupProducer`, opens a
   new MoQ group on every keyframe push (closing the prior group
   first), prepends `FragmentMeta::init_segment` as frame 0 of every
   new group so late-joining subscribers can always decode, writes
   delta fragments into the current group, and silently drops deltas
   that arrive before any keyframe. `Drop` finishes the current
   group. This is the load-bearing shape change: every future ingest
   crate produces Fragments, calls `sink.push(..)`, and never touches
   MoQ directly.

4. **Facade migration across every downstream crate**. `lvqr-relay`,
   `lvqr-ingest`, `lvqr-record`, `lvqr-cli`, plus their tests, now
   import MoQ types from `lvqr_moq::` rather than `moq_lite::`.
   `lvqr-record` dropped its direct `moq-lite` dep entirely. `lvqr-relay`
   and `lvqr-cli` kept their direct `moq-lite` deps because they still
   interoperate with `moq-native` at the transport layer, but every
   *type reference* in those crates now goes through the facade.

5. **`RtmpMoqBridge` migrated to produce Fragments**. The video and
   audio RTMP callbacks no longer manipulate MoQ `GroupProducer`s
   directly. Instead each stream holds a `MoqTrackSink` for video and
   another for audio; the callbacks build a `Fragment` (with the
   appropriate `FragmentFlags::KEYFRAME` or `FragmentFlags::DELTA`)
   and call `sink.push(&frag)`. FLV sequence headers call
   `sink.set_init_segment(init)`. The audio path finishes its group
   after every frame so every AAC frame is its own independently-
   decodable MoQ group (the existing behavior, preserved). Every
   existing `rtmp_bridge_integration` and `rtmp_ws_e2e` test passes
   unchanged, which is the real proof the migration is behavior-
   preserving.

6. **Dead code deletion in `lvqr-core`**. Per the internal audit
   recommendation at `tracking/AUDIT-INTERNAL-2026-04-13.md`, deleted
   `Registry`, `RingBuffer`, `GopCache`, and the `Gop` struct in the
   same commit that lands their replacement. Removed both benches
   (`fanout.rs` and `ringbuffer.rs`), their `criterion` dev-dep, and
   the `TestPublisher` + `synthetic_gop` helpers in `lvqr-test-utils`
   that only existed to exercise `Registry`. `Frame`, `TrackName`,
   `StreamId`, `SubscriberId`, `RelayStats`, `EventBus`, and
   `RelayEvent` survive as shared value types. `lvqr-core` is now
   roughly 40% smaller and every remaining type has at least one
   production consumer.

7. **5-artifact contract closed for the new crates (4 of 5 slots)**.
   `lvqr-moq` and `lvqr-fragment` both ship proptest, integration,
   and e2e coverage on day one; conformance and fuzz slots are still
   open by design (both require additional infrastructure and belong
   to their own follow-up work). `scripts/check_test_contract.sh`
   was updated to include the two new crates in its in-scope list,
   the contract runs green in educational mode, and the only
   remaining warnings are the four still-open fuzz/conformance slots
   across `lvqr-record`, `lvqr-moq`, and `lvqr-fragment`.

### Bonus fix: ffprobe 8.1 false negative in the golden fMP4 conformance slot

`ffprobe_bytes` in `lvqr-test-utils` treated any non-empty stderr on
an exit-zero ffprobe run as a failure. ffprobe 8.1 (the current
Homebrew version) emits decoder-level warnings
(`deblocking_filter_idc 32 out of range`, `no frame!`) on the
synthetic H.264 NAL payloads the golden tests feed it, even though
the container parses cleanly. Under older ffprobe builds those
warnings were silent and the test passed; under 8.1 they broke CI
the moment ffmpeg got installed locally. Fix: trust the exit code
as the authoritative verdict (non-zero = rejected, zero = accepted)
and surface stderr on exit-zero runs via `eprintln!` as diagnostics
rather than failing on them. This closes the last pre-existing test
failure that was latent before session 4 and unrelated to Tier 2.1.

## Session 3 (2026-04-13) additions

Seven Tier 1 items landed, one bonus security fix caught by a new
proptest, one bonus integration harness closing an audit gap. The
single Tier 1 item still blocked is the conformance fixture corpus
bootstrap, which requires `ffmpeg` in the dev environment.

1. **`lvqr_cli::start` library target** (`crates/lvqr-cli/src/lib.rs`).
   Extracted the full server wiring from `main.rs` into a public lib:
   `ServeConfig`, `ServerHandle`, `async fn start(config) -> Result<ServerHandle>`.
   All listeners bind before `start` returns so callers that pass
   `port: 0` get real addresses back off the handle. `main.rs` shrinks
   to ~150 lines (parse args, build auth, call `start`, wait on
   ctrl-c, `handle.shutdown().await`). `RtmpServer::run_with_listener`
   added in `lvqr-ingest` so the pre-bind pattern works without a
   find-available-port race.

2. **`lvqr_test_utils::TestServer`** (`crates/lvqr-test-utils/src/test_server.rs`).
   Thin wrapper over `lvqr_cli::start` that binds on `127.0.0.1:0`,
   disables Prometheus (process-wide, panics on second install), and
   returns a handle with `rtmp_url()`, `ws_url()`, `ws_ingest_url()`,
   `http_base()`, `relay_addr()`, etc. Config builder supports
   `with_mesh(max_peers)`, `with_auth(SharedAuth)`, `with_record_dir`.
   Dev-dep cycle `lvqr-cli -> lvqr-ingest -> [dev] lvqr-test-utils -> lvqr-cli`
   is allowed by cargo and works correctly.
   Smoke tests at `crates/lvqr-test-utils/tests/test_server_smoke.rs`
   prove every listener binds and every URL helper formats against the
   bound address.

3. **`lvqr-signal` input validation** (`crates/lvqr-signal/src/signaling.rs`).
   Closes the internal-audit finding. New `is_valid_peer_id` (enforces
   `[A-Za-z0-9_-]{1,64}`) and `is_valid_track` (wider alphabet plus
   explicit rejection of `..`, `//`, leading/trailing slash,
   backslashes). New `SignalMessage::Error { code, reason }` variant.
   `wait_for_register` sends a structured error frame on every reject
   path (`invalid_json`, `invalid_peer_id`, `invalid_track`,
   `expected_register`) and closes the session. The main loop rejects
   a second Register on an already-registered connection with
   `duplicate_register`, enforcing the audit's "cap registrations per
   connection at 1" explicitly. Peer-id log fields on reject paths
   record only `len`, never the attacker-controlled bytes.
   Integration tests at `crates/lvqr-signal/tests/signal_integration.rs`
   drive the validators through the real `/signal` endpoint on a
   `TestServer::with_mesh(3)` instance using `tokio-tungstenite`.
   Five tests: malformed peer_id, traversal track, non-Register first
   message, duplicate Register, happy path (receives AssignParent).

4. **Proptest extensions for `lvqr-ingest`**
   (`crates/lvqr-ingest/tests/proptest_parsers.rs`). Four new
   properties, roughly 4100 generated cases per run (up from 2560):
   `extract_resolution_never_panics`,
   `extract_resolution_never_panics_on_sps_prefix`,
   `generate_catalog_always_parses_as_json` (parses output with
   `serde_json::from_str`, asserts track count and required fields),
   `generate_catalog_places_video_before_audio` (ordering invariant
   the browser MSE player depends on). Added `serde_json` as dev-dep.

5. **Proptest for `lvqr-record` pure helpers**
   (`crates/lvqr-record/tests/proptest_recorder.rs`). Five
   properties targeting the internal helpers exposed via a new
   `#[doc(hidden)] pub mod internals` re-export. **Proptest caught a
   real path-traversal bypass in `sanitize_name`**: input `".\0."`
   sanitized to `".."` because the old ordering stripped control
   chars *after* the `..` replacement pass, so deleting `\0`
   regenerated a traversal sequence. Fixed by stripping controls
   first, then replacing `/`, `\`, and `..`. Regression seed pinned
   in `tests/proptest_recorder.proptest-regressions`.

6. **Nightly cargo-fuzz CI** (`.github/workflows/fuzz.yml`). 60s per
   target on PR (path-filtered so unrelated PRs don't compile the
   fuzz harness), 15 min per target on daily 07:00 UTC cron, manual
   dispatch supported. Matrix over `parse_video_tag` and
   `parse_audio_tag`. `continue-on-error: true` during Tier 1.
   Crash artifacts and corpora upload unconditionally with 30-day
   retention.

7. **cargo-audit CI job** (`.github/workflows/ci.yml`).
   `continue-on-error: true`, separate `audit-v1` cache key. Step
   failures surface honestly in the Checks tab without blocking
   PRs. Promote to required once the baseline is clean.

8. **5-artifact contract enforcement** (`scripts/check_test_contract.sh`
   plus `.github/workflows/contract.yml`). Portable bash script
   (no `globstar` / bash 4+ features; runs on macOS bash 3.2). Walks
   the in-scope crate list, checks each of the five slots, emits
   GitHub Actions warning annotations on missing slots. Soft-fail
   during Tier 1; flipped to strict via `LVQR_CONTRACT_STRICT=1` in
   Tier 2. Per-crate E2E exemption via
   `CONTRACT_E2E_EXEMPT_<crate_with_underscores>=1`. Current state:
   `lvqr-ingest` satisfies all 5 slots; `lvqr-record` satisfies 3/5
   (missing fuzz and conformance slots).

9. **Playwright E2E scaffold** (`tests/e2e/`,
   `.github/workflows/e2e.yml`). Shell-level specs over the test-app
   rendered through `python3 -m http.server`. Three specs covering
   the three-tab navigation, the Watch-tab video element and
   broadcast input, and the Stream-tab form reachability. Tier 1
   scope: no live LVQR binary. Tier 2 extends the
   `playwright.config.ts` webServer array with a `cargo run` entry
   and specs assert on buffered media.

10. **Admin HTTP + JWT integration tests**
    (`crates/lvqr-cli/tests/auth_integration.rs`). Six tests driving
    `TestServer` with three auth providers (Noop, StaticAuthProvider,
    JwtAuthProvider) over a hand-rolled HTTP/1.1 client on raw
    `tokio::net::TcpStream`. Closes the
    `tracking/AUDIT-READINESS-2026-04-13.md` gap: "JWT provider is
    wired into the CLI but has no integration test ... no test
    verifies that `lvqr-cli serve --jwt-secret foo` actually
    validates a real JWT end-to-end". Covers: open access happy
    path, static token missing/wrong/correct, JWT good token, JWT
    wrong secret, JWT insufficient scope, JWT expired. Mints tokens
    via `jsonwebtoken::encode` using `lvqr_auth::JwtClaims` directly
    so the test cannot drift from the production claim schema. First
    integration-level coverage of the admin HTTP layer at all.

## Bonus security fix: `sanitize_name` path-traversal bypass

The `lvqr-record` proptest for `sanitize_name` (added in session 3 as
part of item #5 above) failed on its first run with minimal repro
`".\0."`. The old ordering stripped control characters *after* the
`..` replacement pass, so deleting `\0` regenerated the traversal
sequence `..` from `.\0.`. An attacker-supplied broadcast name like
`"..\0.."` would sanitize to `"...."`, and `"..\0..\0etc\0passwd"`
would sanitize to `"....etc..passwd"` — both still containing `..`.

**Fix**: reorder so control-char stripping runs first, then `/`, `\`,
and `..` replacement. The prior ordering's unit test
(`sanitize_strips_path_traversal` in `recorder.rs`) was not wrong,
just incomplete: it only exercised a literal `"../etc/passwd"` which
the old code did catch. The proptest found the class of input the
unit test missed in under a second. Minimal repro pinned in
`crates/lvqr-record/tests/proptest_recorder.proptest-regressions`
for replay on every future run.

This is the clearest Tier 1 validation that the 5-artifact contract
pays for itself: adding one proptest to a crate that already had a
passing unit test suite surfaced a real security bypass that had
been latent across multiple releases.

## Tier 1 work list status (end of session 3)

| Item | Status |
|---|---|
| 1. TestServer in `lvqr-test-utils` | DONE |
| 2. `lvqr-signal` validators + integration test | DONE |
| 3. Proptest for `extract_resolution` and catalog JSON | DONE |
| 4. Nightly cargo-fuzz CI | DONE |
| 5. `cargo audit` in CI | DONE (soft-fail) |
| 6. `lvqr-conformance` fixture corpus bootstrap | BLOCKED (ffmpeg missing locally) |
| 7. 5-artifact CI enforcement script | DONE (educational mode) |
| 8. Playwright `tests/e2e/` scaffolding | DONE (shell-only) |
| bonus: `lvqr-record` proptest + `sanitize_name` fix | DONE |
| bonus: JWT + static admin auth integration tests | DONE |
| bonus: first integration coverage of admin HTTP layer | DONE |

The load-bearing Tier 2 architectural call
(`lvqr-fragment` + `lvqr-moq` facade, roadmap decisions 1 and 2)
remains explicitly the next target now that Tier 1 is substantially
closed. Item 6 is the only remaining Tier 1 blocker and needs an
ffmpeg-equipped host for one session to capture fixture bytes.

## Known debt and honest limitations after session 3

These are not bugs; they are tracked follow-ups a future session
should be aware of so nothing is discovered twice.

- **`start()` fire-and-forget tasks**: the optional recorder task
  and the mesh reaper task are spawned outside the outer
  `tokio::join!` in `lvqr_cli::start`. Both respect the shared
  shutdown token and exit cleanly, but `ServerHandle::shutdown().await`
  does not block on them. In practice fine (they are short-lived
  after cancellation), but tests that inspect recorder output after
  shutdown must drive the recorder directly rather than through
  `TestServer`. See `crates/lvqr-record/tests/record_integration.rs`
  for the direct-drive pattern.
- **`lvqr-record` contract slots**: after session 3, lvqr-record
  satisfies proptest, integration, and (via the workspace E2E)
  the e2e slot of the 5-artifact contract. The fuzz and conformance
  slots are still open. Fuzz is low-marginal-value (the helpers are
  already proptest-covered); conformance requires ffprobe against
  recorded segments and is a natural follow-up once a session has
  ffmpeg available.
- **`scripts/check_test_contract.sh` cross-crate E2E attribution**:
  the script accepts workspace-level `tests/e2e/**/*.spec.ts` as
  satisfying the e2e slot for any in-scope crate. This is over-
  permissive during Tier 1 and should be tightened in Tier 2 via
  the `CONTRACT_E2E_EXEMPT_<crate>` knob plus a per-crate e2e
  convention (e.g. `tests/e2e/<crate-name>/*.spec.ts`).
- **`docs/architecture.md` and `docs/quickstart.md` are stale**
  per `tracking/AUDIT-READINESS-2026-04-13.md`. Architecture still
  says `tokio::select!` for the CLI server composition; the Tier 0
  fix was `tokio::join!`. Quickstart references a `/watch/*` admin
  endpoint that does not exist. `CONTRIBUTING.md` crate list is
  missing `lvqr-auth`, `lvqr-record`, `lvqr-conformance`. None of
  this affects CI; it is a dedicated docs pass for Tier 5.
- **`lvqr-cli` stale deps**: `rcgen`, `rustls`, `serde`,
  `serde_json`, `futures`, and `toml` are declared in
  `crates/lvqr-cli/Cargo.toml` as normal deps but the new
  `lib.rs` + `main.rs` don't use them directly (they were
  dependencies of the old 930-line `main.rs`). Harmless but
  worth a cleanup pass once the Tier 2 rewrite of the CLI
  composition root settles.
- **Admin-level hardening (Tier 3)**: `/metrics` is intentionally
  unauthenticated for Prometheus scraping; `CorsLayer::permissive()`
  is applied workspace-wide; admin auth middleware does not emit
  `lvqr_auth_failures_total{entry="admin"}`; no rate limiting
  anywhere. All four are already tracked in
  `tracking/AUDIT-INTERNAL-2026-04-13.md` as Tier 3 work.
- **Dead code in lvqr-core: DELETED in session 4** alongside the
  Tier 2.1 landing. `Registry`, `RingBuffer`, `GopCache`, and the
  `Gop` struct are gone. The remaining surface is `Frame`,
  `TrackName`, `StreamId`, `SubscriberId`, `RelayStats`, `EventBus`,
  `RelayEvent`. `StreamId`/`SubscriberId` are still dead (no
  external consumers) but were deliberately kept to avoid scope
  creep in this commit; they should be deleted in a later cleanup
  pass if they remain unused.
- **`lvqr-wasm`**: entire crate is self-deprecated. Scheduled for
  removal in v0.5. CI still builds it.
- **Still-open 5-artifact slots after session 5 (educational mode,
  not blocking)**: fuzz for `lvqr-record`, `lvqr-moq`,
  `lvqr-fragment`, `lvqr-cmaf`; conformance for `lvqr-moq`,
  `lvqr-fragment`, `lvqr-codec`. `lvqr-ingest` is 5/5; `lvqr-record`,
  `lvqr-codec`, and `lvqr-cmaf` are 4/5. Fuzz is low-marginal-value
  for the facade + fragment + cmaf types (they are pure value
  types or stateful shims with no parser attack surface).
  Conformance for `lvqr-codec` is the single most obvious next
  slot to close: pin a handful of real encoder-captured HEVC SPS
  and AAC ASC byte blobs plus their expected decoded values,
  reusing the x265 fixture already in
  `parse_sps_decodes_real_x265_single_sublayer` as the seed.

## Recommended Tier 2.3 entry point (session 12)

Session 11 closed every cross-crate item from the prior list (dep
cycle, CLI HLS composition, `cmaf-writer` feature flag, WHEP
scoping). Session 12 inherits the follow-ups that depend on either
the new HLS pipeline being on `main` for a release cycle, or the
WHEP scoping doc being ready to absorb implementation work.

1. **Multi-broadcast HLS routing.** The `HlsFragmentBridge` shipped
   in session 11 is intentionally single-rendition: only the first
   broadcast that publishes a video track feeds the HLS server.
   Production-grade routing requires either (a) a per-broadcast
   `HlsServer` instance keyed by broadcast name with the axum
   router demultiplexing under a `/hls/{broadcast}/...` prefix, or
   (b) a multi-tenant `HlsServer` that grows broadcast-aware
   routing internally. Option (a) keeps `lvqr-hls` simple and
   matches the LL-HLS single-rendition mental model; option (b)
   touches the manifest generator. Pick (a) unless option (b)
   surfaces a clean reuse path during implementation. Update
   `crates/lvqr-cli/tests/rtmp_hls_e2e.rs` to publish two
   broadcasts and assert both playlists return distinct content.

2. **Audio rendition group in HLS.** `FragmentObserver::on_fragment`
   already fires for `1.mp4`. `HlsFragmentBridge` ignores it
   today. The work is: extend `HlsFragmentBridge` to track a
   second `CmafPolicyState` keyed on the audio track id; mount a
   sibling `HlsServer` (or sibling per-track tracks inside one
   server, depending on whether `lvqr-hls` learns rendition
   groups) at `/hls/audio/playlist.m3u8`; update the integration
   test to verify the audio playlist is fetchable and that an
   `EXT-X-MEDIA:TYPE=AUDIO` master playlist points at it. The
   scope is bigger than item 1 because it forces `lvqr-hls` to
   learn `EXT-X-STREAM-INF` master-playlist generation. Plan
   carefully before starting.

3. **Begin `lvqr-whep` implementation.** The session 11 design doc
   at `crates/lvqr-whep/docs/design.md` lays out a 5-artifact plan
   with concrete test file paths plus four open questions. Start
   by answering the four open questions in a 5-bullet design
   reply, then create `crates/lvqr-whep/Cargo.toml`, register the
   crate as a workspace member, and land item 1 of the 5-artifact
   plan (proptest on the H.264 RTP packetizer) before any
   networking code. **Prerequisite**: a `RawSampleObserver` hook on
   `RtmpMoqBridge` so WHEP can subscribe to per-sample data
   without re-parsing CmafChunks. The cleanest add is a sibling
   trait method (or a new trait altogether) following the same
   pattern as the session-11 `FragmentObserver`. Pick this only
   if items 1 and 2 above are deferred or already in progress;
   running all three in one session is too much surface area.

4. **Flip `cmaf-writer` to default-on.** Once the
   `test-cmaf-writer` matrix job has been green on `main` for at
   least one release cycle (track in `tracking/HANDOFF.md` cycle
   notes), flip `default = ["rtmp", "cmaf-writer"]` in
   `crates/lvqr-ingest/Cargo.toml`. Keep the hand-rolled
   `video_segment` writer in place under a `legacy-fmp4` feature
   for one more cycle, then delete in a later session. The parity
   gate at `crates/lvqr-ingest/tests/parity_avc_segment.rs`
   becomes unnecessary at deletion time and should be removed in
   the same commit.

Session 12 should pick **at most two** of the four items above and
land them cleanly. Items 1 and 4 are the safest pair to bundle.
Items 2 and 3 each blow most of a session by themselves.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, or
`lvqr-archive` this session. Every non-WHEP egress crate stays
gated on the items above, plus eventually `lvqr-whep` itself
landing as the proof point that the egress shape generalizes
beyond HLS.

## Recommended Tier 2.3 entry point (session 11, closed)

Session 10 closed every in-crate item from the session 10 list
(parity gate, AAC coalescer round trip, sample segmenter). Session
11 inherited the cross-crate items that required touching the CLI
composition root and flipping dep directions. All four landed.
The original work list is preserved here for historical reference:

1. **Break the `lvqr-cmaf <-> lvqr-ingest` dev-dep cycle** before
   any session 11 work that requires `lvqr-ingest` to normal-dep
   `lvqr-cmaf`. Option A: move `parity_avc_init.rs` and
   `parity_avc_segment.rs` out of `crates/lvqr-cmaf/tests/` and
   into a new top-level `tests/parity/` directory as a standalone
   test crate that normal-deps both `lvqr-cmaf` and
   `lvqr-ingest`. Option B: move the parity tests into
   `crates/lvqr-ingest/tests/` as a dev-dep on `lvqr-cmaf`; this
   reverses the current direction and sets up item 2 cleanly.
   Pick option B unless option A turns up a better reason during
   implementation.

2. **Wire `lvqr-cli serve` to compose HLS**. Add an `--hls-addr`
   flag to `ServeConfig`, have `lvqr_cli::start` spin up an axum
   binding on that address with `HlsServer::router()`, and
   adapt the RTMP bridge's fragment output into the `HlsServer`
   push API via the pass-through `CmafSegmenter` (no coalescer
   needed yet -- the bridge still emits pre-muxed `Fragment`
   values). Day-one E2E: extend
   `lvqr-test-utils::TestServer` with an `hls_url()` helper and
   write a new integration test in `crates/lvqr-cli/tests/`
   that publishes a real RTMP stream, fetches
   `GET /playlist.m3u8`, and asserts the playlist contains the
   ingested broadcast's segments. This is the canonical "can
   LVQR serve HLS to a real HTTP client" proof.

3. **Retire the hand-rolled
   `lvqr-ingest::remux::fmp4::video_segment` writer behind a
   feature flag**. Prerequisite: item 1 must be done so the
   `lvqr-ingest` -> `lvqr-cmaf` normal-dep can be added. Then:
   add a `cmaf-writer` feature on `lvqr-ingest` (default off
   during the transition) that routes through
   `lvqr_cmaf::TrackCoalescer::flush_pending` +
   `lvqr_cmaf::build_moof_mdat` instead of the hand-rolled
   `video_segment`. Flip the feature on in a CI matrix job so
   both paths are exercised on every PR. When both are green on
   main for a few sessions, flip the default to on, then delete
   the hand-rolled writer in a later session.

4. **Scope the first non-HLS egress crate**. Likely `lvqr-whep`
   because WHEP is the simplest WebRTC-based subscribe path and
   it slots cleanly onto `CmafChunk` (WHEP consumers see
   `CmafChunk`s, not raw samples). Do NOT start implementation
   until items 1-3 above land; otherwise the crate has no real
   producer to validate against.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-srt`, `lvqr-rtsp`, or
`lvqr-archive` this session. Every non-HLS egress crate is gated
on the session 11 wiring above. Stay focused on closing the Tier
2.3 loop before any new protocol crate begins.

### Session 10 items from the prior HANDOFF (all closed)

1. **Add `CmafSegmenter::from_sample_stream`** plus a `SampleStream`
   trait. Scaffold:
   ```text
   pub trait SampleStream: Send {
       fn next_sample<'a>(&'a mut self)
           -> Pin<Box<dyn Future<Output = Option<RawSample>> + Send + 'a>>;
       fn meta(&self) -> &FragmentMeta;
   }
   ```
   The `CmafSegmenter::from_sample_stream` constructor owns a
   `HashMap<u32, TrackCoalescer>` keyed by track id and pulls
   samples via the trait, routing each into its track's
   coalescer. `next_chunk` returns the next flushed chunk across
   any track. This lets the lvqr-hls router consume a real
   producer (once one emits `RawSample` values) without an extra
   adapter layer.

2. **Wire `lvqr-cli serve` to compose HLS**. Add an `--hls-addr`
   flag to `ServeConfig`, have `lvqr_cli::start` spin up an axum
   binding on the address with `HlsServer::router()`, and teach
   the RTMP bridge to push CmafChunks into the `HlsServer`. The
   RTMP bridge today emits pre-muxed `Fragment` values; session
   10 can route those through the pass-through `CmafSegmenter`
   (no coalescer needed yet) and then into `HlsServer::push_chunk_bytes`.
   Day-one E2E: `lvqr-test-utils::TestServer::hls_url()` plus a
   real tokio HTTP client that publishes RTMP and GETs
   `/playlist.m3u8`, asserting the returned playlist contains
   the RTMP-ingested broadcast's segments.

3. **Retire the hand-rolled
   `lvqr-ingest::remux::fmp4::video_segment` writer behind a
   feature flag**. The session-7 parity gate proves the cmaf
   path is structurally equivalent for the AVC init segment; the
   session-9 coalescer now has ffprobe-validated media segment
   output too. The migration is a feature flag on `lvqr-cli` (or
   `lvqr-ingest`) that switches between the two writers. CI
   matrix runs with both settings; when both are green on main,
   the hand-rolled path moves to a `legacy-fmp4` feature gate
   and eventually to deletion.

4. **Audio coalescing round trip**. Extend
   `conformance_coalescer.rs` with an AAC variant so the
   audio-side coalescer state is covered by a real ffprobe run,
   not just the shared unit tests. Blocker today is that the AAC
   init writer refuses non-indexable sample rates; the test can
   pick 44.1 kHz or 48 kHz to stay on the happy path.

5. **Session 7 byte-diff followups against the hand-rolled
   writer**. Expand `parity_avc_init.rs` into
   `parity_avc_segment.rs` that compares coalescer output against
   `lvqr-ingest::remux::fmp4::video_segment` for the same sample
   sequence. If the bytes match structurally, the feature flag
   migration in item 3 is low-risk; if they differ, document
   the harmless differences and pin the structural assertions.

Do NOT start `lvqr-dash`, `lvqr-whip`, `lvqr-whep`, `lvqr-srt`,
`lvqr-rtsp`, or `lvqr-archive` until the `CmafSegmenter::from_sample_stream`
constructor and the `lvqr-cli` HLS composition above are in place.
Every egress beyond LL-HLS needs the coalescer to produce chunks
from arbitrary sample sources, not just the RTMP pre-muxed path.

## Recommended Tier 2.3 entry point (session 6, closed)

Session 6 closed item 3 from this list (grow `lvqr-cmaf` beyond AVC,
with the same ffprobe conformance harness extended to HEVC and AAC).
Items 1, 2, and 4 are now deferred to session 7 per the list above.
The original item text is preserved here for historical reference:

1. **Bootstrap the `lvqr-conformance` fixture corpus** now that
   ffmpeg is available locally. This has been BLOCKED since
   session 3 and unblocks codec conformance, HLS comparison
   harnesses, and the DASH path when those land. Capture a small
   matrix of FLV, fMP4, H.264, HEVC, and AAC bytes under
   `crates/lvqr-conformance/fixtures/` via `ffmpeg -f lavfi` and
   pin them into the corpus with a per-fixture `metadata.toml`
   stating the expected parser outputs.
2. **Add a codec conformance slot to `lvqr-codec`** using the new
   fixture corpus. Parser outputs (profile, level, resolution,
   sample rate, channel count) should match the corpus metadata
   exactly. This closes the last educational warning on
   `lvqr-codec` per `scripts/check_test_contract.sh`.
3. **Grow `lvqr-cmaf` beyond AVC**: add `write_hevc_init_segment`
   and `write_aac_init_segment` using `mp4-atom`'s `Hev1` / `Hvcc`
   / `Mp4a` / `Esds` types plus the new `lvqr_codec::hevc` and
   `lvqr_codec::aac` decoded values. Wire the cmaf init writer
   into `rtmp_ws_e2e` in parallel with the hand-rolled writer
   and diff the bytes as the first byte-level proof that
   `mp4-atom` output is a drop-in replacement for the current
   writer.
4. **Multi-sub-layer HEVC fixture capture**: try `kvazaar` or an
   nvenc-based HEVC encoder rather than x265 to get a real
   `sps_max_sub_layers_minus1 > 0` SPS on disk. If none of the
   available encoders produce one, capture an HEVC SPS from a
   publicly licensed sample (Apple bipbop? Big Buck Bunny HEVC
   rendition?) and pin the bytes.

Do NOT start any Tier 2 egress protocol crate (`lvqr-whip`,
`lvqr-whep`, `lvqr-hls`, `lvqr-dash`, `lvqr-srt`, `lvqr-rtsp`,
`lvqr-archive`) until `lvqr-cmaf` has a working segmenter that
emits real `moof + mdat` bytes from raw samples. The scaffold
landed in session 5 is a pass-through that annotates pre-muxed
fragments; the sample-coalescer is the actual Tier 2.3 load-bearing
piece.

---
**E2E Verified**: real RTMP publish -> RtmpMoqBridge -> MoQ origin -> axum WS
relay -> tungstenite WebSocket client, with fMP4 init (ftyp) and media (moof)
segments verified byte-by-byte. See `crates/lvqr-cli/tests/rtmp_ws_e2e.rs`.

The roadmap at `tracking/ROADMAP.md` is the authoritative plan for the next
18-24 months of work; read it alongside CLAUDE.md before starting anything.
Three audits sit next to it:

- `tracking/AUDIT-2026-04-13.md` (external) compares LVQR's current
  surface area against MediaMTX, LiveKit, OvenMediaEngine, SRS, Ant
  Media, AWS Kinesis Video Streams, Janus, and Jitsi, and calibrates
  the five strategic bets.
- `tracking/AUDIT-INTERNAL-2026-04-13.md` (internal) is the dead-code,
  latent-bug, and security-hardening audit of LVQR itself. Every
  critical claim was manually verified before landing. Five fixes
  shipped the same session.
- `tracking/AUDIT-READINESS-2026-04-13.md` (readiness) audits CI
  wiring, supply-chain, documentation drift, unwired CLI surface,
  and Tier 0/1 progress against the roadmap. Five fixes landed:
  README refresh, ffmpeg installed in CI, `--config` dead flag
  removed, plus this document.

## What Tier 0 Closed (2026-04-13)

The v0.4 audit found real bugs hiding behind v0.3.1's "green CI" claim. Tier 0
addressed each of them; the state today is:

1. **Graceful shutdown race fixed.** `crates/lvqr-cli/src/main.rs` now runs
   the relay, RTMP, and admin subsystems via `tokio::join!` with per-subsystem
   wrappers that cancel the shared token on exit. The outer `select!` arm
   that pre-empted draining subsystems on ctrl-c is gone.
2. **EventBus wired end-to-end.** `lvqr_core::EventBus` is created once in
   the CLI and handed to `RtmpMoqBridge::with_events`. The RTMP bridge emits
   `BroadcastStarted/Stopped` on publish/unpublish; the WS ingest handler
   emits the same events around its session; `spawn_recordings` subscribes
   to the bus instead of polling `bridge.stream_names()`, so WS-ingested
   broadcasts are recorded identically to RTMP-ingested ones.
3. **Player audio SourceBuffer fix.** Both `@lvqr/player` and the test app's
   watch tab now set `sb.mode = 'sequence'` only for video (`0.mp4`); audio
   stays in the default `segments` mode so fMP4 `baseMediaDecodeTime` is
   honored and A/V stays in lock.
4. **Tokens out of query strings.** WebSocket auth now travels in
   `Sec-WebSocket-Protocol: lvqr.bearer.<token>`. The new `resolve_ws_token`
   helper in `lvqr-cli` parses the header and echoes the exact subprotocol
   back so axum's upgrade handshake completes. `?token=` is still accepted
   during the transition but logs a deprecation warning per upgrade. The
   JS client (`bindings/js/packages/core/src/client.ts`) and the test app
   construct their WebSockets with the subprotocol array when a token is
   set, and the test app grew token inputs on both Watch and Stream tabs.
5. **Pluggable protocol scaffolding and auth crate.** `lvqr-auth` is a new
   crate with `AuthProvider`, `StaticAuthProvider`, `NoopAuthProvider`, and
   an optional `JwtAuthProvider` behind the `jwt` feature. `lvqr-ingest`
   gained `IngestProtocol` + an `RtmpIngest` adapter. `lvqr-relay` gained
   a mirror `RelayProtocol` trait. The object-safety mock test that the
   audit flagged as theatrical is gone.
6. **Real RTMP to WS E2E test.** `crates/lvqr-cli/tests/rtmp_ws_e2e.rs`
   drives a real rml_rtmp publisher through the bridge, subscribes via a
   real tokio-tungstenite WebSocket client, and asserts both an init
   segment (`ftyp`) and a media segment (`moof`) arrive over the wire.
   Zero mocks, zero helper-in-isolation assertions.

## Breaking Changes vs 0.3.1

- **WS auth transport**: prefer `Sec-WebSocket-Protocol: lvqr.bearer.<token>`
  over `?token=`. The query-string form still works but logs a deprecation
  warning and is scheduled for removal in a future release.
- **Recorder eligibility**: anything ingested over WebSocket is now recorded
  when `--record-dir` is set; previously only RTMP-ingested streams were.

## Next Up: Tier 1 (Test Infrastructure)

Per the roadmap, Tier 0 unblocks Tier 1: build the reference fixture corpus,
proptest harnesses, cargo-fuzz targets, testcontainers fixtures, playwright
E2E, ffprobe validation in CI, and the MediaMTX comparison harness. The
load-bearing architectural call after that (Tier 2) is the Unified Fragment
Model in `crates/lvqr-fragment/` and the `lvqr-moq` facade crate -- do NOT
add new protocol code before those two land.

The audit reorders two Tier 1 items:

1. The MediaMTX cross-implementation comparison harness graduates to a
   first-day CI requirement for Tier 2.5 (LL-HLS) rather than a late
   Tier 1 add-on. Bake it into `lvqr-conformance` during Tier 1 so
   Tier 2.5 does not have to build it later.
2. `lvqr-conformance` and the proptest/cargo-fuzz harnesses ship before
   `lvqr-chaos`. Chaos testing is valuable but does not block Tier 2
   the way the conformance corpus does.

## Tier 1 Progress as of 2026-04-13

Landed in this session:

1. **`lvqr-conformance` crate skeleton** (`publish = false`). Directory
   layout for fixtures under `fixtures/{rtmp,fmp4,hls,dash,moq,edge-cases}/`,
   `ValidatorResult` type with soft-skip semantics, README documenting
   the provenance metadata every fixture must ship with.
2. **Proptest harness** for `lvqr-ingest` parsers and fMP4 writer at
   `crates/lvqr-ingest/tests/proptest_parsers.rs`. `parse_video_tag` and
   `parse_audio_tag` tested to never panic across 1024 cases each;
   `video_init_segment_with_size` and `video_segment` tested to produce
   structurally well-formed ISO BMFF buffers across 256 cases each
   (2560 generated cases total, all green).
3. **Golden-file regression** for the fMP4 writer at
   `crates/lvqr-ingest/tests/golden_fmp4.rs` with two fixtures under
   `crates/lvqr-ingest/tests/fixtures/golden/`. `BLESS=1` regenerates
   both after intentional format changes.
4. **`ffprobe_bytes` helper** in `lvqr-test-utils` with
   `FfprobeResult::{Ok, Skipped, Failed}`. Tests soft-skip when ffprobe
   is not on PATH so contributor laptops without ffmpeg do not break CI.
5. **ffprobe wired into the golden fMP4 test** via a new
   `ffprobe_accepts_concatenated_cmaf` case that feeds the init segment
   plus a keyframe media segment into ffprobe and asserts acceptance.
6. **cargo-fuzz scaffold** for the FLV parsers at
   `crates/lvqr-ingest/fuzz/`, excluded from the main workspace so
   stable builds do not pull libfuzzer-sys. Two targets:
   `parse_video_tag` and `parse_audio_tag`. Nightly-only; runs via
   `cargo +nightly fuzz run <target>`.
7. **5-artifact test contract** documented at `tests/CONTRACT.md` with
   a table tracking each in-scope crate's current coverage of the
   five required artifacts (proptest, fuzz, integration, E2E,
   conformance). Educational during Tier 1; hard CI gate from Tier 2.

After this session, `lvqr-ingest` has four of the five artifacts for
its parsers and writers: proptest (new), cargo-fuzz (new, nightly),
integration (existing RTMP bridge test), and conformance (new golden
plus ffprobe). The fifth slot (browser E2E) is covered transitively
by the `lvqr-cli` rtmp_ws_e2e test. No other crate has full coverage yet.

## Internal Audit Fixes (2026-04-13)

The internal audit identified confirmed bugs, dead code, and hardening
targets. Five items landed in the same commit as the audit document:

1. **Broadcast path traversal hardening** in `lvqr-relay::parse_url_token`.
   A new `is_valid_broadcast_name` validator rejects names containing
   `..`, backslash, control characters, leading/trailing slashes, or
   anything outside `[A-Za-z0-9._/-]`. Empty names remain permitted
   because MoQ sessions legitimately connect to the relay root and
   select broadcasts via SUBSCRIBE. Six new unit tests, plus the five
   existing relay integration tests continuing to pass.
2. **Stale child reference fix** in `lvqr-mesh::reassign_peer`. The
   function overwrote the peer's parent field but never removed the
   stale child reference from the old parent's children list. Latent
   bug that only triggers on live rebalance (the orphan path calls
   `remove_peer` first which deletes the old parent entirely). Defensive
   fix plus a new regression test for the live-rebalance path.
3. **Theatrical heartbeat test replaced** in `lvqr-mesh`. The prior
   version set `heartbeat_timeout_secs = 0` and asserted nothing
   meaningful. New version exercises the full lifecycle: fresh peer
   alive, stale after 1.1s sleep, alive again after heartbeat.
4. **JWT provider wired into the CLI**. `JwtAuthProvider` was
   feature-complete but had zero consumers outside its own unit tests.
   `lvqr-cli` now pulls `lvqr-auth` with the `jwt` feature on and
   exposes `--jwt-secret` / `--jwt-issuer` / `--jwt-audience` plus
   matching `LVQR_JWT_*` env vars, taking precedence over static
   tokens.
5. **lvqr-mesh scaffolding comment** at the top of `crates/lvqr-mesh/src/lib.rs`
   making it explicit that the crate is a topology planner and no
   code in the repo yet drives real WebRTC DataChannel peer forwarding.
   The offload percentage exposed via the admin API is intended
   offload, not actual. Documentation change only.

Plus a new Tier 1 test that closes one of the audit's deferred items:

6. **lvqr-record integration test** at
   `crates/lvqr-record/tests/record_integration.rs`. Drives a
   synthesized MoQ broadcast through a real `record_broadcast` call
   in a tempdir and asserts the on-disk layout matches the documented
   structure. Also verifies that cancellation returns Ok cleanly within
   a timeout. Before this test, `record_track` had zero integration
   coverage; only the pure helpers (`looks_like_init`, `track_prefix`,
   `sanitize_name`) were tested.

## Readiness Audit Fixes (2026-04-13)

A third audit pass focused on readiness: what a new contributor or
future session encounters when they sit down to work. Five fixes
landed in the same commit as `tracking/AUDIT-READINESS-2026-04-13.md`:

1. **README refreshed** to v0.4-dev. Removed the stale "83 Rust
   tests, no auth, no recording" claims. Added current crate list
   including `lvqr-auth`, `lvqr-record`, `lvqr-conformance`. Added
   a pointer at the three audit documents and the roadmap.
2. **ffmpeg installed in CI** on both the Linux and macOS legs of
   the test matrix via apt and brew respectively. Before this
   change, the `ffprobe_accepts_concatenated_cmaf` test landed in
   Tier 1 kickoff silently soft-skipped on every CI run because
   ffprobe was not on PATH.
3. **`cargo test --workspace`** used on both matrix legs (previously
   split into `--lib` and `--test '*'` which skipped doc tests).
   Doctests in `lvqr-auth` and `lvqr-ingest::protocol` now run.
4. **Verify-ffprobe step** added to CI so if the ffmpeg install
   silently succeeds but ffprobe is not on PATH we fail fast with
   a loud error instead of silently skipping the conformance
   check.
5. **Dead `--config` CLI flag removed** from `lvqr-cli::ServeArgs`.
   The flag was declared but never read, leaking into `--help`,
   the README, the quickstart, and CONTRIBUTING as a capability
   lie. Will be re-added with a real loader alongside the Tier 3
   hot config reload work.

Tracked by the audit for later (not fixed this commit):

- `docs/architecture.md` still says `tokio::select!` for the CLI
  server composition. The Tier 0 fix was `tokio::join!`. Dedicated
  docs pass in Tier 5.
- `docs/quickstart.md` references a `/watch/my-stream` endpoint
  that does not exist.
- `CONTRIBUTING.md` crate list missing `lvqr-auth`, `lvqr-record`,
  `lvqr-conformance`, and references a `docker/docker-compose.test.yml`
  that does not exist.
- No cargo-audit job in CI. Supply-chain CVE scan deferred.
- No nightly cargo-fuzz runner wired up. The fuzz targets exist and
  compile under nightly but nothing runs them on a schedule.
- No playwright E2E suite. No 5-artifact CI enforcement script.

## Tier 1 Remaining Work

Big-ticket items still to build:

- `TestServer` in `lvqr-test-utils` that spawns a full LVQR binary (or
  calls `lvqr_cli::serve` directly once the CLI crate exposes a lib)
  with ephemeral ports and cleanup. Replaces ad-hoc server setup in
  every test file.
- testcontainers fixtures for MinIO (S3-compatible object storage),
  needed for the Tier 2.4 archive crate.
- `tests/e2e/` playwright suite that drives a real Chrome against the
  test app to exercise ingest plus playback. Trace recording on
  failure. Gating for the audio A/V drift soak test the audit calls
  out.
- ffprobe-backed validation of every fMP4 output in every test,
  swapping hand-rolled structural assertions for the external
  validator where practical.
- `mediastreamvalidator` wrapper in `lvqr-conformance` that runs Apple's
  HLS validator against generated playlists. Blocks on Tier 2.5 existing.
- Cross-implementation comparison harness: same RTMP into LVQR and
  MediaMTX, structural diff of HLS playlists. Blocks on Tier 2.5.
- 24-hour soak rig that runs synthetic publisher plus subscribers and
  asserts no memory growth, no FD leaks, no gauge drift.
- `lvqr-loadgen` crate for Rust-native data-plane load generation.
- `lvqr-chaos` crate for fault injection. Lowest priority per the audit.
- CI enforcement script for the 5-artifact contract. Educational PR
  comments in Tier 1, hard fail in Tier 2.

The `lvqr-fragment` and `lvqr-moq` crates from Tier 2.1 remain the
load-bearing architectural call. Do not ship new protocol code before
those two land. Read `tracking/AUDIT-2026-04-13.md` for the full
competitor comparison and the five strategic bets before arguing about
priority.

## End-to-End Pipeline (Proven Working)

## End-to-End Pipeline (Proven Working)

## End-to-End Pipeline (Proven Working)

```
Browser Webcam (getUserMedia)
    |
    v
VideoEncoder (H.264 Baseline, WebCodecs API)
    |
    v
WebSocket (/ingest/{broadcast}) -- [type][timestamp][AVCC NALUs]
    |
    v
LVQR Server (Rust)
  - Parses AVCC config (SPS/PPS, width/height)
  - Generates fMP4 init segment (ftyp+moov with avcC, correct dimensions)
  - Remuxes H.264 NALUs to fMP4 media segments (moof+mdat)
  - Publishes to MoQ tracks via OriginProducer
    |
    v
WebSocket (/ws/{broadcast}) -- forwards fMP4 binary frames
    |
    v
Browser Viewer (MSE)
  - Auto-detects codec from avcC box in init segment
  - Creates SourceBuffer in sequence mode
  - Chases live edge (seeks when >500ms behind)
  - Plays video
```

Also works with RTMP ingest (OBS/ffmpeg) via the same fMP4 remux pipeline.

## All Packages Published

### crates.io (8 crates)
lvqr-core, lvqr-signal, lvqr-relay, lvqr-ingest, lvqr-mesh, lvqr-admin, lvqr-wasm, lvqr-cli @ 0.3.1

### npm (2 packages)
@lvqr/core, @lvqr/player @ 0.3.1

### PyPI
lvqr @ 0.3.1

## Repository Structure

```
lvqr/
  crates/
    lvqr-core/          Shared types, ring buffer, GOP cache (25 tests)
    lvqr-relay/         MoQ relay on moq-lite, connection callbacks (4 integration tests)
    lvqr-ingest/        RTMP server + FLV-to-CMAF remuxer (26 lib + 2 integration tests)
    lvqr-mesh/          Peer tree coordinator (13 tests)
    lvqr-signal/        WebRTC signaling + mesh push (7 tests)
    lvqr-admin/         HTTP API: stats, streams, mesh (6 tests)
    lvqr-wasm/          WebTransport browser bindings
    lvqr-cli/           Single binary: relay + RTMP + admin + WS relay/ingest + mesh
    lvqr-test-utils/    Test helpers (publish = false)
  bindings/
    js/packages/
      core/             MoQ client, admin client, mesh peer (@lvqr/core)
      player/           <lvqr-player> web component (@lvqr/player)
    python/             Admin API client (lvqr on PyPI)
  test-app/             Brutalist test UI: stream, watch, admin
  tracking/             Handoff, audit, session notes
```

## Test App

`test-app/index.html` -- single-page brutalist test app with three tabs:

- **Stream**: Webcam capture via WebCodecs VideoEncoder (H.264 Baseline, level 4.0), streams over WebSocket to `/ingest/{broadcast}`. No ffmpeg or OBS needed.
- **Watch**: WebSocket fMP4 viewer with MSE SourceBuffer. Auto-detects codec from avcC box. Chases live edge to keep latency low.
- **Admin**: Real-time dashboard polling `/healthz`, `/api/v1/stats`, `/api/v1/streams`, `/api/v1/mesh`. Auto-refresh toggle.

Run:
```bash
lvqr serve                              # terminal 1
cd test-app && python3 -m http.server 9000  # terminal 2
# open http://localhost:9000
```

## Key Endpoints

| Endpoint | Protocol | Description |
|----------|----------|-------------|
| `:4443` | QUIC/UDP | MoQ relay (WebTransport/QUIC subscribers) |
| `:1935` | TCP | RTMP ingest (OBS/ffmpeg) |
| `:8080/healthz` | HTTP GET | Health check |
| `:8080/api/v1/stats` | HTTP GET | Publisher/subscriber/track counts |
| `:8080/api/v1/streams` | HTTP GET | Active stream list |
| `:8080/api/v1/mesh` | HTTP GET | Mesh peer count, offload % |
| `:8080/ws/{broadcast}` | WebSocket | fMP4 viewer relay (binary frames) |
| `:8080/ingest/{broadcast}` | WebSocket | Browser H.264 ingest |
| `:8080/signal` | WebSocket | WebRTC signaling for mesh peers |

## WS Ingest Wire Format

Binary WebSocket messages: `[u8 type][u32 BE timestamp_ms][payload]`

| Type | Payload |
|------|---------|
| 0 | Config: `[u16 BE width][u16 BE height][AVCDecoderConfigurationRecord]` |
| 1 | Keyframe: AVCC-format NALUs (length-prefixed) |
| 2 | Delta frame: AVCC-format NALUs |

The AVCC record comes from `VideoEncoder.output()` metadata's `decoderConfig.description`. NALUs use `avc: { format: 'avc' }` (length-prefixed, not Annex B).

## All Bugs Found and Fixed (12 total)

### Protocol bugs (found by reading moq-lite source, no browser needed)

| # | Bug | Impact |
|---|-----|--------|
| 1 | CLIENT_SETUP size: varint instead of u16 BE | Every MoQ connection fails |
| 2 | Path encoding: segmented array instead of plain string | Every subscribe returns NotFound |
| 3 | AnnouncePlease: 1 empty segment instead of 0 | Discovery returns nothing |
| 4 | Subscribe priority: varint instead of u8 | Misparse for priority > 63 |
| 5 | trun box version 0 for signed CTS | B-frame timestamps wrong |
| 6 | Player hardcoded codec string | Non-High-profile H.264 fails |
| 7 | Video+audio to single MSE SourceBuffer | MSE crash on first audio frame |

### E2E bugs (found during live browser testing)

| # | Bug | Impact |
|---|-----|--------|
| 8 | Init segment width=0 height=0 in avc1/tkhd | Chrome MSE rejects init |
| 9 | Duplicate init segments (stored + group frame 0) | MSE error after first append |
| 10 | No live-edge seeking | Unbounded latency growth |
| 11 | H.264 level 3.0 for 720p capture | VideoEncoder refuses to encode |
| 12 | No CORS headers on admin HTTP | Admin tab fetch() blocked |

## What Works (verified)

| Feature | How Verified |
|---------|-------------|
| Webcam -> browser -> LVQR -> browser viewer | E2E in Chrome |
| RTMP ingest (OBS/ffmpeg) -> fMP4 -> MoQ | 2 integration tests |
| FLV parsing (SPS/PPS, AAC config) | 12 unit tests |
| fMP4 generation (init + media segments) | 10 unit tests |
| MoQ QUIC fan-out (1 pub, 3 subs) | 3 integration tests |
| Relay connection callback | 1 integration test |
| Mesh tree assignment + orphan reassignment | 13 unit tests |
| Signal server message routing + push | 7 unit tests |
| Admin API (stats, streams, mesh) | 6 unit tests |
| Admin dashboard in browser | Manual test |
| Registry fanout benchmark | ~230ns to 500 subscribers |

## What's Not Done

| Feature | Status |
|---------|--------|
| MoQ/WebTransport browser path | Code written (SETUP, Subscribe, Group/Frame), WS fallback works, WebTransport path untested |
| WebRTC mesh media relay | Coordination works (tree + signal push), DataChannel code written, relay untested |
| Audio playback | Separate MSE SourceBuffer needed, not wired in player |
| Stream authentication | Not started |
| Recording | Not started |
| Multi-server federation | Not started |

## Key Technical Decisions

- **FLV-to-CMAF remux** in Rust (manual fMP4 box writer, no external crate). AVCC NALUs pass through unchanged since both FLV and fMP4 use length-prefixed format.
- **WebSocket for browser ingest** because browsers can't do RTMP (no TCP sockets). WebCodecs VideoEncoder provides hardware H.264 encoding.
- **WebSocket for browser playback** as fallback because MoQ/WebTransport version negotiation hasn't been E2E tested. The WS path is proven working.
- **Init segment per group** in MoQ tracks so late-joining subscribers always get codec config. The WS relay skips duplicate init segments to avoid MSE errors.
- **Live-edge chasing** in the viewer (seek forward when >500ms behind) because MSE in sequence mode accumulates buffer without bound.
- **CORS permissive** on the admin/WS server for development. Should be restricted in production.
