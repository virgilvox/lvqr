# SCTE-35 ad-marker passthrough

**Status:** v1 shipped in session 152 (2026-04-25).

LVQR passes SCTE-35 splice events from the publisher side through
to LL-HLS and DASH egress without interpretation. Operators who
mark ad breaks at the encoder see those marks render as
`#EXT-X-DATERANGE` lines on the HLS playlists and as
`<EventStream>` / `<Event>` elements on the DASH MPD; clients that
care (downstream ad servers, SSAI proxies, manifest manipulators)
consume the original `splice_info_section` bytes verbatim.

This is a passthrough surface, not an ad-decisioning surface. LVQR
never chooses to insert, replace, or black out content based on
the events it receives.

## Standards references

* **ANSI/SCTE 35-2024** -- splice_info_section binary format
  (section 8.1) and DASH carriage convention (section 12.2).
* **draft-pantos-hls-rfc8216bis** section 4.4.5 -- HLS DATERANGE
  attribute semantics, including SCTE35-OUT / SCTE35-IN /
  SCTE35-CMD hex-sequence rendering.
* **ISO/IEC 23009-1** section 5.10 + Annex G.7 -- DASH EventStream
  + Event shape, Period-level placement.
* **SCTE 214-1** -- DASH carriage of SCTE-35 in the
  `urn:scte:scte35:2014:xml+bin` scheme.

## Ingest paths

| Path | Status | Convention |
|------|--------|------------|
| SRT MPEG-TS | shipped (session 152) | PMT stream_type 0x86 on a dedicated PID (typically 0x1FFB by broadcast convention); section reassembly across TS packet boundaries. |
| RTMP onCuePoint | shipped (session 152 follow-up) | AMF0 Data message with method name `onCuePoint` and an object carrying `name="scte35-bin64"` + `data="<base64 splice_info_section>"`. The Adobe convention used by OBS, Wirecast, vMix, and ffmpeg's `-bsf:v scte35` pipeline. Wired via a vendored `rml_rtmp` v0.8 fork at `vendor/rml_rtmp/` that adds an `Amf0DataReceived` ServerSessionEvent so non-`@setDataFrame` AMF0 Data messages reach LVQR's RTMP path; without the patch the upstream library silently drops them. The fork is loaded via `[patch.crates-io]` in the workspace `Cargo.toml`. |
| WHIP / WebRTC | deferred | No widely-adopted publisher convention for in-band SCTE-35 over WebRTC data channels. |
| RTSP | deferred | No publisher convention; would require RDT or custom DESCRIBE handling. |

## Parser

`lvqr-codec/src/scte35.rs` exposes
`parse_splice_info_section(bytes) -> Result<SpliceInfo, CodecError>`.
The parser:

* Validates the trailing CRC_32 (MPEG-2 polynomial 0x04C11DB7,
  initial 0xFFFFFFFF). Sections that fail CRC return
  `CodecError::Scte35BadCrc { computed, wire }` and never reach
  the egress.
* Decodes the timing fields the egress renderers need:
  `command_type`, `pts_adjustment`, splice_time PTS (from
  `splice_insert.splice_time` or `time_signal.splice_time`),
  `break_duration` (when the splice_insert sets the duration
  flag), `splice_event_id`, and the cancel /
  out_of_network_indicator flags.
* Preserves the entire section verbatim in `SpliceInfo::raw` for
  downstream re-emission. The raw bytes flow through to HLS as
  `0x...` hex and to DASH as base64 with no LVQR-side
  modification.

The parser does NOT decode descriptors (segmentation_descriptor
etc.) or interpret splice semantics. Operators who need
descriptor-level decisions should consume the raw bytes from the
egress and decode them in their own pipeline.

## Egress wire shapes

### HLS

Each rendered video media playlist (one per variant rendition;
audio + captions playlists are unaffected) gains a
`#EXT-X-DATERANGE` block at the playlist head, scoped to the
playlist's current segment window. Ad markers age out alongside
the segment whose `#EXT-X-PROGRAM-DATE-TIME` precedes the next
remaining segment's PDT.

