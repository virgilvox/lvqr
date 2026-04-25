# Session 143 Briefing -- TURN deployment recipe + server-driven ICE config

**Date kick-off**: 2026-04-24
**Phase D row**: README `Peer mesh data plane` item "TURN deployment
recipe + STUN fallback config. Document coturn integration for peers
behind symmetric NAT."
**Predecessor**: Session 142 (three-peer Playwright matrix). Follows
the mesh-data-plane row 3 of 4.

## Goal

Make TURN/STUN configuration a single operator knob that flows from
the server down to every browser peer automatically. Today,
`MeshPeer` accepts an `iceServers: RTCIceServer[]` constructor
argument; integrators have to thread the JSON through their own
code, and a single mistake produces silent ICE failures behind
symmetric NAT. After this session: `lvqr serve --mesh-ice-servers
'[...]'` configures the list once on the server, and every browser
peer registering on `/signal` receives it via a new field on the
existing `AssignParent` server-push message. Plus the operator-
facing artifacts (a real coturn deployment recipe + sample config)
under `deploy/turn/`.

## Wire shape

### New `IceServer` type in `lvqr-signal`

Mirrors WebRTC's `RTCIceServer` JSON shape so JS clients can pass
the value straight into `new RTCPeerConnection({ iceServers: [...] })`.

```rust
// crates/lvqr-signal/src/signaling.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}
```

Single `urls: Vec<String>` (not `OneOrMany`): operators always pass
an array in JSON. Less ergonomic for "one URL" but the parser
complexity drop is worth the trade. Each entry is a stun: or
turn:/turns: URL per RFC 7064 / 7065.

### Extended `SignalMessage::AssignParent`

```rust
AssignParent {
    peer_id: String,
    role: String,
    parent_id: Option<String>,
    depth: u32,
    #[serde(default)]
    ice_servers: Vec<IceServer>,  // NEW
}
```

`#[serde(default)]` on the new field so:
* Pre-143 servers emitting AssignParent without the field still
  deserialize cleanly into a new client (Vec defaults to empty).
* Pre-143 clients that strictly parse known fields only ignore the
  new key (JS's `JSON.parse` is lenient; Rust serde with
  `#[serde(deny_unknown_fields)]` is not used on this enum).

### CLI flag

```
--mesh-ice-servers <JSON>     (env LVQR_MESH_ICE_SERVERS)
```

JSON array of `IceServer` objects. Single flag, one JSON blob,
parsed once at boot. If unset, defaults to empty vec -- the server
emits `ice_servers: []` and clients fall back to whatever
`MeshPeer` was constructed with (or the hardcoded Google STUN
default in `MeshPeer.iceConfig`).

Example:
```
--mesh-ice-servers '[
  {"urls":["stun:stun.l.google.com:19302"]},
  {"urls":["turn:turn.example.com:3478"],"username":"u","credential":"p"}
]'
```

Parse failure: lvqr exits at boot with a clear message naming the
offending JSON path.

### Client-side (JS `MeshPeer`)

`handleAssignment` reads `msg.ice_servers`. If non-empty, rebuilds
`this.iceConfig` from the server-provided list, replacing whatever
was constructor-provided. If empty, no change -- the constructor
list (or hardcoded default) stays. **Server is authoritative when
configured; otherwise client decides.**

Important: rebuilding `iceConfig` only affects future
`RTCPeerConnection` instances (parent-side and child-side). On
late updates -- if the server re-emits AssignParent with a
different list mid-session -- existing PCs keep their stale config.
For v1 this is acceptable (operators set the list at boot, never
change it); a session-N follow-up could close existing PCs on the
list-changed case.

## Operator-facing artifacts

### `deploy/turn/coturn.conf` (sample)

A minimal but production-shaped coturn config. ~30-40 lines with
inline comments:

* listening-port = 3478
* fingerprint, lt-cred-mech, realm = lvqr.local
* user = lvqr-mesh:CHANGE_ME
* min-port / max-port for relay
* no-tcp / no-tls (TLS variant is operator-deployment-specific)
* total-quota / user-quota / max-bps as commented placeholders

### `deploy/turn/README.md`

Operator recipe. Sections:
* What problem this solves (symmetric NAT keeps DataChannel
  candidates from connecting; TURN relays around it).
* coturn install on Debian-family + alpine.
* The minimal coturn.conf that ships in this directory.
* How to wire the lvqr `--mesh-ice-servers` flag to the running
  coturn server.
* Sanity check: `turnutils_uclient` against the running coturn.
* Cost shape: TURN traffic flows through the server's NIC; not a
  cost for STUN-only deployments.

