# `lvqr-whep` design note (session 11 scope, not yet implemented)

This is the scoping artifact the session 11 work list (item 4) calls
for. It exists so a future session can land the first non-HLS egress
crate against a definite contract instead of a blank page.

The crate is **not implemented yet**. There is no `Cargo.toml`, no
`src/`, and `lvqr-whep` is not a workspace member. This file is the
only artifact the session is allowed to land per the session 11
directive. Implementation begins in a later session, after items 1
through 3 have been on `main` for at least one release cycle and the
`cmaf-writer` matrix job is green by default.

## Why WHEP first

The competitive audit
(`tracking/AUDIT-2026-04-13.md`) ranks WebRTC HTTP Egress Protocol
(WHEP) as the highest-leverage next egress because:

1. Every browser player ships WebRTC support out of the box. WHEP is
   the standardized HTTP signaling path on top of it.
2. The egress is sub-second by design, complementing the LL-HLS
   target (~2 s) without competing with it.
3. The SDP / HTTP signaling shape maps cleanly onto an axum router
   in the same style as `lvqr-hls::HlsServer`.
4. WHEP and WHIP are the two pieces every Tier-A competitor (LiveKit,
   MediaMTX, Ant Media) treat as table stakes, and the absence of
   either is the first thing an evaluator notices.

The non-WHEP options were considered and deferred:

| Crate | Why deferred |
|---|---|
| `lvqr-dash` | DASH adds a second manifest format with no new audience; HLS already covers the "stream to a player" case for v1.0. Defer to Tier 2.6. |
| `lvqr-whip` | WHIP is the publish side. We already have RTMP for publish; WHEP unlocks a brand-new audience. WHIP becomes the v1.1 target. |
| `lvqr-srt` | Broadcast-encoder ingest, not egress. Different roadmap slot. |
| `lvqr-rtsp` | IP camera ingest. Surveillance market is downstream of v1.0 broadcast scope. |
| `lvqr-archive` | Already partly designed in Tier 2.4; needs the recorder + redb index landing in its own session, not coupled to a new egress. |

## What WHEP needs from `CmafChunk`

WHEP is RTP-over-WebRTC. The wire shape that hits the subscriber's
browser is **per-NAL-unit RTP packets** for video (RFC 6184 H.264, RFC
7798 HEVC, RFC 6716 Opus for audio). That is **not** what `CmafChunk`
carries today.

Today `CmafChunk` exposes a pre-muxed `moof + mdat` payload plus the
segmenter's policy classification (Partial / PartialIndependent /
Segment). The mdat carries AVCC length-prefixed NAL units, not the
Annex-B start-code stream RTP wants, and certainly not packetized
into MTU-sized chunks.

Two paths are viable:

### Path A: consume `RawSample` directly via `SampleStream` (preferred)

`lvqr-cmaf` already exposes a `SampleStream` trait
(`crates/lvqr-cmaf/src/sample.rs`) that yields per-sample
[`RawSample`] values with `payload`, `dts`, `cts_offset`, `duration`,
and `keyframe` fields. A WHEP server tap subscribes to a
`SampleStream` instead of a `CmafChunk` stream. Each `RawSample`
becomes:

* a starting RTP timestamp (`dts` mapped to the H.264 90 kHz clock),
* a list of NAL units (strip the 4-byte AVCC length prefix to get
  the raw NAL bytes),
* one or more RTP packets per NAL unit using STAP-A or FU-A
  fragmentation per RFC 6184 §5.4-§5.6, sized against the
  negotiated path MTU (default 1200 bytes for safety).

This path is clean: WHEP never re-parses an mdat and never undoes
work the segmenter did. It is also the path the `cmaf-writer`
feature flag work (session 11 item 3) is implicitly preparing for:
both writers now agree on per-sample `tfdt + trun`, so the producer
side can emit `RawSample` once and feed both the `TrackCoalescer`
(for HLS / DASH / archive) and the WHEP RTP packetizer with no
duplication.

The single dependency this path adds is that the RTMP bridge in
`lvqr-ingest` must expose a `SampleStream` (or its `RawSample`
sequence) alongside the existing pre-muxed `Fragment` output. The
Fragment Observer hook landed in session 11 (`crates/lvqr-ingest/src/observer.rs`)
is the pattern to extend: a new `RawSampleObserver` trait or a
broadened `FragmentObserver::on_raw_sample` method gives the bridge
a place to hand samples to a registered consumer without coupling
the bridge to WHEP-specific types.

### Path B: parse `CmafChunk` on the wire (transitional shim)

