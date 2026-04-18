# lvqr-ingest

RTMP ingest for LVQR.

Accepts RTMP connections via [rml_rtmp](https://crates.io/crates/rml_rtmp),
parses FLV video + audio tags into `lvqr_cmaf::RawSample`
values, and emits them through the shared
`FragmentBroadcasterRegistry` as `lvqr_fragment::Fragment`
objects. The `RtmpMoqBridge` installs the MoQ producer side;
every other egress (HLS, DASH, WHEP, WebSocket fMP4, archive)
subscribes to the same fragment stream via
`FragmentObserver` and `RawSampleObserver` taps.

The other ingest protocols in the v0.4 surface live in
their own crates:

- `lvqr-whip` -- WebRTC ingest (H.264 + HEVC + Opus via str0m)
- `lvqr-srt` -- SRT-over-UDP + MPEG-TS demuxer
- `lvqr-rtsp` -- RTSP/1.0 server + interleaved RTP depacketizer

This split follows the roadmap's unified-fragment-model
discipline: each ingest produces fragments; the egress side
never knows which wire protocol they came from.

## Features

- `rtmp` (default) -- RTMP server via `rml_rtmp`.

## Usage

```rust
use lvqr_ingest::{RtmpConfig, RtmpMoqBridge};

let bridge = RtmpMoqBridge::new(origin.clone(), registry.clone());
let rtmp_server = bridge.create_rtmp_server(
    RtmpConfig::new("0.0.0.0:1935".parse()?)
);
rtmp_server.run().await?;
```

`lvqr-cli::start` handles the wiring; embedders should prefer
constructing a `ServeConfig` and calling `lvqr_cli::start`
unless they need a customised bridge shape.

## 5-artifact test coverage

Every parser in this crate carries all five test artifacts
(proptest, fuzz, integration, E2E, conformance). See
[`../../tests/CONTRACT.md`](../../tests/CONTRACT.md) for the
full scorecard.

## License

AGPL-3.0-or-later for open-source use; commercial license
available for proprietary / SaaS deployments. See the top-
level [`COMMERCIAL-LICENSE.md`](../../COMMERCIAL-LICENSE.md)
for the process.