```
#EXTM3U
#EXT-X-VERSION:9
#EXT-X-TARGETDURATION:2
#EXT-X-MEDIA-SEQUENCE:1234
#EXT-X-MAP:URI="init.mp4"
#EXT-X-DATERANGE:ID="splice-1234567",START-DATE="2026-04-25T18:30:00.000Z",DURATION=30.000,SCTE35-OUT=0xFC301...
#EXT-X-PROGRAM-DATE-TIME:2026-04-25T18:30:00.000Z
#EXT-X-PART:DURATION=0.200,URI="part-1234-0.m4s",INDEPENDENT=YES
...
#EXTINF:2.000,
seg-1234.m4s
```

Attribute mapping per `splice_command_type`:

| splice_command_type | out_of_network | HLS attribute |
|---------------------|----------------|---------------|
| splice_insert (0x05) | 1 (going to ad) | SCTE35-OUT |
| splice_insert (0x05) | 0 (returning from ad) | SCTE35-IN |
| splice_null, time_signal, splice_schedule, bandwidth_reservation, private_command, splice_insert with cancel | -- | SCTE35-CMD |

`DURATION` is rendered only when the splice_insert sets the
duration flag (with `break_duration` in the section body).
`SCTE35-OUT` / `SCTE35-IN` / `SCTE35-CMD` carry the raw
splice_info_section as `0x` followed by uppercase hex per HLS spec
section 4.4.5.1.

### DASH

The MPD's single Period gains an `<EventStream>` child with
`schemeIdUri="urn:scte:scte35:2014:xml+bin"` and `timescale=90000`.
Each splice event renders as one `<Event>` carrying the
base64-encoded splice_info_section inside a `<Signal><Binary>`
body per SCTE 214-1. EventStream elements are emitted before
AdaptationSets per ISO/IEC 23009-1 section 5.3.2.1 ordering.

```xml
<Period id="0" start="PT0S">
  <EventStream schemeIdUri="urn:scte:scte35:2014:xml+bin" timescale="90000">
    <Event presentationTime="8100000" duration="2700000" id="1234567">
      <Signal xmlns="http://www.scte.org/schemas/35/2016">
        <Binary>/DAvAAAAAAAAAA/wBQb+...AAAAAAA=</Binary>
      </Signal>
    </Event>
  </EventStream>
  <AdaptationSet contentType="video" ...>
    ...
  </AdaptationSet>
</Period>
```

`presentationTime` is the absolute splice PTS in 90 kHz ticks
(`splice_time.pts_time + pts_adjustment` masked to 33 bits).
`duration` is the splice `break_duration` in 90 kHz ticks; the
attribute is omitted when the splice command sets no duration.
`id` is the SCTE-35 `splice_event_id` (or zero for command types
that have no event_id).

## Internal architecture

The event surface is a parallel "scte35" track on the existing
`FragmentBroadcasterRegistry`, mirroring the WebVTT captions
pattern from `lvqr-agent-whisper`. The reserved track name lives
on `lvqr_fragment::SCTE35_TRACK`.

```text
                              +---------------------------+
                              |  FragmentBroadcasterRegistry  |
                              +---------------------------+
                                            |
   SRT MPEG-TS PID 0x86       publish_scte35()           subscribe ("scte35")
   ----+                            |                            |
       |                            v                            v
       |     +------------+    +-----------+    +-------------------------+
       +---->| TsDemuxer  |--->| Fragment  |--->| BroadcasterScte35Bridge |
             | section    |    | (scte35)  |    | (lvqr-cli)              |
             | reassembly |    +-----------+    |  - parse_splice_info... |
             +------------+                     |  - hls.push_date_range  |
                                                |  - dash.push_event      |
                                                +-------------------------+
                                                            |
                                       +--------------------+--------------------+
                                       v                                         v
                              +-----------------+                       +-----------------+
                              |  lvqr-hls       |                       |  lvqr-dash      |
                              |  PlaylistBuilder|                       |  DashServer     |
                              |  date_ranges    |                       |  event_streams  |
                              +-----------------+                       +-----------------+
                                       |                                         |
                                       v                                         v
                          #EXT-X-DATERANGE in playlist                <EventStream> in MPD
```

Per-broadcast drain task spawns on the first
`(broadcast, "scte35")` registry entry creation. The drain runs
until every producer-side clone of the scte35 broadcaster drops,
then exits cleanly.

