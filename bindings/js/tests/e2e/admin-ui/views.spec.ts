// admin-ui view-render smoke tests (console feature-completeness wave).
//
// Mounts the built @lvqr/admin-ui SPA (served by `vite preview`, see
// playwright.config.ts) and route-mocks every `/api/v1/*` endpoint so the
// views render against deterministic data with no live relay. Seeds a
// connection profile in localStorage so the app has an active client.
//
// The point is to lock in the views the buildout wave made real
// (StreamDetail, Transcode, Agents, Recordings, Logs) at the browser level.
// We only fulfill mocked responses here (no upstream to proxy), so each
// route handler calls `route.fulfill` exactly once.

import { test, expect, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';

const STORAGE_KEY = 'lvqr.admin.connection.v1';
const BASE = 'http://relay.test:8080';

const MOCKS: Record<string, unknown> = {
  '/api/v1/stats': { publishers: 1, subscribers: 2, tracks: 3, bytes_received: 0, bytes_sent: 0, uptime_secs: 42 },
  '/api/v1/streams': [{ name: 'live/demo', subscribers: 2 }],
  '/api/v1/streams/live/demo': {
    name: 'live/demo',
    subscribers: 2,
    tracks: [
      { track: '0.mp4', kind: 'video', codec: 'avc1.640028', timescale: 90000, fragments: 120, subscribers: 2, lagged_skips: 0 },
      { track: '1.mp4', kind: 'audio', codec: 'mp4a.40.2', timescale: 48000, fragments: 240, subscribers: 2, lagged_skips: 0 },
    ],
  },
  '/api/v1/slo': { broadcasts: [] },
  '/api/v1/mesh': { enabled: false, peer_count: 0, offload_percentage: 0, peers: [] },
  '/api/v1/transcode/ladders': {
    enabled: true,
    encoder: 'software',
    renditions: [{ name: '720p', width: 1280, height: 720, video_bitrate_kbps: 2500, audio_bitrate_kbps: 128 }],
    active: [],
  },
  '/api/v1/agents': {
    enabled: true,
    agents: [{ name: 'captions', kind: 'captions', model: '/models/ggml-tiny.en.bin', window_ms: 5000 }],
    active: [],
  },
  '/api/v1/archive': {
    enabled: true,
    recordings: [
      {
        broadcast: 'live/demo',
        segment_count: 3,
        total_bytes: 6144,
        duration_secs: 6,
        tracks: [{ track: '0.mp4', segment_count: 3, total_bytes: 6144, duration_secs: 6, timescale: 90000 }],
      },
    ],
  },
  '/api/v1/wasm-filter': { enabled: false, chain_length: 0, broadcasts: [], slots: [] },
  '/api/v1/ingest': {
    listeners: [
      { protocol: 'rtmp', addr: '0.0.0.0:1935', enabled: true },
      { protocol: 'whip', addr: '0.0.0.0:8443', enabled: false },
    ],
  },
  '/api/v1/server-info': {
    version: '1.0.0',
    build_features: ['rtmp'],
    uptime_secs: 42,
    bound: { admin: '0.0.0.0:8080', rtmp: '0.0.0.0:1935', srt: null, rtsp: null, whip: '0.0.0.0:8443' },
    features: { mesh_enabled: false, cluster_enabled: false, wasm_filter_chain_length: 0, auth_mode: 'noop', hmac_playback_secret_configured: false, stream_keys_enabled: true },
    config_path: null,
    wasm_filter_paths: [],
  },
  '/api/v1/config-reload': { config_path: null, last_reload_at_ms: null, last_reload_kind: null, applied_keys: [], warnings: [] },
  '/api/v1/cluster/nodes': [],
  '/api/v1/cluster/broadcasts': [],
  '/api/v1/cluster/config': [],
  '/api/v1/cluster/federation': { links: [] },
  '/api/v1/streamkeys': [],
};

async function setup(page: Page) {
  await page.addInitScript(
    ([key, base]) => {
      localStorage.setItem(
        key,
        JSON.stringify({ profiles: [{ id: 'cp-e2e', label: 'e2e', baseUrl: base }], activeId: 'cp-e2e' }),
      );
    },
    [STORAGE_KEY, BASE],
  );

  await page.route('**/api/v1/**', (route) => {
    // Decode so the encoded `streams/live%2Fdemo` path matches the mock key.
    const path = decodeURIComponent(new URL(route.request().url()).pathname);
    // The log tail is an EventSource stream; hand back a tiny event-stream so
    // the viewer connects rather than erroring.
    if (path === '/api/v1/logs') {
      void route.fulfill({
        status: 200,
        contentType: 'text/event-stream',
        body: `data: ${JSON.stringify({ ts_ms: Date.now(), level: 'INFO', target: 'lvqr', message: 'e2e tail line' })}\n\n`,
      });
      return;
    }
    const body = MOCKS[path] ?? {};
    void route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });
  await page.route('**/metrics', (route) => route.fulfill({ status: 200, contentType: 'text/plain', body: '' }));
  await page.route('**/app-config.json', (route) => route.fulfill({ status: 404, body: '' }));
}

test.beforeEach(async ({ page }) => {
  await setup(page);
});

test('dashboard mounts with an active connection', async ({ page }) => {
  await page.goto('/#/');
  // The rail nav is always present once the app mounts.
  await expect(page.getByText('Operations', { exact: false }).first()).toBeVisible();
});

test('transcode view renders the configured ladder + add form', async ({ page }) => {
  await page.goto('/#/transcode');
  await expect(page.getByText('Configured renditions')).toBeVisible();
  await expect(page.getByText('720p').first()).toBeVisible();
  await expect(page.getByText('ADD RENDITION')).toBeVisible();
});

test('agents view renders the running agent + start form', async ({ page }) => {
  await page.goto('/#/agents');
  await expect(page.getByText('captions').first()).toBeVisible();
  await expect(page.getByText('/models/ggml-tiny.en.bin')).toBeVisible();
  await expect(page.getByText('START AGENT')).toBeVisible();
});

test('recordings view lists archived broadcasts with a scrubber link', async ({ page }) => {
  await page.goto('/#/recordings');
  await expect(page.getByText('live/demo').first()).toBeVisible();
  await expect(page.getByRole('link', { name: /scrubber/i })).toBeVisible();
});

test('stream detail renders the per-track table', async ({ page }) => {
  await page.goto('/#/streams/live%2Fdemo');
  await expect(page.getByRole('heading', { name: 'Tracks' })).toBeVisible();
  await expect(page.getByText('0.mp4').first()).toBeVisible();
  await expect(page.getByText('1.mp4').first()).toBeVisible();
});

test('ingest view lists bound listeners with a Stop control for each enabled row', async ({ page }) => {
  await page.goto('/#/ingest');
  await expect(page.getByRole('heading', { name: 'Ingest listeners' })).toBeVisible();
  // Addresses come from the live registry endpoint (`/api/v1/ingest`), not the
  // static server-info bound block.
  await expect(page.getByText('0.0.0.0:1935')).toBeVisible(); // rtmp enabled
  await expect(page.getByText('0.0.0.0:8443')).toBeVisible(); // whip stopped
  await expect(page.getByText('lvqr 1.0.0')).toBeVisible();
  // The enabled row gets a Stop button; the stopped row does not.
  await expect(page.getByRole('button', { name: 'Stop RTMP listener' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Stop WHIP listener' })).toHaveCount(0);
});

test('logs view renders the live-tail shell with level filters', async ({ page }) => {
  await page.goto('/#/logs');
  await expect(page.getByText('tail.', { exact: false })).toBeVisible();
  await expect(page.getByRole('button', { name: 'INFO' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'ERROR' })).toBeVisible();
});

// Accessibility: no axe violations across every view. color-contrast is
// excluded -- the palette is lifted verbatim from the design system
// (tokens.css), so contrast is an operator design-token decision, not a
// code defect.
const A11Y_ROUTES = [
  '/#/',
  '/#/streams',
  '/#/streams/live%2Fdemo',
  '/#/recordings',
  '/#/dvr',
  '/#/ingest',
  '/#/filters',
  '/#/transcode',
  '/#/agents',
  '/#/egress',
  '/#/cluster',
  '/#/mesh',
  '/#/auth',
  '/#/provenance',
  '/#/observability',
  '/#/logs',
  '/#/settings',
];
for (const route of A11Y_ROUTES) {
  test(`a11y: ${route} has no axe violations`, async ({ page }) => {
    await page.goto(route);
    await page.waitForTimeout(250);
    const results = await new AxeBuilder({ page }).disableRules(['color-contrast']).analyze();
    expect(results.violations, JSON.stringify(results.violations.map((v) => `${v.id}@${route}`))).toEqual([]);
  });
}

// Responsive: no horizontal page scroll at a 390px mobile viewport across
// every view (the rail collapses to a drawer at the 1023px breakpoint).
test('responsive: no horizontal overflow at 390px', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  for (const route of A11Y_ROUTES) {
    await page.goto(route);
    await page.waitForTimeout(150);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow, `horizontal overflow on ${route}`).toBeLessThanOrEqual(1);
  }
});