If the producer side is not yet emitting `RawSample`, WHEP can fall
back to parsing each `CmafChunk::payload` on the wire: walk the
`moof.traf.trun` entries to recover per-sample sizes and offsets,
then carve the corresponding bytes out of the `mdat` and packetize.
The HANDOFF claim that WHEP "slots cleanly onto the CmafChunk type"
is correct **only via this fallback path**.

Path B is acceptable for a first prototype that wants to ship before
the bridge raw-sample work lands, but it is not the long-term plan.
The design should make the path-A wiring the default and treat
path B as a feature-flagged shim.

## Signaling handshake mapping onto axum

WHEP signaling is HTTP/1.1 SDP exchange:

1. **`POST /whep/{broadcast}`** with `Content-Type: application/sdp`
   and an offer SDP in the request body. The server constructs the
   answer SDP, allocates an ICE/DTLS endpoint, and returns:
   * `201 Created`
   * `Content-Type: application/sdp`
   * `Location: /whep/{broadcast}/{session_id}` (resource URL the
     client uses for subsequent operations)
   * answer SDP in the response body.
2. **`PATCH /whep/{broadcast}/{session_id}`** for trickle-ICE
   candidates. Body is the SDP fragment containing the candidate.
   Returns `204 No Content`.
3. **`DELETE /whep/{broadcast}/{session_id}`** to terminate. Returns
   `200 OK`.

This maps onto an axum `Router` exactly like
`lvqr_hls::server::HlsServer::router`:

```rust
Router::new()
    .route("/whep/{broadcast}", post(handle_offer))
    .route("/whep/{broadcast}/{session_id}", patch(handle_trickle))
    .route("/whep/{broadcast}/{session_id}", delete(handle_terminate))
    .with_state(state)
```

The `state` is a `WhepServer` analogue of `HlsServer`: an
`Arc<WhepState>` carrying a `DashMap<SessionId, ActiveSubscriber>`
and a handle to the `RawSample` source tap. The router is mounted
under the same axum binding as the LL-HLS surface in `lvqr-cli`
(or under its own `--whep-addr` flag, TBD).

The actual WebRTC stack lives behind one dependency: **`str0m`**
(roadmap library decision, sans-IO, the reference Rust SFU
toolkit). `str0m::Rtc` owns the DTLS/ICE/SRTP state machine for one
peer connection. The WHEP server hands the `Rtc` an outbound RTP
packet on every push and reads back the encrypted SRTP bytes plus
ICE / DTLS handshake bytes; a small UDP socket task forwards those
bytes between `Rtc` and the network.

## Crates this design reuses

| Existing crate | Reuse |
|---|---|
| `lvqr-cmaf` | `RawSample`, `SampleStream`. The producer side of the path-A wiring. |
| `lvqr-fragment` | `Fragment` + the `FragmentObserver` hook landed in session 11 for the path-B fallback. |
| `lvqr-ingest` | `RtmpMoqBridge` is the RTMP -> sample source. WHEP plugs in via the same observer pattern that `lvqr-cli::hls::HlsFragmentBridge` uses today. |
| `lvqr-hls::server` | `HlsServer::router` is the axum-routing template. Same shape: `Arc<State>` shared between push API and handlers, `Notify`-based wakeups, no middleware. |
| `lvqr-auth` | `AuthContext::Subscribe { token, broadcast }` already exists. WHEP authenticates on the offer POST. |
| `lvqr-core` | `EventBus::ViewerJoined` / `ViewerLeft` for telemetry on subscribe / unsubscribe. |

New external dependencies:

| Crate | Version | Why |
|---|---|---|
| `str0m` | latest | WebRTC sans-IO state machine. Roadmap library decision; mature; one of two Rust options (the other, `webrtc-rs`, is significantly heavier and less actively maintained). |
| `sdp-rs` or `sdp` | latest | SDP parsing for the offer / answer body. `str0m` provides its own SDP layer; reuse it if the API permits. |

No new system dependencies. WHEP runs in pure Rust over UDP. No
ffmpeg, no libvpx, no platform codec hooks.

## 5-artifact contract plan

Per `tests/CONTRACT.md`. Each row is a deliverable that must land
before the crate ships its first 0.x release; nothing is optional.

