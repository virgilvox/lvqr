# Session 141 Briefing -- Actual-vs-intended mesh offload reporting

**Date kick-off**: 2026-04-24
**Phase D row**: README `Peer mesh data plane` item "Actual-vs-intended
offload reporting: clients report 'served by peer X'; coordinator
aggregates; `/api/v1/mesh` returns measured offload."
**Predecessor**: Session 140 (per-slot WASM counters) -- same shape
(admin-surface observability extension with `#[serde(default)]` cross-
version compat).

## Goal

Extend `GET /api/v1/mesh` so operators can see, per peer: the *intended*
offload the topology planner assigned (child-count from the tree) AND
the *actual* offload the peer has delivered (cumulative count of frames
it forwarded to its children via WebRTC DataChannel). This closes the
first of four named unshipped items under `README.md` "Peer mesh data
plane" (lines 439-450).

Out-of-scope for this session (remaining phase-D mesh items, each a
future session):
* Per-peer capacity advertisement
* TURN deployment recipe
* Three-peer browser Playwright E2E
* Flipping `docs/mesh.md` to IMPLEMENTED

## Wire shape

Two additive changes, both `#[serde(default)]` for cross-version compat
mirroring session 140.

### New `SignalMessage::ForwardReport` variant (client -> server only)

```rust
// lvqr-signal/src/signaling.rs
SignalMessage::ForwardReport { forwarded_frames: u64 }
```

No `peer_id` on the wire: the server resolves it from the WS session
state (the post-Register `peer_id` closed over in `handle_ws_connection`).
This tightens the contract -- a peer can only report for itself --
and matches the intent of the pre-existing `handle_signal_message`
signature (`from_peer: &str` is the trusted identity).

`forwarded_frames` is **cumulative** (client sends its own running total).
Server replaces rather than accumulates, which makes reconnect safe: at
reconnect the client counter starts at 0, the server sees the reset, and
the displayed offload simply drops to the new running total.

### Extended `/api/v1/mesh` response

```rust
// lvqr-admin/src/routes.rs
pub struct MeshState {
    pub enabled: bool,
    pub peer_count: usize,
    pub offload_percentage: f64,
    #[serde(default)]
    pub peers: Vec<MeshPeerStats>, // NEW
}

pub struct MeshPeerStats {
    pub peer_id: String,
    pub role: String,                // "Root" | "Relay" | "Leaf"
    pub parent: Option<String>,
    pub depth: u32,
    pub intended_children: usize,    // children.len() from tree planner
    pub forwarded_frames: u64,       // from last ForwardReport
}
```

### `PeerInfo` gets one new field

```rust
// lvqr-mesh/src/tree.rs
pub struct PeerInfo {
    // ... existing fields unchanged ...
    #[serde(default)]
    pub forwarded_frames: u64,       // NEW: replaced by record_forward_report
}
```

Plain `u64`, not atomic: DashMap already serializes per-key access via
`get_mut`, so the write is already guarded. Atomics would complicate the
existing `Clone` derive without buying anything.

### `MeshCoordinator::record_forward_report`

```rust
// lvqr-mesh/src/coordinator.rs
impl MeshCoordinator {
    pub fn record_forward_report(&self, peer_id: &str, forwarded_frames: u64) {
        if let Some(mut entry) = self.peers.get_mut(peer_id) {
            entry.forwarded_frames = forwarded_frames;
        }
        // unknown peer is silently ignored: a client may briefly
        // outlive its tree entry (remove_peer happened between
        // the client's last emit and WS close); not an error.
    }
}
```

### `SignalServer` gets a sibling callback

```rust
// lvqr-signal/src/signaling.rs
pub type ForwardReportCallback = Arc<dyn Fn(&str, u64) + Send + Sync>;

impl SignalServer {
    pub fn set_forward_report_callback(&mut self, cb: ForwardReportCallback);
}
```

Mirrors the existing `on_peer: PeerCallback` pattern. Keeps
`lvqr-signal` independent of `lvqr-mesh`; the bridge is wired in
`lvqr-cli::start()` alongside the existing register/unregister bridge.

## Client-side reporting (JS)

`MeshPeer` gains:
* A private `forwardedFrameCount: number` field, incremented inside
  `forwardToChildren` per successful `dc.send()`.
* A new public getter `forwardedFrameCount: number`.
* A 1-second `setInterval` started after the WS opens that emits
  `{type: "ForwardReport", forwarded_frames: N}` ONLY when N has
  changed since the last emit (skip-on-unchanged to avoid WS chatter
  on idle peers and on leaf peers that never forward).
* Interval cleanup in `close()`.

No Python SDK change: `lvqr` Python bindings do not ship a `MeshPeer`
-- that surface is browser-only, and Python has no analogous role.

## Reporting cadence decision

1-second interval picked because:
* Admin-polling operators reading `/api/v1/mesh` get sub-real-time
  visibility without the WS becoming noisy.
* Forwarded-frame counts grow slowly (frame rate ~30 fps, one frame
  = one send) so a 1-second sample is more than enough resolution for
  operators.
* Skip-on-unchanged keeps idle peers silent on the signaling channel.

No per-child breakdown. One aggregate counter per peer. Operator
question answered: "is peer X actually relaying to its tree children?"
That is a yes/no shape; per-child splits belong in a future v1.2
session if operator feedback asks for them.

## Testing strategy

* `lvqr-mesh`: unit tests for `record_forward_report` (default zero,
  record sets field, replaces monotonically, unknown peer is no-op,
  non-interference between peers).
* `lvqr-signal`: unit tests for `ForwardReport` round-trip, callback
  invoked with correct peer_id + count, callback NOT invoked for
  unregistered sessions, variant serde tag format.
