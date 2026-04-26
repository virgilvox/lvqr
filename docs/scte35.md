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
| RTMP onCuePoint | deferred | rml_rtmp v0.8 silently drops AMF0 Data messages other than `@setDataFrame`-wrapped onMetaData. Lifting requires either an upstream patch or replacing the rml_rtmp dep. Tracking: session 152 close block. |
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