### `docs/mesh.md` TURN section

Inline section in the existing mesh.md, after the bandwidth-offload
table. ~50 lines. Cross-links to `deploy/turn/`.

## Testing strategy

* **Unit (lvqr-signal)**: round-trip serde for `IceServer` (one URL
  with credentials, multiple URLs, no credentials) and for
  AssignParent with a populated `ice_servers` list.
* **Unit (lvqr-signal)**: `AssignParent` deserialize from a pre-143
  body that omits `ice_servers` returns an empty vec via the
  `#[serde(default)]` fallback.
* **Unit (lvqr-cli)**: CLI parse tests for `--mesh-ice-servers`:
  unset = empty vec, valid JSON parses, invalid JSON errors with a
  helpful message.
* **Integration (lvqr-cli/tests/mesh_ice_servers.rs)**: spin up
  `lvqr serve --mesh-enabled --mesh-ice-servers '[...]'`, open a
  WebSocket client to `/signal`, send `Register`, read the pushed
  `AssignParent` message, parse the JSON, and assert
  `ice_servers` matches the configured list. This is the locally-
  verifiable end-to-end check.
* No JS test in this session: the existing `two-peer-relay.spec.ts`
  + `three-peer-chain.spec.ts` already exercise the path with
  `ice_servers: []` (the playwright webServer does not configure
  `--mesh-ice-servers`). The non-empty path is covered by the Rust
  integration test; full Playwright coverage of the iceServer
  reconfiguration path requires a running TURN server which the
  CI harness does not have.

## Anti-scope

* **No mid-session ice_servers updates.** Server emits the list once
  on AssignParent; clients build their PCs from that snapshot. If
  an operator changes the list and restarts lvqr, in-flight peers
  keep their stale config until they reconnect.
* **No client-side credential refresh** (turn-rest-api / shortlived
  TURN credentials). Static long-term creds only in v1; rotation
  recipe is a v1.2 candidate.
* **No turn:turns:// URL validation in the CLI parser.** Operators
  can pass arbitrary URLs; coturn or the browser surfaces failures.
  Keeps the parser simple and forward-compat with future ICE
  schemes.
* **No `iceTransportPolicy: 'relay'` enforcement** (force-TURN
  mode). MeshPeer continues to use the default `'all'`.
* **No actual coturn boot in CI.** The deploy/turn/ recipe is
  documented but unverified in this session; verification is an
  operator runbook entry.
* **No JS unit test of the rebuild-iceConfig path.** Adding it would
  require a non-trivial WebSocket mock; the integration coverage at
  the Rust layer + the existing Playwright happy-path coverage are
  enough.

## Deliverables checklist

- [ ] `tracking/SESSION_143_BRIEFING.md` (this file)
- [ ] `crates/lvqr-signal/src/signaling.rs` -- new `IceServer` type +
      `AssignParent.ice_servers` field + 3-4 unit tests
- [ ] Update existing `AssignParent` constructions in
      `lvqr-signal/src/signaling.rs` (test sites at lines 491, 551,
      581) to include `ice_servers: Vec::new()`
- [ ] `crates/lvqr-cli/src/lib.rs` -- `ServeConfig.mesh_ice_servers:
      Vec<IceServer>` + plumb into the signal callback so
      AssignParent carries the configured list
- [ ] `crates/lvqr-cli/src/main.rs` -- `--mesh-ice-servers <JSON>`
      flag + env fallback + parse-error path + 3 CLI tests
- [ ] `crates/lvqr-cli/tests/mesh_ice_servers.rs` -- new integration
      test booting lvqr with the flag, opening a WS client,
      asserting the AssignParent body
- [ ] `bindings/js/packages/core/src/mesh.ts` -- `handleAssignment`
      reads `msg.ice_servers`; rebuilds `iceConfig` if non-empty
- [ ] `deploy/turn/coturn.conf` (sample)
- [ ] `deploy/turn/README.md` (operator recipe)
- [ ] `docs/mesh.md` -- new TURN section after bandwidth-offload
      table + phase-D scope subsection trims this row out
- [ ] `README.md` -- flip the TURN deployment-recipe bullet from
      `[ ]` to `[x]`
- [ ] CI gates clean: fmt, clippy, cargo test --workspace, pytest
      (unaffected), tsc on @lvqr/core
- [ ] Live smoke: boot lvqr with `--mesh-ice-servers`, curl-WS or
      similar, observe AssignParent body
- [ ] Session 143 close block on `HANDOFF.md`
- [ ] `MEMORY.md` status line updated
- [ ] Two commits (feat + docs close); push
