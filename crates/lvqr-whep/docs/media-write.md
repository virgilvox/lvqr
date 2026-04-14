# lvqr-whep media write design

Status: executed in session 22 (commit to follow). Everything in
this note is now reflected in `crates/lvqr-whep/src/str0m_backend.rs`
with ONE correction flagged inline below: Decision 2 was wrong —
str0m's `H264Packetizer` scans for Annex B start codes and silently
drops AVCC input. The executed code converts AVCC to Annex B at
the boundary; see the "Decision 2 (corrected)" section.

## Why this is its own session

Sessions 20 and 21 landed the `Str0mAnswerer` + `SessionHandle`
boundary and the sans-IO poll loop. A real browser that POSTs a
WHEP offer at `--whep-port` now completes ICE / DTLS / SRTP against
the server and sits in a silent SRTP tunnel. The server is
authoritative on the transport; what it still does not do is emit
any media packets to the subscriber.

The gap is not "call a function". There are four load-bearing
decisions that have to be made together or the resulting code is
wrong in a way compile-time checks will not catch:

1. Which str0m API emits H.264 bytes on the wire? Frame-level
   `Writer::write` or RTP-level `DirectApi::stream_tx`?
2. What byte format does the chosen API expect — AVCC (what lvqr
   produces) or Annex B?
