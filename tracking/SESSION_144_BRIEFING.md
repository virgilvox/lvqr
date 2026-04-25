# Session 144 Briefing -- Per-peer capacity advertisement

**Date kick-off**: 2026-04-24
**Phase D row**: README `Peer mesh data plane` item "Per-peer capacity
advertisement so rebalancing uses bandwidth + CPU instead of hardcoded
`max-children`" (README.md line 450-451).
**Predecessor**: Session 143 (TURN deployment recipe + server-driven
ICE config). Closes the LAST unshipped row of the four mesh-data-plane
bullets; with this row landed, `docs/mesh.md` flips from "topology
planner + signaling + ..." to "IMPLEMENTED" and the entire mesh-data-
plane README checklist closes.

## Goal

The current planner uses a single global `MeshConfig.max_children`
(set via `--max-peers <N>`) as the per-peer cap for every node. That
forces operators to pick one number that is correct for every viewer,
which is wrong: a low-bandwidth mobile peer should not be a parent at
all; a wired-laptop peer can probably serve 5+. After this session the
client self-reports a static capacity at registration time, the server
clamps it to the operator-configured ceiling, and `find_best_parent`
respects the per-peer value -- so a misbehaving viewer cannot exceed
the operator's ceiling, and an honest viewer can opt down without the
operator having to lower the ceiling for everyone.

## Decisions locked

### 1. What "capacity" means in v1

**Static, client-self-reported, integer "I can serve up to N
children".** No bandwidth probing, no CPU-headroom heuristic.

The web platform does not give us either knob honestly:
* `navigator.connection.downlink` is downlink-only and inconsistent.
* No portable upload-bandwidth API.
* No portable CPU-headroom API.
* ICE bandwidth probing is heavyweight and changes the wire shape.

The integrator chooses the value in `MeshConfig.capacity` (e.g. their
app picks 0 for a known-mobile profile, 5 for a known-laptop profile,
omits the field for "let the server decide"). Empirical capacity
discovery is anti-scope; v1.2 candidate.

The README phrase "bandwidth + CPU" is aspirational, not a v1
contract. The briefing names the v1 shape explicitly so docs can
match.

### 2. Wire shape: extend Register, not new `Capacity` variant

Capacity is part of the registration contract: the server must know
it AT THE MOMENT of computing the assignment, not in a follow-up
message. A separate `Capacity` variant would either (a) force a
round-trip delay on first AssignParent or (b) require an immediate
reassign-on-Capacity, churning the tree on every join. Extending
Register keeps the bootstrap path one round-trip wide.

```rust
SignalMessage::Register {
    peer_id: String,
    track: String,
    #[serde(default)]
    capacity: Option<u32>,  // NEW
}
```

`#[serde(default)]` on the new field, so:
* Pre-144 clients omitting the field still parse (defaults to `None`).
* New clients sending `capacity` to a pre-144 server are silently
  ignored (`SignalMessage` is not `#[serde(deny_unknown_fields)]`).

Mid-session capacity revisions (tab switch, network change) are
anti-scope. If/when needed, add a separate `Capacity { capacity: u32 }`
variant alongside Register-extension; both can coexist.

### 3. Coordinator integration

`PeerInfo` (in `crates/lvqr-mesh/src/tree.rs`) grows:

```rust
/// Self-reported relay capacity (max children this peer is willing
/// to serve). None = use MeshConfig.max_children. Server clamps the
/// client claim to the operator-configured ceiling at register time
/// so on-disk values are always in [0, max_children]. Session 144.
#[serde(default)]
pub capacity: Option<u32>,
```

`PeerInfo::can_accept_child` consults the field:

```rust
pub fn can_accept_child(&self, default_max: usize) -> bool {
    self.children.len() < self.effective_capacity(default_max)
}

pub fn effective_capacity(&self, default_max: usize) -> usize {
    self.capacity
        .map(|c| (c as usize).min(default_max))
        .unwrap_or(default_max)
}
```

`MeshCoordinator::find_best_parent` body is unchanged -- the existing
`peer.can_accept_child(self.config.max_children)` call now consults
per-peer capacity automatically.

`MeshCoordinator::add_peer` signature grows the new argument:

```rust
pub fn add_peer(
    &self,
    id: PeerId,
    track: String,
    capacity: Option<u32>,
) -> Result<PeerAssignment, MeshError>
```

