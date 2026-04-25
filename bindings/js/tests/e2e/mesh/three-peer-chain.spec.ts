// Session 142: three-peer DataChannel mesh chain E2E.
//
// Extends session 115's two-peer test to a depth-2 chain:
//
//   peer-1 (Root, depth 0)
//     |-- peer-2 (Relay, depth 1)
//           |-- peer-3 (Relay, depth 2)
//
// The webServer entry in playwright.config.ts spawns lvqr with
// `--mesh-root-peer-count 1` AND `--max-peers 1`, so:
//   * peer-1 takes the only Root slot.
//   * peer-2 attaches to peer-1 (peer-1's only child slot).
//   * peer-3 descends because peer-1 is full; attaches to peer-2.
//
// What this proves over and above the two-peer test:
//   1. Multi-hop relay works -- peer-3 receives bytes that peer-1
//      pushed only into peer-2's DataChannel; peer-2's
//      `dc.onmessage -> forwardToChildren` path is what carries
//      them across the second hop.
//   2. Session 141's offload-reporting feature behaves on the
//      depth-2 case: the middle peer's `forwarded_frames` counter
//      is strictly positive (the leaf never forwards, so
//      forwarded_frames there is 0). A single-hop test cannot
//      distinguish "received-then-forwarded" from "received-only".
//
// What this does NOT cover (anti-scope, see SESSION_142_BRIEFING):
//   * 4+ peer matrix.
//   * Browser matrix beyond Chromium.
//   * Fault injection / orphan reassignment.
//   * Frame-rate or throughput assertions.