3. How does the `Str0mSessionHandle::on_raw_sample` side (invoked
   from the ingest bridge's tokio task) hand bytes to the poll loop
   task (which owns `&mut Rtc`)?
4. How does the answerer learn the `Mid` for the video track and
   the RTP clock rate for the audio track?

This note answers all four and stops. The execution session picks
it up from here without re-doing the research.

## Decision 1: Writer (frame-level) over DirectApi (RTP-level)

`str0m::media::Writer::write(pt, wallclock, rtp_time, data)` takes
a frame of bytes, handles RTP packetization, SRTP sealing, pacing,
NACK, and FEC for the caller. `DirectApi::stream_tx` hands the
caller an RTP stream and expects already-packetized RTP packets.

Use `Writer`. Reasoning:

* The `H264Packetizer` at `crates/lvqr-whep/src/rtp.rs` is already
  tested with proptest and covers RFC 6184 single-NAL + FU-A, but
  it does not cover pacing, FEC, NACK retransmission, or SRTP
  sealing. Shipping the DirectApi path would require re-implementing
  those inside this crate, which is explicit scope creep.
* The briefing calls for "packetizing samples into RTP via the
  existing `H264Packetizer`" only as an MVP hand-wave; once the
  real implementation is being written, the incentive is to use
  what str0m provides rather than maintain a parallel packetizer.
* `H264Packetizer` is still load-bearing as proptest coverage of
  the RFC 6184 invariants (slot 1 of 5 in the test contract). It
  stays in the tree and keeps running in tests; it just is not
  wired into the send path. A future session may find a use for
  it (e.g. a DirectApi-driven SFU mode) and that is fine.

Follow-up: leave `crate::rtp::H264Packetizer` reachable, do not
remove the proptest slot, do not remove the re-exports from
`lib.rs`. The crate can own both an unused RFC 6184 packetizer and
a `Writer`-based send path without contradiction.

## Decision 2 (corrected): AVCC must be converted to Annex B

The session-21 version of this note claimed AVCC passthrough would
work. It does not. `str0m::packet::h264::H264Packetizer::packetize`
starts by calling `next_ind` to scan for Annex B start codes
(`0x00 0x00 0x01` or `0x00 0x00 0x00 0x01`). When the input has no
start code, the whole buffer is handed to `emit`, which reads
byte 0 as the NAL header. On AVCC bytes the first byte is the
high byte of a 32-bit length prefix — almost always `0x00`, which
decodes as NAL type 0, which is the "unspecified" NAL and is
silently dropped. **An AVCC-passthrough build would compile, run,
complete ICE/DTLS/SRTP, and emit zero video packets.** No error
anywhere; the failure mode is a black screen.

The in-tree fix is a small converter,
`str0m_backend::avcc_to_annex_b`, that walks AVCC length prefixes
and prepends `0x00 0x00 0x00 0x01` before each NAL body. The
converter has five unit tests covering single NAL, multi NAL,
empty input, truncated length prefix, length overruns buffer, and
zero-length NAL entries. Malformed input falls through to an
empty vec rather than panicking, so the converter is safe to run
on attacker-shaped bytes.

Upstream issue: none filed. str0m's API is internally consistent
(depacketizer outputs Annex B unless `is_avc = true`, packetizer
expects Annex B) and switching its packetizer to accept both
formats would be an upstream behavior change, not a bug fix.
Conversion at the boundary is the right layering.

## Decision 3: mpsc channel from observer to poll task

`Str0mSessionHandle::on_raw_sample` is invoked from the RTMP
callback chain via the bridge's `RawSampleObserver` fanout. The
poll task owns `Rtc` by value and cannot share it (`Rtc: Send +
!Sync`). So the observer must not touch the `Rtc` at all; it
forwards the sample over a channel and returns immediately.

Design:

```rust
enum SessionMsg {
    Sample {
        track: TrackKind,      // Video | Audio
        payload: Bytes,        // cheap Clone of RawSample::payload
        dts: u64,              // sample.dts in the ingest timescale
        duration: u32,
        keyframe: bool,
    },
    Trickle(Vec<u8>),          // for completeness; still TODO
}

pub struct Str0mSessionHandle {
    tx: mpsc::UnboundedSender<SessionMsg>,
    shutdown: Option<oneshot::Sender<()>>,
    // warn flags as today
}
```

The poll loop gains a fourth arm in its `tokio::select!`:

```rust
tokio::select! {
    biased;
    _ = &mut shutdown => return,
    Some(msg) = rx.recv() => handle_session_msg(&mut rtc, &ctx, msg),
    recv = socket.recv_from(&mut buf) => { ... }
    _ = tokio::time::sleep(sleep_dur) => { ... }
}
```

`rx.recv()` resolving `None` means every sender has been dropped,
which is another clean shutdown condition (handle dropped). `biased;`
preserves shutdown priority over sample ingest.

Channel type: `mpsc::unbounded_channel`. A bounded channel would
either drop samples (wrong for a live relay where the ingest side
is the source of truth) or backpressure the RTMP callback chain
(which must not block or ingest stalls). Unbounded is the correct
choice here; the memory-growth risk is bounded by the ingest
side's own pacing: you cannot enqueue faster than RTMP produces.

Cost per sample: one `Bytes::clone` (cheap refcount bump), one
enum boxing, one `unbounded_send`. For a 30 fps H.264 stream with
N WHEP subscribers that is 30 * N per second of very small work;
the fanout itself already loops the session map once per sample,
so this adds no new asymptotic cost.

## Decision 4: Mid and clock-rate discovery

### Video mid

`Event::MediaAdded { mid, kind: MediaKind::Video, .. }` fires from
`poll_output` once the SDP answer has been applied. The poll task
captures both the video and audio `Mid` in a local
`SessionContext`:

```rust
struct SessionContext {
    video_mid: Option<Mid>,
    audio_mid: Option<Mid>,
    video_pt: Option<Pt>,     // resolved via writer.payload_params()
    audio_pt: Option<Pt>,
    connected: bool,
}
```

On `Event::Connected`, flip `connected = true`. Until then, drop
samples (the `Writer::write` docs say writes before Connected are
dropped anyway, but explicit is cheaper than reaching into str0m).

Inside `handle_session_msg` the task calls `rtc.writer(mid)` and
then `writer.write(pt, wallclock, rtp_time, payload)`.

### Video clock rate is 90 kHz on both sides

The ingest bridge writes video at a 90 kHz timescale
(`crates/lvqr-ingest/src/remux/fmp4.rs:85` hardcodes `90000` as
the trak timescale). WebRTC RTP video uses 90 kHz as the clock
rate per RFC 3551. That means `sample.dts` is already in 90 kHz
ticks and converts straight to `MediaTime::new(sample.dts, 90_000)`
with zero arithmetic.

This is a happy accident of the bridge's existing choice and the
WebRTC spec, not a negotiated invariant. Assert on it if you are
paranoid, but do not architect around the possibility of a
mismatch: fixing that would be a bridge change, not a WHEP change.

### Audio clock rate is NOT matched — ship video first

`crates/lvqr-ingest/src/bridge.rs:351` sets
`audio_timescale = config.sample_rate`, which is whichever sample
rate the AAC stream from RTMP is at — typically 44100 or 48000.
WebRTC negotiates Opus at 48 kHz. Two independent problems:

1. **Codec mismatch**: the RTMP payload is AAC raw access units,
   WebRTC wants Opus packets. No conversion without a real
   transcoder; AAC passthrough over Opus PT is not legal.
2. **Clock-rate mismatch**: if the codec problem is ever solved,
   we still need `(audio_dts * 48000) / audio_timescale`.

Conclusion: audio write is out of scope for this session and the
next. Video-only WHEP egress is a complete user story; Opus audio
is gated on either (a) an AAC -> Opus transcoder crate, or (b) a
second ingest path (WebRTC publish) that produces Opus directly.
Track this as a known gap in HANDOFF, not a TODO to fix inline.

### No `on_init` on `RawSampleObserver` for WHEP

`FragmentObserver::on_init` carries a `timescale` for consumers
like LL-HLS that do not have an implicit 90 kHz assumption.
`RawSampleObserver` deliberately does not: WHEP only needs video,
and video timescale is always 90 kHz for the bridge. Do not add
`on_init` to `RawSampleObserver` just to make the audio path
compile cleaner — the audio path is not going to compile this
session or the next for the reasons above.

## Wallclock

`Writer::write` takes a `wallclock: Instant`. The sample arrived
from ingest on the tokio task; the arrival time is good enough for
a "simple SFU" (str0m's own phrasing). Use
`std::time::Instant::now()` at the point the message is popped off
the channel. For better sync with other subscribers later, the
ingest bridge could attach a wall clock to the sample at emission
time, but that is a bridge change and not a WHEP concern.

## Order of operations for the execution session (done in session 22)

1. **Verified** by reading `str0m::packet::h264` that the packetizer
   expects Annex B. Found the silent-drop failure mode described in
   Decision 2 (corrected). Landed `avcc_to_annex_b` at the
   boundary.
2. Added `SessionMsg::Video { payload, dts, keyframe }` and a
   `mpsc::UnboundedSender<SessionMsg>` field on
   `Str0mSessionHandle`. `on_raw_sample` routes `track == "0.mp4"`
   onto the sender and drops non-video with a one-shot warn.
3. Added `SessionCtx { video_mid, video_pt, connected,
   write_error_logged, first_write_logged }` inside the poll task.
   `absorb_event` handles `Connected`, `MediaAdded(video)`, and
   `IceConnectionStateChange(Disconnected)`. `video_pt` is resolved
   lazily via `Writer::payload_params` filtered on
   `Codec::H264`.
4. Added the `samples.recv()` arm to the `select!` with `biased;`
   shutdown priority. `write_video_sample` guards on
   `connected && video_mid && video_pt`, converts AVCC to Annex B,
   and calls `writer.write(pt, Instant::now(),
   MediaTime::new(dts, Frequency::NINETY_KHZ), annex_b)`. First
   success and first error are each logged once.
5. **Still open**: real browser E2E test. Requires a WHEP client
   binary (`simple-whep-client` or similar) the CI image does not
   yet carry, plus an ffmpeg RTMP push harness wired into the
   existing `lvqr-test-utils::TestServer`. Tracking as v0.5 gate in
   HANDOFF session 22 notes; does not block this commit.
6. **Kept the audio-warn flag**; it still serves its purpose as
   the AAC -> Opus path is explicitly out of scope.

## Stop conditions

If any of these fail, stop and document rather than pushing
through:

* str0m rejects AVCC and wants Annex B. Add conversion or open an
  upstream issue; do not silently paper over the format mismatch.
* `Event::MediaAdded` never fires because the offer and answer
  negotiated no sendonly direction from the server side. That is
  an offer/answer bug in `Str0mAnswerer::create_session` and the
  fix is there, not in the media path.
* `Writer::write` returns `RtcError::UnknownPt`. The PT resolution
  via `writer.payload_params()` picked the wrong params. Re-read
  how str0m's `match_params` works and use it.
* ICE connects but DTLS never completes. This is a crypto-provider
  or certificate issue, not a media-write issue; media write is
  blocked until it is resolved.

## What this note is not

Not a replacement for the execution itself. Not a general WebRTC
tutorial. Not a design for audio, DASH, or RTSP egress. Those are
all correctly deferred elsewhere.
