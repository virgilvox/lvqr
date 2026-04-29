// Session 116 row 115: two-peer DataChannel mesh relay E2E.
//
// This is the first end-to-end exercise of @lvqr/core's MeshPeer
// class against a real LVQR signaling server. The test:
//
//   1. Spawns `lvqr serve` with --mesh-enabled and
//      --mesh-root-peer-count 1 (via playwright.config.ts webServer).
//   2. Opens two browser contexts (peer_1 and peer_2) and injects
//      the compiled dist/mesh.js into each as a global.
//   3. Connects peer_1 first. It registers and receives
//      AssignParent{role: "Root", parent_id: null, depth: 0}.
//   4. Connects peer_2. It registers and receives
//      AssignParent{role: "Relay", parent_id: peer_1_id, depth: 1}.
//   5. peer_2's MeshPeer auto-initiates an RTCPeerConnection +
//      DataChannel to peer_1. SDP offer/answer and ICE candidates
//      flow through the /signal server.
//   6. Once the DataChannel is open on peer_1's side (i.e.
//      peer_1.childCount === 1), the test tells peer_1 to call
//      pushFrame(knownBytes).
//   7. peer_2's onFrame callback fires with the bytes. The test
//      polls window.__frames until the bytes arrive, then asserts
//      byte-for-byte equality.
//
// The test covers the full client-side relay path: signaling
// register + AssignParent + SDP offer/answer + ICE + DataChannel
// open + push + forward + receive. It does NOT exercise the
// server-originating media path (MoQ fanout into a root's
// pushFrame); that is phase-D scope per the session 116 briefing.

