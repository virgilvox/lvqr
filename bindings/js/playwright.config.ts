// Playwright config for the session 116 row 115 two-peer mesh E2E.
//
// The `webServer` block launches `lvqr serve` with mesh-enabled and
// `mesh-root-peer-count=1` so the second subscriber is forced into a
// Relay-child role. Fixed admin port so the test knows where to POST
// the /signal WebSocket URL. Other ports are deliberately off-default
// to avoid colliding with a locally-running lvqr instance on a
// developer workstation.
//
// Browsers: Chromium only. The MeshPeer client uses RTCPeerConnection
// + RTCDataChannel, both of which Playwright's Chromium bundle
// supports natively. Firefox/WebKit matrix is phase-D scope per the
// session 116 briefing.

import { defineConfig, devices } from '@playwright/test';

const ADMIN_PORT = 18088;

export default defineConfig({
  testDir: './tests/e2e',
  timeout: 60_000,
  expect: {
    timeout: 15_000,
  },
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${ADMIN_PORT}`,
    trace: 'on-first-retry',
    actionTimeout: 15_000,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: {
    // `target/debug/lvqr` is built by `cargo build -p lvqr-cli`. The
    // test-runner is expected to ensure the binary exists before
    // invoking Playwright (CI: cargo build step; locally: either
    // cargo build beforehand or the binary is already present).
    command: [
      '../../target/debug/lvqr serve',
      '--mesh-enabled',
      '--mesh-root-peer-count 1',
      // Session 142: cap each peer at one child so the three-peer
      // chain test (peer-1 -> peer-2 -> peer-3) forms deterministic-
      // ally. Peer-2 attaches to peer-1 (its only slot); peer-3
      // descends to peer-2. The two-peer-relay spec is unaffected
      // because it only ever has one child on peer-1.
      '--max-peers 1',
      '--no-auth-signal',
      `--admin-port ${ADMIN_PORT}`,
      '--hls-port 0',
      '--rtmp-port 11935',
      '--port 14443',
    ].join(' '),
    // /api/v1/mesh returns 200 JSON when --mesh-enabled; the default
    // `/` 404 causes Playwright's url poller (which accepts only
    // <400) to retry until the 30 s timeout.
    url: `http://127.0.0.1:${ADMIN_PORT}/api/v1/mesh`,
    reuseExistingServer: false,
    timeout: 30_000,
    stdout: 'pipe',
    stderr: 'pipe',
  },
});