Two callers in-tree:
* `lvqr-cli` ws_relay_session path (server-generated `ws-{counter}`
  peer_id) -- passes `None`; /ws subscribers do not advertise
  capacity in v1.
* `lvqr-cli` /signal register-callback bridge -- passes the
  `PeerEvent.capacity` value through.

### 4. `PeerCallback` shape: switch to `PeerEvent` struct

Capacity must reach the on_peer callback so the lvqr-cli bridge can
call `MeshCoordinator::add_peer(peer_id, track, capacity)`. The
current shape `Fn(&str, &str, bool) -> Option<SignalMessage>` cannot
carry a fourth argument cleanly: a positional extension introduces a
`None` at disconnect time that reads as noise (capacity is not
meaningful on disconnect).

Adopt the named-field-struct pattern that landed in 143 for
`IceServer`:

```rust
pub struct PeerEvent<'a> {
    pub peer_id: &'a str,
    pub track: &'a str,
    pub capacity: Option<u32>,
    pub connected: bool,
}
pub type PeerCallback =
    Arc<dyn Fn(&PeerEvent<'_>) -> Option<SignalMessage> + Send + Sync>;
```

Two production call sites: `register_peer` (connected=true) and
`remove_peer` (connected=false). About 5 test sites in
`signaling.rs::tests` update mechanically.

### 5. Clamp at register time, not at consult time

When the /signal `Register` arrives, the lvqr-cli bridge clamps the
client claim to `[0, MeshConfig.max_children]` BEFORE storing into
`PeerInfo.capacity`. The clamped value is what `/api/v1/mesh` returns.

Alternative considered: store-as-claimed, clamp at find_best_parent
consult time. Cheaper to implement, but exposes raw client claims on
the admin route -- a misbehaving client claiming `u32::MAX` would
appear on the operator's dashboard with a wildly inflated capacity.
Clamp-at-ingest preserves the invariant that `PeerInfo.capacity` is
always within the operator's ceiling.

### 6. Admin route exposure

`MeshPeerStats` (in `crates/lvqr-admin/src/routes.rs`) grows:

```rust
#[serde(default)]
pub capacity: Option<u32>,
```

Operators see `intended_children` (planner-assigned), `forwarded_frames`
(session 141), and now `capacity` (session 144) on the same per-peer
row. Lets dashboards spot under-utilized high-capacity peers (claim=5,
intended_children=1) and detect tree shapes constrained by mobile
caps.

### 7. JS API

`MeshConfig` (`bindings/js/packages/core/src/mesh.ts`) grows:

```typescript
/** Self-reported relay capacity (max children this peer can serve).
 *  Server clamps to its configured global max-peers. Omit for the
 *  server default. */
capacity?: number;
```

`connect()` includes the field on the Register payload.
`JSON.stringify` skips undefined fields, so omitting the config
produces a Register without capacity (server uses its default).

`@lvqr/core` admin types: `MeshPeerStats.capacity?: number` mirrors
the Rust struct.

No new JS getter (`MeshPeer.capacity`) -- the integrator passes the
value in once at construct time and does not need to read it back.
Adding a getter is mechanical follow-up work if operators ask.

### 8. Python types + admin client

`bindings/python/python/lvqr/types.py`: `MeshPeerStats.capacity:
Optional[int] = None`. `client.mesh()` uses `.get("capacity")` to
tolerate pre-144 servers (mirrors the `.get("peers", [])` pattern
session 141 used).

### 9. CLI: no new flag

The existing `--max-peers <N>` is the operator's global ceiling. No
new flag; capacity is a per-peer client-side concept, not an operator
knob. (A "default capacity for unset clients" knob would carry the
same value as `--max-peers`; one number is enough.)

## Wire-shape compat matrix

| client \ server          | new server (144)             | pre-144 server                  |
|---|---|---|
| pre-144 client (no field) | server: capacity = None -> uses global max | works (always did)            |
| new client sends capacity | server clamps + stores       | server ignores extra field; uses global max |

Both directions work without a schema-version bump.

## Testing strategy

* **Unit (lvqr-mesh::tree)**: `peer_info_defaults` extended with
  `capacity is None`. New `effective_capacity_uses_global_when_unset`,
  `effective_capacity_clamps_oversize_claim`, and
  `can_accept_child_respects_capacity_zero` (capacity=0 returns false
  even with default_max=10).