import { test, expect, Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const MESH_JS_PATH = resolve(__dirname, '../../../packages/core/dist/mesh.js');
const SIGNAL_URL = 'ws://127.0.0.1:18088/signal';
const ADMIN_URL = 'http://127.0.0.1:18088';
const BROADCAST = 'live/three-peer-test';

// Same conversion as the two-peer spec: dist ships as ESM, so we
// strip the `export` and assign the class to `window.MeshPeer` for
// page.addInitScript injection.
function loadMeshJsAsGlobal(): string {
  const raw = readFileSync(MESH_JS_PATH, 'utf-8');
  const stripped = raw.replace(/^export\s+class\s+MeshPeer/m, 'class MeshPeer');
  return `${stripped}\nwindow.MeshPeer = MeshPeer;\n`;
}

async function installMeshPeerAndHarness(page: Page): Promise<void> {
  const meshScript = loadMeshJsAsGlobal();
  await page.addInitScript({ content: meshScript });
  await page.addInitScript(() => {
    (window as unknown as { __frames: number[][] }).__frames = [];
    (window as unknown as { __setupPeer: (c: Record<string, unknown>) => Promise<void> }).__setupPeer =
      async function (config: Record<string, unknown>) {
        const MeshPeer = (window as unknown as { MeshPeer: unknown }).MeshPeer as new (c: Record<string, unknown>) => {
          connect(): Promise<void>;
        };
        const frames = (window as unknown as { __frames: number[][] }).__frames;
        const peer = new MeshPeer({
          ...config,
          onFrame: (data: Uint8Array) => {
            frames.push(Array.from(data));
          },
        });
        (window as unknown as { __peer: unknown }).__peer = peer;
        await peer.connect();
      };
  });
  await page.goto('about:blank');
}

async function connectPeer(page: Page, peerId: string): Promise<void> {
  await page.evaluate(
    async ({ signalUrl, track, peerId: pid }) => {
      await (window as unknown as { __setupPeer: (c: Record<string, unknown>) => Promise<void> }).__setupPeer({
        signalUrl,
        peerId: pid,
        track,
        // Empty iceServers: on loopback host candidates suffice and
        // we want to avoid any external STUN lookup that a sandboxed
        // CI runner might block or delay.
        iceServers: [],
      });
    },
    { signalUrl: SIGNAL_URL, track: BROADCAST, peerId },
  );
}

test('three-peer chain relays root-pushed frames to a depth-2 grandchild', async ({ browser }) => {
  const payload = [0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x23, 0x45, 0x67];

  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  const contextC = await browser.newContext();
  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();
  const pageC = await contextC.newPage();

  await installMeshPeerAndHarness(pageA);
  await installMeshPeerAndHarness(pageB);
  await installMeshPeerAndHarness(pageC);

  // --- peer-1 (Root) ---
  await connectPeer(pageA, 'peer-one');
  await pageA.waitForFunction(
    () => (window as unknown as { __peer?: { peerRole: string } }).__peer?.peerRole === 'Root',
    null,
    { timeout: 10_000 },
  );

  // --- peer-2 (Relay attached to peer-1) ---
  await connectPeer(pageB, 'peer-two');
  await pageB.waitForFunction(
    () => {
      const peer = (window as unknown as { __peer?: { peerRole: string; parentPeerId: string | null } }).__peer;
      return peer?.peerRole === 'Relay' && peer?.parentPeerId === 'peer-one';
    },
    null,
    { timeout: 10_000 },
  );

  // Wait for peer-1 to register peer-2 as a child via ondatachannel.
  // This is necessary before peer-3 connects, because peer-1's child
  // slot is what determines whether peer-3 cascades to peer-2 or
  // races into peer-1's slot first (with --max-peers 1, the slot
  // claim is whichever Register lands at the coordinator first).
  await pageA.waitForFunction(
    () => (window as unknown as { __peer?: { childCount: number } }).__peer!.childCount >= 1,
    null,
    { timeout: 10_000 },
  );

  // --- peer-3 (Relay attached to peer-2 -- depth 2) ---
  await connectPeer(pageC, 'peer-three');
  await pageC.waitForFunction(
    () => {
      const peer = (window as unknown as { __peer?: { peerRole: string; parentPeerId: string | null } }).__peer;
      return peer?.peerRole === 'Relay' && peer?.parentPeerId === 'peer-two';
    },
    null,
    { timeout: 10_000 },
  );

  // Wait for peer-2 to register peer-3 as a child. Without this the
  // initial pushFrame ticks may silently no-op while the second-hop
  // DataChannel is still in 'connecting'.
  await pageB.waitForFunction(
    () => (window as unknown as { __peer?: { childCount: number } }).__peer!.childCount >= 1,
    null,
    { timeout: 10_000 },
  );

  // Push frames from peer-1 on a 100 ms loop. The two-peer spec
  // documents the full rationale for the loop pattern; same
  // rationale applies here, with the additional observation that
  // the second hop adds another readyState race window.
  await pageA.evaluate((bytes) => {
    const peer = (window as unknown as { __peer: { pushFrame: (d: Uint8Array) => void } }).__peer;
    (window as unknown as { __pushTimer?: ReturnType<typeof setInterval> }).__pushTimer = setInterval(() => {
      peer.pushFrame(new Uint8Array(bytes));
    }, 100);
  }, payload);

  // peer-3 (the depth-2 grandchild) should receive the bytes via
  // the chain peer-1 -> peer-2 -> peer-3. The middle peer's
  // dc.onmessage -> forwardToChildren is the load-bearing path
  // here; if peer-2 dropped the message after delivering to its
  // own onFrame (the test does not configure onFrame on peer-2,
  // but forwardToChildren runs regardless), this assertion fails.
  await pageC.waitForFunction(
    (expected) => {
      const frames = (window as unknown as { __frames: number[][] }).__frames;
      return frames.some((f) => f.length === expected.length && f.every((b, i) => b === expected[i]));
    },
    payload,
    { timeout: 20_000 },
  );

  // Let session 141's 1 s ForwardReport interval emit at least
  // twice (peer-2 needs at least one emit AFTER it begins
  // forwarding to peer-3, otherwise its forwarded_frames stays at
  // 0 in the admin snapshot). 2.5 s is a comfortable margin.
  await pageA.waitForTimeout(2_500);
  await pageA.evaluate(() => {
    const timer = (window as unknown as { __pushTimer?: ReturnType<typeof setInterval> }).__pushTimer;
    if (timer) {
      clearInterval(timer);
    }
  });
  // Final ForwardReport tick after we stop pushing.
  await pageA.waitForTimeout(1_200);

  type MeshPeerStats = {
    peer_id: string;
    role: string;
    parent: string | null;
    depth: number;
    intended_children: number;
    forwarded_frames: number;
  };
  const mesh = (await (await fetch(`${ADMIN_URL}/api/v1/mesh`)).json()) as {
    enabled: boolean;
    peer_count: number;
    offload_percentage: number;
    peers: MeshPeerStats[];
  };
  expect(mesh.enabled).toBe(true);
  expect(mesh.peer_count).toBe(3);
  // Two of three peers are non-root, so intended offload = 2/3 ~= 66.7%.
  expect(mesh.offload_percentage).toBeGreaterThan(60);
  expect(mesh.offload_percentage).toBeLessThan(70);
  expect(Array.isArray(mesh.peers)).toBe(true);
  expect(mesh.peers.length).toBe(3);

  const root = mesh.peers.find((p) => p.peer_id === 'peer-one');
  const middle = mesh.peers.find((p) => p.peer_id === 'peer-two');
  const leaf = mesh.peers.find((p) => p.peer_id === 'peer-three');
  expect(root, 'peer-one (Root) must be present').toBeDefined();
  expect(middle, 'peer-two (Middle) must be present').toBeDefined();
  expect(leaf, 'peer-three (Leaf) must be present').toBeDefined();

  // peer-1: Root with one child (peer-2). Has been forwarding for
  // ~2.5 s on a 100 ms cadence; count is harness-dependent but
  // strictly positive.
  expect(root!.role).toBe('Root');
  expect(root!.parent).toBeNull();
  expect(root!.depth).toBe(0);
  expect(root!.intended_children).toBe(1);
  expect(root!.forwarded_frames).toBeGreaterThan(0);

  // peer-2: Middle Relay with one child (peer-3). The
  // load-bearing assertion: forwarded_frames > 0 proves the
  // second hop actually carried bytes.
  expect(middle!.role).toBe('Relay');
  expect(middle!.parent).toBe('peer-one');
  expect(middle!.depth).toBe(1);
  expect(middle!.intended_children).toBe(1);
  expect(middle!.forwarded_frames).toBeGreaterThan(0);

  // peer-3: depth-2 Relay leaf. No children, no forwards.
  expect(leaf!.role).toBe('Relay');
  expect(leaf!.parent).toBe('peer-two');
  expect(leaf!.depth).toBe(2);
  expect(leaf!.intended_children).toBe(0);
  expect(leaf!.forwarded_frames).toBe(0);

  await contextA.close();
  await contextB.close();
  await contextC.close();
});