## Anti-scope

* No semantic interpretation of splice events. LVQR never decides
  to insert, replace, black out, or schedule based on a SCTE-35
  event. Ad-decisioning is the operator's responsibility (typically
  via a downstream SSAI proxy that consumes the egress playlists).
* No SCTE-104 (the studio-side wire format that pre-dates SCTE-35).
* No mid-segment splice handling. Events whose PTS falls inside an
  in-flight segment surface on the next segment boundary; the relay
  does not split segments to align splices.
* No transcoder-level mid-stream IDR insertion. Operators who need
  IDR-aligned splices configure their upstream transcoder ladder
  accordingly.
* No SDK shape change. TS + Python clients consume HLS / DASH
  directly via hls.js / dash.js / Shaka; no new admin route, no new
  SDK type.

## Metrics

| Metric | Type | Labels | Meaning |
|--------|------|--------|---------|
| `lvqr_scte35_events_total` | counter | `ingest`, `command` (hex) | Sections successfully parsed and emitted onto the scte35 track. |
| `lvqr_scte35_drops_total` | counter | `ingest`, `reason` (`crc` / `malformed` / `truncated` / `other`) | Sections dropped at the parser boundary. |
| `lvqr_scte35_bridge_drops_total` | counter | `broadcast`, `reason` (`parse`) | Sections that reached the cli-side bridge but failed parse on the second pass. |

## Publisher quickstart

How to actually emit SCTE-35 from the common publisher tools. The
LVQR side accepts whatever the encoder emits as long as the wire
shape matches the convention documented in the ingest-paths table
above; nothing in the relay configuration needs to change to enable
the feature.

### ffmpeg over SRT

ffmpeg's MPEG-TS muxer carries SCTE-35 sections on a dedicated PID
when the input source produces them. The simplest path is to stream
a TS file that already contains a SCTE-35 PID (e.g. one generated
by an upstream encoder) straight into LVQR via SRT:

```bash
ffmpeg -re -i source-with-scte35.ts \
  -c copy \
  -f mpegts srt://relay.example.com:8890?streamid=publish:live/cam1
```

To inject SCTE-35 sections programmatically, use ffmpeg's
`-bsf:v scte35_data` bitstream filter (contributed in FFmpeg 6.0+)
or a patched ffmpeg build with the `--enable-libklscte35`
configuration. Verify the output PMT has stream_type 0x86 with
`ffprobe -hide_banner -show_streams source.ts | grep -i scte`.

### ffmpeg over RTMP (onCuePoint)