* **Unit (lvqr-mesh::coordinator)**: New
  `find_best_parent_respects_per_peer_capacity` -- 3 peers with
  root_peer_count=1, max_children=5, capacities (1, 5, 5). After
  peer-1 (Root) and peer-2 (Relay child of peer-1) are added, peer-3
  must descend to peer-2 because peer-1 hit its capacity=1.
* **Unit (lvqr-signal::signaling)**: `Register` round-trips with
  `capacity: Some(3)`. Pre-144-shape Register (no capacity field)
  deserializes with `capacity: None`. New `PeerEvent` struct fields
  set correctly on register + unregister callbacks (extends the
  existing `peer_callback_on_register` test).
* **Unit (lvqr-admin::routes)**: `MeshPeerStats` round-trips capacity.
  Pre-144 admin body without capacity deserializes via
  `#[serde(default)]`.
* **Integration (`crates/lvqr-cli/tests/mesh_capacity_e2e.rs`, new)**:
  Boot lvqr with `--max-peers 5 --mesh-root-peer-count 1`. Open 3
  tokio-tungstenite WS clients to /signal, send Register with
  capacities (1, undefined, undefined). Poll `/api/v1/mesh` and
  assert: peer-1 Root with capacity=Some(1), peer-2 Relay with
  parent=peer-1, peer-3 Relay with parent=peer-2 (forced descent
  because peer-1 was at capacity).
* **Integration**: companion test
  `register_with_oversize_capacity_is_clamped` -- send Register with
  capacity=u32::MAX, assert `/api/v1/mesh` reports capacity=Some(5)
  (the global max-peers).
* **JS Vitest**: existing `mesh returns a MeshState shape` test grows
  a `peer.capacity === undefined` assertion (no live peer harness
  pushes a non-default capacity, but the field shape is exercised).
* **Python pytest**: `test_mesh_peer_stats_defaults` extended with
  `capacity is None`. New
  `test_mesh_pre_session_144_server_omits_capacity` proves the
  defensive parse via `.get("capacity")`.
* **Existing Playwright `three-peer-chain.spec.ts`**: must continue
  passing without advertising capacity -- regression guard for the
  pre-144-client path against a new server.

## Anti-scope

* **No mid-session capacity updates.** Register-time only. Future
  session can add a `SignalMessage::Capacity` variant if operators
  ask.
* **No browser-bandwidth measurement.** No reliable cross-browser API
  for upload-bandwidth.
* **No CPU-headroom measurement.** No portable browser API.
* **No `PeerRole::Leaf` transition for capacity=0.** Role stays
  Relay; capacity=0 just makes the peer ineligible in
  find_best_parent. Renaming the role would invite cross-cutting
  serialization-compat work that buys nothing.
* **No rebalance of existing tree on capacity changes.** Capacity is
  captured in PeerInfo at register time; future peer joins consider
  it; current children stay assigned to their current parents.
* **No CLI flag for default capacity.** `--max-peers` already serves
  as both the global ceiling and the default-when-unset.
* **No Prometheus per-peer capacity metric.** Operator question is
  "what did peer X claim?" -- a JSON read on the admin route is the
  answer; high-cardinality peer_id labels would not help dashboards.
* **No Playwright capacity test.** Three-peer-chain regression run
  covers the no-capacity path; the Rust integration test covers the
  with-capacity path. A 4+ peer mixed-capacity Playwright test adds
  proportional flake risk for incremental signal.
* **No JS `MeshPeer.capacity` getter.** Integrator-side concern only;
  not needed today.
* **No /ws-subscriber capacity advertisement.** /ws subscribers
  register through `add_peer(.., None)`; if operators ask for
  capacity on /ws-side subscribers, that is its own session.

## Implementation order

1. Briefing (this file). Pause for user review of the locked
   decisions BEFORE touching source.
2. lvqr-mesh: PeerInfo + can_accept_child + effective_capacity +
   tests.
3. lvqr-mesh: coordinator add_peer signature + find_best_parent
   regression test (mixed capacities).
4. lvqr-signal: Register field + PeerEvent struct + PeerCallback
   reshape + tests.
5. lvqr-cli: bridge update (PeerEvent unpack, clamp, add_peer call,
   ws_relay_session passes None).
6. lvqr-admin: MeshPeerStats field + tests.
7. lvqr-cli integration test: mesh_capacity_e2e.rs (3-peer descent +
   clamp).
