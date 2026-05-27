# `lvqr` Python SDK Changelog

User-visible changes to the LVQR Python SDK published on PyPI.
The head of `main` is always the source of truth; this file
summarises shipped + unreleased work between PyPI releases. For
session-by-session engineering notes see
[`tracking/HANDOFF.md`](../../tracking/HANDOFF.md).

## [1.1.0] - 2026-05-26

Catches the Python SDK up to the v1.1.0 admin surface that shipped to
crates.io / npm a few hours earlier (the console buildout wave: slices
1-9b). Every new admin endpoint now has a typed `LvqrClient` method and
every new server-side serde struct has a matching `@dataclass` mirror.
Pure-additive over 1.0.0; no existing method signature changed.

### Added -- read-only introspection

* `LvqrClient.stream_detail(name)` -- `GET /api/v1/streams/{name}`,
  returns `Optional[StreamDetailInfo]` (404 -> `None`).
* `LvqrClient.transcode_ladders()` -> `TranscodeState` (configured
  renditions + live per-input stats).
* `LvqrClient.agents()` -> `AgentState` (configured agents + per-
  attachment counters).
* `LvqrClient.archive()` -> `ArchiveState` (recorded broadcasts from
  the DVR segment index).
* `LvqrClient.ingest_listeners()` -> `IngestState` (per-protocol
  listener inventory + live `enabled` state).
* `LvqrClient.broadcasts()` -> `BroadcastSessionsState` (live publisher
  sessions across every ingest protocol whose ingest crate wired its
  session registrar).
* `LvqrClient.server_info()` -> `ServerInfo` (version + uptime + bound
  addresses + runtime feature snapshot).
* `LvqrClient.logs_stream_url()` -> `str` (EventSource-ready URL with
  `?token=` query-param auth for the SSE log tail).

### Added -- runtime CRUD

* `LvqrClient.add_rendition(req: AddRenditionRequest)` --
  `POST /api/v1/transcode/ladders`.
* `LvqrClient.remove_rendition(name)` --
  `DELETE /api/v1/transcode/ladders/{name}`.
* `LvqrClient.add_agent(req: AddAgentRequest)` --
  `POST /api/v1/agents`.
* `LvqrClient.remove_agent(name)` --
  `DELETE /api/v1/agents/{name}`.

### Added -- runtime STOP (slice 9b) + per-publisher KICK (slice 6)

* `LvqrClient.stop_ingest_listener(protocol)` -> `IngestStopResult` --
  `DELETE /api/v1/ingest/{protocol}`. Idempotent.
* `LvqrClient.stop_broadcast(name)` -> `BroadcastStopResult` --
  `DELETE /api/v1/broadcasts/{name}`. Truly disconnects the live
  publisher (TCP/UDP socket closed; subscribers see EOS).

### Added -- types (23 new dataclasses)

`TrackInfo`, `StreamDetailInfo`, `RenditionInfo`, `TranscodeActiveStats`,
`TranscodeState`, `AddRenditionRequest`, `AgentInfo`, `AgentActiveStats`,
`AgentState`, `AddAgentRequest`, `ArchiveTrackInfo`,
`ArchiveBroadcastInfo`, `ArchiveState`, `IngestListenerInfo`,
`IngestState`, `IngestStopResult`, `BroadcastSessionInfo`,
`BroadcastSessionsState`, `BroadcastStopResult`, `BoundAddresses`,
`RuntimeFeatures`, `ServerInfo`, `LogLine`.

### Tests

39 -> 58 passing. New `TestV1_1_0_Types` covers dataclass defaults; new
`TestV1_1_0_Methods` mocks `httpx` and exercises every new method (incl.
the 404-as-`None` semantics for `stream_detail` and the URL-token
encoding for `logs_stream_url`).

## [1.0.0] - 2026-04-28

Stability commitment for the Python admin client. Same source as
0.3.3; the version label moves with the rest of the SDK family
(`@lvqr/core`, `@lvqr/dvr-player`, `@lvqr/player`,
`@lvqr/admin-ui`) and the Rust workspace. No API change; every
method on `LvqrClient` keeps its existing signature.

## [0.3.3] - 2026-04-28

Cross-language SDK release wave alongside `@lvqr/core` 0.3.3
and the first npm publish of `@lvqr/dvr-player` 0.3.3. Brings
the Python admin client in line with the v0.4.2 server surface
(runtime stream-key CRUD + hot config reload).

### Added

* **`LvqrClient.config_reload_status()` /
  `trigger_config_reload()` + `ConfigReloadStatus` dataclass**
  (session 147). Defensive `.get(...)` parsers carry forward
  across sessions 148 / 149's `applied_keys` extension without
  a SDK change; the wire shape stays a simple
  `list[str]` / `list[str]` / etc.

* **`LvqrClient.list_streamkeys()` / `mint_streamkey()` /
  `revoke_streamkey()` / `rotate_streamkey()` + `StreamKey` /
  `StreamKeySpec` dataclasses** (session 146). Mint, list,
  revoke, and rotate ingest stream keys from Python. See
  `docs/sdk/python.md`.

## [0.3.2] - 2026-04-24

Tracker-only republish alongside `@lvqr/core` 0.3.2 + Rust
0.4.1. No SDK shape changes; sessions 141-144 source on `main`
is the published artifact.