| Slot | Concrete deliverable |
|---|---|
| **proptest** | `crates/lvqr-whep/tests/proptest_packetizer.rs`. Property: the H.264 RTP packetizer never panics on arbitrary AVCC length-prefixed NAL sequences and the concatenation of payloads of every emitted RTP packet round-trips back to the original NAL sequence (modulo Annex-B / AVCC framing). |
| **fuzz** | `crates/lvqr-whep/fuzz/fuzz_targets/parse_offer_sdp.rs`. Target: feed `arbitrary` SDP byte sequences to the offer parser, assert no panic. Seeded from the Pion / `webrtc-rs` test vectors plus a small handful of real-browser offers captured via Chrome devtools. |
| **integration** | `crates/lvqr-whep/tests/integration_signaling.rs`. Drive a real `WhepServer` axum router via `tower::ServiceExt::oneshot` (the same pattern `lvqr-hls`'s `tests/integration_server.rs` uses), assert the offer / answer / trickle / terminate cycle works against a synthetic offer SDP. |
| **e2e** | `crates/lvqr-cli/tests/rtmp_whep_e2e.rs`. Sister to the session-11 `rtmp_hls_e2e.rs`: publish via real RTMP to a TestServer, run a WHEP client (`webrtc-rs` or Pion through a subprocess) against `/whep/{broadcast}`, assert one decoded video frame arrives. The webrtc-rs client crate exists; if its API is too heavy a subprocess Pion client is the fallback. |
| **conformance** | Cross-implementation test against `simple-whep-client` (the IETF reference WHEP client maintained by Lorenzo Miniero / Meetecho). Soft-skip if the binary is not on PATH, same pattern as `lvqr_test_utils::mediastreamvalidator_playlist`. |

E2E exemption: `lvqr-whep` ships its own E2E in `lvqr-cli` rather
than in-crate, same pattern as `lvqr-cmaf` and `lvqr-hls`. Track the
exemption via `CONTRACT_E2E_EXEMPT_lvqr_whep=1` in the CI workflow
when strict mode flips on.

## Out of scope for the first release

The first WHEP release is a single-rendition video subscriber for
the AVC + AAC broadcast that the LL-HLS path already serves. The
following are explicit non-goals for v0.x:

* Audio-only subscribers. Audio support lands once the video path
  is conformance-clean.
* Simulcast layer selection. WHEP does not standardize it; it
  ships in a v1.x extension.
* SVC. Same reason; deferred to the audit's Bet 5 work.
* Bandwidth estimation / congestion control beyond what `str0m`
  provides out of the box.
* Recording the WHEP RTP stream to disk. Recording is the LL-HLS
  path's responsibility; the WHEP RTP and the HLS CMAF chunks
  share the same upstream `RawSample` source.
* WHIP (publish). Separate crate, separate session.

## Sequencing

WHEP cannot start until the producer side emits `RawSample` cleanly
through the bridge. That requires either:

1. The `cmaf-writer` feature has been on by default for at least one
   release cycle (so the parity gate is no longer load-bearing and
   the bridge can be cut over to a sample-emitting path), **or**
2. A new `RawSampleObserver` hook is added to `RtmpMoqBridge`
   alongside the existing `FragmentObserver` so WHEP can subscribe
   to per-sample data without disturbing the pre-muxed Fragment
   output.

Option 2 is the lower-risk path because it leaves the existing MoQ /
HLS data plane untouched. The observer trait is already in place;
adding a sibling method is one crate change in `lvqr-ingest`.

## Open questions

The first implementation session must answer these before writing
code:

1. **Where does the RTP packetizer live?** A standalone
   `lvqr-whep::rtp::H264Packetizer` is the obvious home, but if a
   future `lvqr-whip` implementation needs the same packetizer
   inverted (depacketize RTP -> NAL units), promoting it to a new
   `lvqr-rtp` crate may be cleaner. Decision can wait until WHIP
   work begins.
2. **One UDP socket per session, or shared?** `str0m` is sans-IO,
   so the choice is up to LVQR. Per-session sockets are simpler;
   shared sockets demultiplex via ICE-lite remote address.
   Per-session is the path-of-least-resistance for v0.x.
3. **Default WHEP bind address.** Match HLS at port `8889` (HLS sits
   on `8888` after session 11)? Or share the admin axum binding the
   way `lvqr-hls`'s router is currently mountable? Decision is a
   `--whep-addr` flag debate in the implementation session, not now.
4. **Token transport.** `Authorization: Bearer <token>` on the
   offer POST is the WHEP convention, but `lvqr-auth` already
   accepts query-param and `Sec-WebSocket-Protocol` tokens for the
   WS surface. The WHEP server should accept `Authorization:` and
   defer the legacy fallbacks to a later session.

These are the issues the WHEP implementer should escalate at the
top of the next session before touching any code.