* `lvqr-admin`: configured-snapshot unit test asserts `peers` vec
  shape; disabled-route test asserts `peers` is empty.
* `lvqr-cli`: no new integration test file; the existing
  `wasm_filter_admin_route.rs` pattern does not apply cleanly here
  because the mesh route needs a live signal session. Coverage comes
  from the Playwright E2E extension below.
* JS Vitest: `MeshPeer.forwardedFrameCount` getter starts at 0, grows
  after `pushFrame`. This can live as a unit test of the class
  without a real WS.
* JS Playwright (`bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts`):
  after the existing `pushFrame` loop, poll `GET /api/v1/mesh` and
  assert that peer-1 (root) shows `forwarded_frames >= N` and
  `intended_children == 1`; peer-2 (relay) shows `forwarded_frames
  == 0` (no grandchildren in this two-peer harness) and
  `intended_children == 0`.
* Python pytest: type-default tests for the new dataclass, plus a
  client-mock test that the `mesh()` method parses the new `peers`
  array and a defensive-parse test for pre-141 server bodies.

## Scope estimate

~550-650 lines across:
* `crates/lvqr-mesh/src/tree.rs` -- 1 field + 1 test
* `crates/lvqr-mesh/src/coordinator.rs` -- 1 method + 4-5 tests
* `crates/lvqr-signal/src/signaling.rs` -- 1 variant + 1 callback
  type + 1 setter + handler arm + 2-3 tests
* `crates/lvqr-admin/src/routes.rs` -- 1 type + 1 field + default
  closure update + 2 test updates + re-export
* `crates/lvqr-admin/src/lib.rs` -- re-export
* `crates/lvqr-cli/src/lib.rs` -- wire `set_forward_report_callback`
  + extend `with_mesh` closure to build `Vec<MeshPeerStats>`
* `bindings/js/packages/core/src/mesh.ts` -- counter + interval +
  getter + close cleanup
* `bindings/js/packages/core/src/admin.ts` -- 1 interface + field +
  re-export
* `bindings/js/packages/core/src/index.ts` -- re-export
* `bindings/js/tests/sdk/admin-client.spec.ts` -- grow mesh
  assertions
* `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts` -- new
  assertion block after `pushFrame` loop
* `bindings/python/python/lvqr/types.py` -- 1 dataclass + field
* `bindings/python/python/lvqr/client.py` -- defensive parse
* `bindings/python/python/lvqr/__init__.py` -- re-export
* `bindings/python/tests/test_client.py` -- 3 new tests (defaults,
  populated, pre-141 defensive)
* `docs/mesh.md` -- new wire section naming `ForwardReport` +
  admin route extension; phase-D checklist flipped 1/4 row from
  `[ ]` to `[x]`.
* `docs/sdk/javascript.md` -- `MeshPeerStats` + `MeshState.peers`
* `docs/sdk/python.md` -- `MeshPeerStats` + `MeshState.peers`
* `docs/observability.md` -- new mesh offload-reporting bullet

## Anti-scope

* **No per-(peer, child) breakdown.** Single aggregate counter per
  peer.
* **No byte counters.** Frame count only; matches `pushFrame`
  semantics.
* **No received-from-parent counter.** v1 reports forwarded-out
  only, which answers "who served whom" via the topology lookup.
* **No Prometheus metric exposure** for the new counters.
* **No protocol change** to DataChannel-carried frames. The count
  lives entirely client-side and is reported via `/signal`.
* **No defensive rate-limit** on ForwardReport emissions. 1 Hz with
  skip-on-unchanged already bounds the rate to ~1 msg/sec/peer.
* **No Python `MeshPeer`.** Browser-only.

## Deliverables checklist

- [ ] `tracking/SESSION_141_BRIEFING.md` (this file)
- [ ] `crates/lvqr-mesh/src/tree.rs` -- `forwarded_frames` field
- [ ] `crates/lvqr-mesh/src/coordinator.rs` --
      `record_forward_report` + tests
- [ ] `crates/lvqr-signal/src/signaling.rs` -- `ForwardReport`
      variant + callback + routing + tests
- [ ] `crates/lvqr-admin/src/routes.rs` -- `MeshPeerStats` +
      `MeshState.peers` + tests
- [ ] `crates/lvqr-admin/src/lib.rs` -- re-export
- [ ] `crates/lvqr-cli/src/lib.rs` -- bridge signal callback into
      coordinator + extend admin mesh closure
- [ ] `bindings/js/packages/core/src/mesh.ts` -- counter + periodic
      emit + getter + cleanup
- [ ] `bindings/js/packages/core/src/admin.ts` -- type + field
- [ ] `bindings/js/packages/core/src/index.ts` -- re-export
- [ ] `bindings/js/tests/sdk/admin-client.spec.ts` -- assertions
- [ ] `bindings/js/tests/e2e/mesh/two-peer-relay.spec.ts` --
      assertions
- [ ] `bindings/python/python/lvqr/types.py` -- dataclass + field
- [ ] `bindings/python/python/lvqr/client.py` -- defensive parse
- [ ] `bindings/python/python/lvqr/__init__.py` -- re-export
- [ ] `bindings/python/tests/test_client.py` -- 3 new tests
- [ ] `docs/mesh.md` + `docs/sdk/javascript.md` +
      `docs/sdk/python.md` + `docs/observability.md`
- [ ] CI gates clean: fmt, clippy, cargo test --workspace, pytest,
      tsc on both @lvqr/core + @lvqr/player, vitest
- [ ] Live smoke of `GET /api/v1/mesh` against a mesh-enabled
      lvqr
- [ ] Session 141 close block on `HANDOFF.md`
- [ ] `MEMORY.md` status line updated
- [ ] Two commits (feat + docs close); DO NOT PUSH