8. JS: MeshConfig + Register payload + admin types.
9. Python: types + client + tests.
10. Docs: mesh.md + sdk/javascript.md + sdk/python.md.
11. README flips: row checked off + mesh.md "IMPLEMENTED" status.
12. CI gates locally clean: fmt, clippy, cargo test --workspace,
    pytest, tsc.
13. Live smoke: `lvqr serve --max-peers 5 --mesh-enabled`, send a
    Register with capacity via wscat or a tokio-tungstenite probe,
    curl `/api/v1/mesh`, observe the value.
14. Two commits (feat + docs close). DO NOT PUSH (per user
    instruction; user explicitly gates push on a separate signal).

## Deliverables checklist

- [ ] `tracking/SESSION_144_BRIEFING.md` (this file)
- [ ] `crates/lvqr-mesh/src/tree.rs` -- `PeerInfo.capacity:
      Option<u32>`; `effective_capacity` helper; `can_accept_child`
      consults it; tests
- [ ] `crates/lvqr-mesh/src/coordinator.rs` -- `add_peer` grows
      `capacity: Option<u32>`; per-peer-cap regression test
- [ ] `crates/lvqr-signal/src/signaling.rs` -- `Register.capacity`
      with `#[serde(default)]`; `PeerEvent` struct; `PeerCallback`
      reshape; tests
- [ ] `crates/lvqr-cli/src/lib.rs` -- bridge unpacks `PeerEvent`,
      clamps capacity to `MeshConfig.max_children`, calls
      `add_peer(.., capacity)`; ws_relay_session passes `None`
- [ ] `crates/lvqr-admin/src/routes.rs` -- `MeshPeerStats.capacity:
      Option<u32>`; tests
- [ ] `crates/lvqr-cli/tests/mesh_capacity_e2e.rs` (new) -- 3-peer
      descent + clamp tests
- [ ] `bindings/js/packages/core/src/mesh.ts` -- `MeshConfig.capacity?:
      number`; Register payload includes the field
- [ ] `bindings/js/packages/core/src/admin.ts` -- `MeshPeerStats.capacity?:
      number`
- [ ] `bindings/js/tests/sdk/admin-client.spec.ts` -- mesh assertion
      grows capacity-shape check
- [ ] `bindings/python/python/lvqr/types.py` -- `MeshPeerStats.capacity:
      Optional[int] = None`
- [ ] `bindings/python/python/lvqr/client.py::mesh()` -- defensive
      `.get("capacity")` parse
- [ ] `bindings/python/tests/test_client.py` -- defaults + pre-144
      defensive parse
- [ ] `docs/mesh.md` -- new "Per-peer capacity (session 144)" section;
      "still phase-D scope" subsection now empty (or removed); status
      header flipped to "IMPLEMENTED"
- [ ] `docs/sdk/javascript.md` -- `MeshConfig.capacity` documented
- [ ] `docs/sdk/python.md` -- `MeshPeerStats.capacity` documented
- [ ] `README.md` -- "Per-peer capacity advertisement" bullet flipped
      `[ ]` -> `[x]` with shipped-in-144 prose; mesh-data-plane
      checklist now fully closed; mesh.md "IMPLEMENTED" line updated
- [ ] CI gates clean: fmt, clippy, cargo test --workspace, pytest,
      tsc on @lvqr/core
- [ ] Live smoke verified
- [ ] Session 144 close block on `HANDOFF.md`
- [ ] `MEMORY.md` status line updated
- [ ] Two commits (feat + docs close); **DO NOT PUSH** until user
      explicitly authorizes

## Open questions before implementation

1. **`PeerCallback` reshape to `PeerEvent` struct (decision 4).** I
   am picking the struct over a positional extension. Confirm or push
   back.
2. **Clamp at register time vs. on consult (decision 5).** I am
   picking clamp-at-ingest so the on-disk + admin-route values are
   always within ceiling. Confirm or push back.
3. **`docs/mesh.md` "IMPLEMENTED" flip in the same session.** With
   capacity shipped, the four mesh-data-plane bullets close and the
   gating condition for the IMPLEMENTED flip is met. Plan is to do
   the flip in the same session's docs commit. Confirm.
4. **Integration test scope only, no Playwright capacity test.**
   Confirm the Rust integration test (3-peer descent + clamp) is
   sufficient, or do you want a 4-peer Playwright spec mixing
   capacities?
5. **`/ws` subscribers stay capacity-less in v1** (always pass `None`).
   Operators with browser-only mesh deployments do not lose anything;
   /ws is server-side or non-browser. Confirm or push back.