ffmpeg does not natively emit AMF0 onCuePoint scte35-bin64 data
messages on its RTMP output. Use a wrapper such as
[`rtmpdump`-style scriptdata injection](https://wiki.multimedia.cx/index.php/RTMP)
or a custom muxer. Pipelines using
[`klscte35`](https://github.com/LTNGlobal-opensource/libklscte35) +
the `nginx-rtmp-module` `oncuepoint` directive are the most common
in-the-wild route.

### AWS Elemental MediaLive / MediaConnect

* MediaLive: enable "SCTE-35 ad-marker passthrough" on the MPEG-TS
  output group. The encoder muxes SCTE-35 on the configured PID
  (default 0x1FFB). Send to LVQR via SRT.
* MediaConnect: SCTE-35 is preserved when the entitlement specifies
  `output: SRT` with `Source: SRT-listener`. No additional
  configuration on the LVQR side.

### Wirecast + vMix

* Wirecast: open the encoder presets, enable "SCTE-35 in-band
  metadata" on the broadcast output. RTMP target invokes the
  patched onCuePoint path; SRT target uses PID 0x1FFB.
* vMix: the "SCTE-35 Trigger" plugin emits onCuePoint for RTMP and
  PID-carried sections for SRT. Default convention is the
  scte35-bin64 / 0x1FFB pair LVQR expects.

### OBS Studio

OBS does not natively emit SCTE-35. Use the
[`obs-scte35` script](https://github.com/scte-35/obs-scte35) which
hooks the `onCuePoint` AMF0 output during a live RTMP push.
Operators who only need passthrough from a downstream source
(e.g. a hardware encoder feeding OBS) typically bypass OBS for the
SCTE-35 path and let the encoder drive it directly.

### Verification (after wiring)

```bash
# HLS: a DATERANGE line should appear at the playlist head
# during a known ad break.
curl -s https://relay/hls/<broadcast>/video/playlist.m3u8 \
    | grep -E "EXT-X-DATERANGE|SCTE35"

# DASH: an EventStream element should appear at the Period level.
curl -s https://relay/dash/<broadcast>/manifest.mpd \
    | xmllint --xpath "//*[local-name()='EventStream']" -

# Metrics: counters increment per ingest path.
curl -s https://relay/metrics | grep scte35
```

## Client-side consumption

How the rendered events look from a player or ad-decisioning
client. LVQR ships no client-side SCTE-35 SDK -- the wire shape is
standards-compliant, so any HLS or DASH library that exposes
DATERANGE / EventStream events to JavaScript works.

### `@lvqr/dvr-player` web component (turn-key)

For operators who want a drop-in HLS DVR player that renders
markers on its seek bar without writing any JavaScript, the
[`@lvqr/dvr-player`](../bindings/js/packages/dvr-player) web
component (since v0.3.3) parses the playlist's DATERANGE
entries, paints OUT/IN pair spans + CMD ticks on its custom
seek bar, and emits `lvqr-dvr-markers-changed` /
`lvqr-dvr-marker-crossed` events for downstream pipelines.

```html
<script type="module">
  import '@lvqr/dvr-player';
</script>
<lvqr-dvr-player
  src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
  autoplay
  muted
></lvqr-dvr-player>
<script type="module">
  document.querySelector('lvqr-dvr-player').addEventListener(
    'lvqr-dvr-marker-crossed',
    (e) => console.log('crossed', e.detail.marker.kind, e.detail.marker.id),
  );
</script>
```

See [`docs/dvr-scrub.md`](dvr-scrub.md#scte-35-ad-break-markers)
for the full marker recipe (attribute toggles, theming hooks,
edge cases, programmatic API). The DIY hls.js recipe below
remains the right path for integrators who already ship their
own player chrome and just want raw access to the SCTE-35
events.

### hls.js (HLS DATERANGE)

hls.js exposes DATERANGE entries via the
`Hls.Events.LEVEL_UPDATED` payload's `levels[].details.dateRanges`
collection and individual fire events on
`Hls.Events.DATE_RANGE_UPDATED`. SCTE-35 markers carry the
`CLASS="urn:scte:scte35:2014:bin"` attribute so a single filter
distinguishes them from other DATERANGE uses (program boundaries,
chapter markers, etc.).

```javascript
import Hls from 'hls.js';

const video = document.querySelector('video');
const hls = new Hls();
hls.loadSource('https://relay.example.com/hls/live/cam1/playlist.m3u8');
hls.attachMedia(video);

hls.on(Hls.Events.DATE_RANGE_UPDATED, (_event, data) => {
  for (const dr of data.dateRanges) {
    if (dr.attr['CLASS'] !== 'urn:scte:scte35:2014:bin') continue;
    // SCTE35-OUT (going to ad), SCTE35-IN (returning), SCTE35-CMD (other).
    const out = dr.attr['SCTE35-OUT'];
    const in_ = dr.attr['SCTE35-IN'];
    const cmd = dr.attr['SCTE35-CMD'];
    const hex = out || in_ || cmd;
    if (!hex) continue;
    // hex is "0x..." -- decode for downstream ad decisioning.
    const sliceInfoSection = hexToBytes(hex.slice(2));
    onSpliceEvent({
      id: dr.id,
      startDate: new Date(dr.attr['START-DATE']),
      durationSecs: parseFloat(dr.attr['DURATION'] || '0'),
      kind: out ? 'out' : in_ ? 'in' : 'cmd',
      raw: sliceInfoSection,
    });
  }
});

function hexToBytes(hex) {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(hex.substr(i * 2, 2), 16);
  }
  return out;
}
```

### dash.js (DASH EventStream)

dash.js exposes Period-level EventStream events via the
`MediaPlayer.events.EVENT_MODE_ON_RECEIVE` and
`MediaPlayer.events.EVENT_MODE_ON_START` callbacks. The Signal /
Binary body is exposed as the event message data. Subscribe to the
SCTE-35 scheme by URI:

```javascript
import { MediaPlayer } from 'dashjs';

const player = MediaPlayer().create();
player.initialize(
  document.querySelector('video'),
  'https://relay.example.com/dash/live/cam1/manifest.mpd',
  true,
);

const SCTE35_SCHEME = 'urn:scte:scte35:2014:xml+bin';
player.on(
  MediaPlayer.events.EVENT_MODE_ON_RECEIVE,
  (event) => {
    const e = event.event;
    // e.id: splice_event_id (or 0 for non-splice_insert command types)
    // e.presentationTime: 90 kHz absolute splice PTS
    // e.duration: 90 kHz break_duration (0 when undefined)
    // e.messageData: parsed Signal/Binary body
    onSpliceEvent({
      id: e.id,
      ptsTicks: e.presentationTime,
      durationTicks: e.duration,
      raw: e.messageData,  // Uint8Array of base64-decoded splice_info_section
    });
  },
  { schemeIdUri: SCTE35_SCHEME },
);
```

### Shaka Player (DASH EventStream)

Shaka exposes EventStream events via the
`shaka.Player.EventManager` `'metadata'` listener:

```javascript
import shaka from 'shaka-player';

const player = new shaka.Player();
await player.attach(document.querySelector('video'));
await player.load('https://relay.example.com/dash/live/cam1/manifest.mpd');

player.addEventListener('metadata', (event) => {
  if (event.payload?.schemeIdUri !== 'urn:scte:scte35:2014:xml+bin') return;
  onSpliceEvent({
    id: event.payload.id,
    startTime: event.startTime,
    endTime: event.endTime,
    raw: event.payload.value,  // raw splice_info_section bytes
  });
});
```

### Native HLS players (Safari, AVPlayer)

Native players expose DATERANGE entries via the
`AVMetadataItem` API (iOS / macOS) and the standard
`textTracks` collection (browsers without hls.js). The SCTE-35
attributes flow through unchanged; the client decides whether to
act on them.

```javascript
// Browsers: poll the metadata text track.
video.textTracks.addEventListener('change', () => {
  for (const track of video.textTracks) {
    if (track.kind !== 'metadata') continue;
    track.mode = 'hidden';
    track.addEventListener('cuechange', () => {
      for (const cue of track.activeCues) {
        // cue.value carries the DATERANGE attributes for hls.js
        // shim adapters; for raw native HLS the cue is an
        // ID3-style payload that may need different parsing.
      }
    });
  }
});
```

### Decoding the splice_info_section client-side

The egress wire shapes preserve the raw section bytes verbatim:
* HLS SCTE35-* attributes are `0x` + hex.
* DASH `<Binary>` elements are RFC 4648 base64.

Both decode to a SCTE 35-2024 section 8.1 `splice_info_section`
that downstream ad-decisioning libraries (Google IMA, AWS MediaTailor
SDK, Bitmovin Ad Engine) consume directly. LVQR does not interpret
the section beyond what the playlist render needs; semantic decoding
is the client's responsibility.

## Operator runbook

* Confirm the publisher's SCTE-35 PID. Most broadcast encoders
  default to PID 0x1FFB; some (Elemental, Synamedia) make it
  configurable. Verify the PMT stream_type is 0x86; LVQR does not
  pick up SCTE-35 carried under non-standard stream_type values.
* Check `lvqr_scte35_events_total` is incrementing during a known
  ad break. A flat counter when the publisher is sending events
  usually means PID misconfiguration on the publisher or a CRC
  issue (compare against `lvqr_scte35_drops_total`).
* Verify the rendered HLS playlist has `#EXT-X-DATERANGE` lines
  matching the publisher's events. `curl
  https://relay/hls/<broadcast>/video/playlist.m3u8 | grep
  DATERANGE` is the quickest check.
* For DASH, fetch `https://relay/dash/<broadcast>/manifest.mpd`
  and look for `<EventStream schemeIdUri="urn:scte:scte35:2014:
  xml+bin">`. The Event count grows as breaks land.