import { test, expect, Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const MESH_JS_PATH = resolve(__dirname, '../../../packages/core/dist/mesh.js');
const SIGNAL_URL = 'ws://127.0.0.1:18088/signal';
const ADMIN_URL = 'http://127.0.0.1:18088';
const BROADCAST = 'live/mesh-test';

// Dist ships as ESM; convert to a classic script that exposes
// MeshPeer as a global so page.addInitScript can pick it up without
// needing a module loader.
function loadMeshJsAsGlobal(): string {
  const raw = readFileSync(MESH_JS_PATH, 'utf-8');
  // Strip `export` from the class declaration; append a global
  // assignment so the test harness can construct MeshPeer directly
  // in page.evaluate callbacks.
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
  // Serve a blank HTML so page.goto has something to load without
  // needing the admin server to expose a static route. The only
  // network traffic we care about is the WebSocket to /signal.
  await page.goto('about:blank');
}

test('two-peer DataChannel mesh relays a root-pushed frame to the child', async ({ browser }) => {
  const payload = [0xDE, 0xAD, 0xBE, 0xEF, 0x10, 0x20, 0x30, 0x40];

  const contextA = await browser.newContext();
  const contextB = await browser.newContext();
  const pageA = await contextA.newPage();
  const pageB = await contextB.newPage();

  await installMeshPeerAndHarness(pageA);
  await installMeshPeerAndHarness(pageB);

  // --- peer_1 (root) ---
  await pageA.evaluate(
    async ({ signalUrl, track, peerId }) => {
      await (window as unknown as { __setupPeer: (c: Record<string, unknown>) => Promise<void> }).__setupPeer({
        signalUrl,
        peerId,
        track,
        // Empty iceServers: on loopback, host candidates are
        // sufficient and we want to avoid any external STUN lookup
        // that a sandboxed CI runner might block or slow down.
        iceServers: [],
      });
    },
    { signalUrl: SIGNAL_URL, track: BROADCAST, peerId: 'peer-one' },
  );

  // Give peer_1 a moment to register and receive AssignParent.
  // 30 s timeout absorbs CI runner WebSocket-handshake jitter; the
  // happy-path latency is sub-second locally + on Linux CI but the
  // ubuntu-latest runner under load has been observed to take
  // 10-20 s to complete the signal-protocol exchange.
  await pageA.waitForFunction(
    () => (window as unknown as { __peer?: { peerRole: string } }).__peer?.peerRole === 'Root',
    null,
    { timeout: 30_000 },
  );

  // --- peer_2 (relay child) ---
  await pageB.evaluate(
    async ({ signalUrl, track, peerId }) => {
      await (window as unknown as { __setupPeer: (c: Record<string, unknown>) => Promise<void> }).__setupPeer({
        signalUrl,
        peerId,
        track,
        iceServers: [],
      });
    },
    { signalUrl: SIGNAL_URL, track: BROADCAST, peerId: 'peer-two' },
  );

  await pageB.waitForFunction(
    () => (window as unknown as { __peer?: { peerRole: string } }).__peer?.peerRole === 'Relay',
    null,
    { timeout: 30_000 },
  );

  // MeshPeer.children is populated on `pc.ondatachannel` with a
  // `{ dc: null }` entry, then `dc` is set inside that handler --
  // but the DataChannel itself may still be in the `connecting`
  // state at that moment. `childCount >= 1` is therefore a
  // necessary-but-not-sufficient signal. `forwardToChildren` skips
  // sends when `dc.readyState !== 'open'`, so the first pushFrame
  // may silently no-op on a tight race.
  //
  // Defensive approach: fire a pushFrame loop on peer_1 at 100 ms
  // cadence for up to 20 s. As soon as the DataChannel is open on
  // both sides, the next push lands. peer_2 captures every received
  // frame; the assertion below finds the payload in the history.
  await pageA.evaluate((bytes) => {
    const peer = (window as unknown as { __peer: { pushFrame: (d: Uint8Array) => void } }).__peer;
    (window as unknown as { __pushTimer?: ReturnType<typeof setInterval> }).__pushTimer = setInterval(() => {
      peer.pushFrame(new Uint8Array(bytes));
    }, 100);
  }, payload);

  // The child should receive the same bytes via its DataChannel
  // onmessage -> onFrame callback. The pushFrame loop continues in
  // the background until the test ends.
  await pageB.waitForFunction(
    (expected) => {
      const frames = (window as unknown as { __frames: number[][] }).__frames;
      return frames.some((f) => f.length === expected.length && f.every((b, i) => b === expected[i]));
    },
    payload,
    { timeout: 20_000 },
  );

  // Stop the push loop so the browser context can close cleanly.
  await pageA.evaluate(() => {
    const timer = (window as unknown as { __pushTimer?: ReturnType<typeof setInterval> }).__pushTimer;
    if (timer) {
      clearInterval(timer);
    }
  });

  const receivedFrames = await pageB.evaluate(
    () => (window as unknown as { __frames: number[][] }).__frames,
  );
  const matching = receivedFrames.find(
    (f) => f.length === payload.length && f.every((b, i) => b === payload[i]),
  );
  expect(matching).toEqual(payload);

  // Session 141: the `pushFrame` loop is still running from the
  // earlier `setInterval`. Let the client-side 1 s ForwardReport
  // timer fire at least twice and stop the push loop so the counts
  // stabilise, then poll `/api/v1/mesh` and assert the actual-vs-
  // intended shape. The sample windows overlap in practice (the
  // first report fires 1 s after signal.onopen, before a DataChannel
  // exists on a fresh harness), so the second poll is the one that
  // reliably reflects forwarded frames.
  await pageA.waitForTimeout(2_500);
  await pageA.evaluate(() => {
    const timer = (window as unknown as { __pushTimer?: ReturnType<typeof setInterval> }).__pushTimer;
    if (timer) {
      clearInterval(timer);
    }
  });
  // Let the last ForwardReport emit before we poll.
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
  expect(Array.isArray(mesh.peers)).toBe(true);

  const root = mesh.peers.find((p) => p.peer_id === 'peer-one');
  const relay = mesh.peers.find((p) => p.peer_id === 'peer-two');
  expect(root, 'peer-one must be present in the mesh snapshot').toBeDefined();
  expect(relay, 'peer-two must be present in the mesh snapshot').toBeDefined();

  // peer-one is the root with peer-two attached as its child.
  expect(root!.role).toBe('Root');
  expect(root!.parent).toBeNull();
  expect(root!.depth).toBe(0);
  expect(root!.intended_children).toBe(1);
  // peer-one has been forwarding frames to peer-two on a 100 ms
  // cadence for ~2.5 s; the actual count is harness-dependent but
  // must be strictly positive (at least one send landed before the
  // channel close race).
  expect(root!.forwarded_frames).toBeGreaterThan(0);

  // peer-two is a leaf in this two-peer harness (no grandchildren).
  // It has received frames but has not forwarded any.
  expect(relay!.role).toBe('Relay');
  expect(relay!.parent).toBe('peer-one');
  expect(relay!.depth).toBe(1);
  expect(relay!.intended_children).toBe(0);
  expect(relay!.forwarded_frames).toBe(0);

  await contextA.close();
  await contextB.close();
});
