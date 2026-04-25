# Session 142 Briefing -- Three-peer Playwright matrix

**Date kick-off**: 2026-04-24
**Phase D row**: README `Peer mesh data plane` item "Three-peer
browser Playwright E2E feeding the 5-artifact test contract"
(README.md line 446-447).
**Predecessor**: Session 141 (mesh offload reporting). The new
three-peer test naturally validates session 141's `forwarded_frames`
field on a non-trivial topology -- specifically the depth-2 case
where peer-2 (Relay) forwards to peer-3 and so should report
`forwarded_frames > 0` rather than the leaf zero.

## Goal

Ship a Playwright spec that exercises a **depth-2 chain**
(peer-1 -> peer-2 -> peer-3) of WebRTC DataChannel relays. Frames
pushed at the root must reach the grandchild leaf via the middle
peer's `dc.onmessage -> forwardToChildren` path, and `GET
/api/v1/mesh` must report the correct intended-vs-actual offload
shape for all three peers.

This closes one of the four unshipped bullets under README's
"Peer mesh data plane" checklist; remaining unshipped after 142:
per-peer capacity advertisement, TURN deployment recipe, the
mesh.md "IMPLEMENTED" flip.

## Topology forcing

`MeshCoordinator::find_best_parent` picks the shallowest peer with
available child slots, then breaks ties by fewest children. With
`--mesh-root-peer-count 1` (only one Root) and `--max-peers 1` (each
peer accepts one child), the assignment is deterministic:

| Peer | Connects | Resulting role | Parent | Depth |
|---|---|---|---|---|
| peer-1 | first | Root | -- | 0 |
| peer-2 | second | Relay | peer-1 | 1 |
| peer-3 | third | Relay | peer-2 | 2 |

`peer-1` is full after peer-2 attaches (max-peers=1), so the planner
descends to peer-2 for peer-3. The chain forms without retries.

## Forwarded-frame expectations

| Peer | `intended_children` | `forwarded_frames` |
|---|---|---|
| peer-1 (Root) | 1 | `> 0` (forwards to peer-2) |
| peer-2 (Relay) | 1 | `> 0` (forwards to peer-3) |
| peer-3 (Relay leaf) | 0 | `== 0` (no children) |

The peer-2 row is the load-bearing assertion: it proves the multi-
hop relay path increments the `forwardedFrames` counter on the
middle peer (whose `dc.onmessage` calls `forwardToChildren` for
fanout). A single-peer test cannot distinguish "peer-2 forwarded
to peer-3" from "peer-2 just received from peer-1 and never
forwarded", because session 141's counter only fires on `dc.send`.

## Wire shape changes

None. This session is pure test + docs. No source files outside
`bindings/js/`.

## Playwright config tweak

`playwright.config.ts` adds `--max-peers 1` to the webServer
command. The existing two-peer-relay.spec.ts continues to pass
because it only needs 1 child slot on peer-1 (peer-2 is its only
child). The change is global rather than per-spec because
Playwright's `webServer` block runs once for the whole suite.

If a future fan-out test (e.g. one Root with three children) needs
the default max-peers=3, we will add a second `webServer` entry
on a different admin port. Not in scope for 142.

## Test file layout

* New `bindings/js/tests/e2e/mesh/three-peer-chain.spec.ts`,
  not an extension of the existing two-peer file. Separate file so:
  * The two-peer test stays focused on its happy-path narrative.
  * CI can opt-in/out of the new test independently if it proves
    flaky on slow runners.
  * Test-name labelling is clearer in the Playwright report.

* The spec follows the same harness pattern as
  `two-peer-relay.spec.ts`:
  1. Compile-time read of `dist/mesh.js`, strip the `export` so the
     class becomes a global.
  2. `addInitScript` injects `window.MeshPeer` and a thin
     `__setupPeer` helper plus a `__frames` array.
  3. `page.goto('about:blank')` to give Playwright something to
     wait on.

## Test scope

```
test('three-peer chain relays root-pushed frames to a depth-2 grandchild', ...)
```

Single test. Steps:

1. Spawn three browser contexts, install the MeshPeer harness on
   each.
2. Connect peer-1 first; wait for `peerRole === 'Root'`.
3. Connect peer-2; wait for `peerRole === 'Relay'` AND
   `parentId === 'peer-1'` (verified via the mesh admin route or a
   new MeshPeer.parentId getter).
4. Wait for `peer-1.childCount === 1` (DataChannel open on the
   parent side; the existing two-peer test uses the same wait).
5. Connect peer-3; wait for `peerRole === 'Relay'`.
6. Wait for `peer-2.childCount === 1` (DataChannel open from
   peer-3 to peer-2).
7. Push frames from peer-1 on a 100 ms loop.
8. Wait for peer-3's `__frames` array to contain a matching
   payload (proves the chain transit works).
9. Wait ~2.5 s for two ForwardReport emits to land on the server,
   stop the push loop, wait another ~1.2 s for the final emit.
10. Poll `GET /api/v1/mesh` and assert the per-peer shape table
    above.

## MeshPeer change (small)

Add a public getter `parentId: string | null` to `MeshPeer` so the
test can wait for the assignment without fishing in private state.
Single line added next to `peerRole`. The field already exists as
private state; this is a read-only accessor.

## Anti-scope

* **No 4+ peer matrix.** Three peers exercise the multi-hop relay
  path; four-plus is incremental signal at proportionally higher
  flake risk. Future Playwright work can extend.
* **No browser matrix expansion.** Chromium only, same as session
  115. Firefox / WebKit + RTCPeerConnection compat is a separate
  v1.2 candidate.
* **No frame-rate or throughput assertions.** The test asserts
  bytes match and `forwarded_frames > 0`, not a specific count.
  SCTP timing variance on shared CI runners makes exact counts
  flake-prone (same rationale as session 141's two-peer
  assertion).
* **No fault-injection.** The "kill the middle peer; assert peer-3
  is reassigned" scenario would test the orphan-reassignment path
  but requires Playwright's networking primitives to fail-stop a
  WebRTC connection mid-flight. Out of scope; lives at the
  `lvqr-mesh` integration-test layer if anywhere.
* **No source changes outside `bindings/js/`.** Pure test + docs.

## Deliverables checklist

- [ ] `tracking/SESSION_142_BRIEFING.md` (this file)
- [ ] `bindings/js/playwright.config.ts` -- add `--max-peers 1`
- [ ] `bindings/js/packages/core/src/mesh.ts` -- new
      `parentId: string | null` getter
- [ ] `bindings/js/tests/e2e/mesh/three-peer-chain.spec.ts` --
      new spec
- [ ] `docs/mesh.md` -- flip the "three-peer Playwright matrix"
      bullet from `[ ]` to `[x]`; phase-D scope subsection trims
      that line out and into the shipped paragraph.
- [ ] CI gates: tsc clean on `@lvqr/core` (player unaffected);
      no Rust changes so workspace tests are unchanged
- [ ] Session 142 close block on `HANDOFF.md`
- [ ] `MEMORY.md` status line updated
- [ ] Two commits (feat + docs close)
