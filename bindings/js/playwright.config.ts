// Playwright config -- two project profiles share this file:
//
//   * `mesh` (sessions 116 / 142): two- and three-peer DataChannel
//     E2E. webServer launches `lvqr serve --mesh-enabled
//     --mesh-root-peer-count 1 --max-peers 1` on port 18088.
//
//   * `dvr-player` (session 153): mounts the @lvqr/dvr-player web
//     component against a live HLS endpoint with DVR window enabled.
//     webServer launches a second `lvqr serve` with --archive-dir
//     and --hls-dvr-window-secs=300 on port 18089 (non-overlapping
//     with the mesh server so both can run in parallel).
//
// Browsers: Chromium only. Cross-browser is phase-D scope.

import { defineConfig, devices } from '@playwright/test';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const MESH_ADMIN_PORT = 18088;
const DVR_ADMIN_PORT = 18089;
const DVR_RTMP_PORT = 11936;
const DVR_HLS_PORT = 18190;
const DVR_LVQR_PORT = 14444;

const DVR_ARCHIVE_DIR = join(tmpdir(), `lvqr-dvr-player-e2e-${process.pid}`);

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
    trace: 'on-first-retry',
    actionTimeout: 15_000,
  },
  projects: [
    {
      name: 'mesh',
      testMatch: /mesh\/.*\.spec\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: `http://127.0.0.1:${MESH_ADMIN_PORT}`,
      },
    },
    {
      name: 'dvr-player',
      testMatch: /dvr-player\/.*\.spec\.ts$/,
      use: {
        ...devices['Desktop Chrome'],
        baseURL: `http://127.0.0.1:${DVR_ADMIN_PORT}`,
      },
    },
  ],
  webServer: [
    {
      // `target/debug/lvqr` is built by `cargo build -p lvqr-cli`.
      // The test-runner ensures the binary exists before invoking
      // Playwright (CI: cargo build step; locally: either cargo
      // build beforehand or the binary is already present).
      command: [
        '../../target/debug/lvqr serve',
        '--mesh-enabled',
        '--mesh-root-peer-count 1',
        // Session 142: cap each peer at one child so the three-peer
        // chain test forms deterministically.
        '--max-peers 1',
        '--no-auth-signal',
        `--admin-port ${MESH_ADMIN_PORT}`,
        '--hls-port 0',
        '--rtmp-port 11935',
        '--port 14443',
      ].join(' '),
      // /api/v1/mesh returns 200 JSON when --mesh-enabled; the
      // default `/` 404 would cause Playwright's url poller to
      // retry until the 30 s timeout.
      url: `http://127.0.0.1:${MESH_ADMIN_PORT}/api/v1/mesh`,
      reuseExistingServer: false,
      timeout: 30_000,
      stdout: 'pipe',
      stderr: 'pipe',
    },
    {
      // Session 153 dvr-player webServer profile: archive-dir +
      // configured DVR window so the live HLS endpoint walks back
      // 5 minutes of segments. --no-auth-live-playback removes the
      // bearer-token gate on /hls/* so the test does not need to
      // mint signed URLs.
      command: [
        '../../target/debug/lvqr serve',
        '--no-auth-signal',
        '--no-auth-live-playback',
        `--admin-port ${DVR_ADMIN_PORT}`,
        `--hls-port ${DVR_HLS_PORT}`,
        `--rtmp-port ${DVR_RTMP_PORT}`,
        `--port ${DVR_LVQR_PORT}`,
        '--hls-dvr-window 300',
        `--archive-dir ${DVR_ARCHIVE_DIR}`,
      ].join(' '),
      url: `http://127.0.0.1:${DVR_ADMIN_PORT}/healthz`,
      reuseExistingServer: false,
      timeout: 30_000,
      stdout: 'pipe',
      stderr: 'pipe',
    },
  ],
});
