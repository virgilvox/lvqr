// @lvqr/dvr-player mount + shadow-DOM E2E.
//
// Loads the compiled package dist + hls.js ESM bundle into a real
// Playwright Chromium page via routed importmap, then asserts the
// custom-element registration, shadow DOM structure, attribute
// reflection, and synthetic event flow. The Playwright config's
// `dvr-player` project provides a running `lvqr serve` with
// --archive-dir + --hls-dvr-window-secs=300 (an unused dependency
// here, but kept warm so the live-stream-driven follow-up suite
// can use the same harness without spinning a new server).
//
// What this covers:
//   1. Bundle loads (importmap resolution + module graph).
//   2. <lvqr-dvr-player> registers as a custom element.
//   3. Shadow DOM contains the expected parts: seekbar, live-badge,
//      go-live-button, play-button, time-display, labels, preview.
//   4. Attribute reflection: setting `muted` sets the inner video's
//      `muted` property; setting `controls="native"` hides the
//      custom controls.
//   5. Public API: `seek(time)` mutates currentTime + dispatches
//      `lvqr-dvr-seek` with the expected detail shape.
//
// What this does NOT cover (deferred to the follow-up live-push
// suite, scheduled when the ffmpeg-driven push helper is wired):
//   * Real HLS playback against /hls/{broadcast}/master.m3u8.
//   * LIVE badge state transitions driven by real
//     `seekable.end - currentTime` deltas.
//   * Hover thumbnail strip against a real second hls.js instance.
//   * Drag interactions against a real seekable range.

import { test, expect, Route } from '@playwright/test';
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const PKG_DIST = resolve(__dirname, '../../../packages/dvr-player/dist');
const HLS_MJS = resolve(__dirname, '../../../node_modules/hls.js/dist/hls.mjs');

const TEST_HOST = 'http://127.0.0.1:18089';
const TEST_HTML_URL = `${TEST_HOST}/_lvqr_test_/index.html`;

function html(): string {
  return /*html*/ `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>lvqr-dvr-player mount test</title>
    <script type="importmap">
      {
        "imports": {
          "hls.js": "/_lvqr_test_/hls/hls.mjs"
        }
      }
    </script>
    <script type="module" src="/_lvqr_test_/pkg/index.js"></script>
  </head>
  <body>
    <lvqr-dvr-player id="player" muted></lvqr-dvr-player>
  </body>
</html>`;
}

test.beforeEach(async ({ page }) => {
  test.skip(!existsSync(HLS_MJS), 'hls.js ESM bundle not found in node_modules');

  await page.route('**/_lvqr_test_/index.html', (route: Route) => {
    void route.fulfill({ contentType: 'text/html', body: html() });
  });

  await page.route('**/_lvqr_test_/pkg/**', (route: Route) => {
    const url = new URL(route.request().url());
    const sub = url.pathname.replace(/^\/_lvqr_test_\/pkg\//, '');
    const file = resolve(PKG_DIST, sub);
    if (!existsSync(file)) {
      void route.fulfill({ status: 404, body: `not found: ${sub}` });
      return;
    }
    void route.fulfill({
      contentType: 'text/javascript',
      body: readFileSync(file, 'utf-8'),
    });
  });

  await page.route('**/_lvqr_test_/hls/**', (route: Route) => {
    void route.fulfill({
      contentType: 'text/javascript',
      body: readFileSync(HLS_MJS, 'utf-8'),
    });
  });
});

test.describe('@lvqr/dvr-player mount', () => {
  test('registers as a custom element and renders shadow DOM', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    const isDefined = await page.evaluate(() => customElements.get('lvqr-dvr-player') !== undefined);
    expect(isDefined).toBe(true);

    const partsPresent = await page.evaluate(() => {
      const el = document.querySelector('lvqr-dvr-player') as HTMLElement | null;
      const root = el?.shadowRoot;
      if (!root) return null;
      return {
        video: !!root.querySelector('video.main'),
        thumbVideo: !!root.querySelector('video.thumb'),
        seekbar: !!root.querySelector('.seekbar'),
        playedFill: !!root.querySelector('.seekbar .played'),
        bufferedFill: !!root.querySelector('.seekbar .buffered'),
        thumbHandle: !!root.querySelector('.seekbar .thumb'),
        labels: !!root.querySelector('.labels'),
        preview: !!root.querySelector('.preview'),
        liveBadge: !!root.querySelector('.live-badge'),
        goLiveBtn: !!root.querySelector('.go-live-btn'),
        playBtn: !!root.querySelector('.play-btn'),
        muteBtn: !!root.querySelector('.mute-btn'),
        timeDisplay: !!root.querySelector('.time-display'),
      };
    });

    expect(partsPresent).not.toBeNull();
    for (const [name, present] of Object.entries(partsPresent ?? {})) {
      expect(present, `shadow DOM missing part: ${name}`).toBe(true);
    }
  });

  test('reflects the `muted` attribute onto the inner video element', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    const initiallyMuted = await page.evaluate(() => {
      const el = document.querySelector('lvqr-dvr-player') as HTMLElement | null;
      const v = el?.shadowRoot?.querySelector('video.main') as HTMLVideoElement | null;
      return v?.muted ?? null;
    });
    expect(initiallyMuted).toBe(true);

    await page.evaluate(() => {
      document.querySelector('lvqr-dvr-player')?.removeAttribute('muted');
    });

    const unmuted = await page.evaluate(() => {
      const v = document
        .querySelector('lvqr-dvr-player')
        ?.shadowRoot?.querySelector('video.main') as HTMLVideoElement | null;
      return v?.muted ?? null;
    });
    expect(unmuted).toBe(false);
  });

  test('controls="native" hides the custom control overlay', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    await page.evaluate(() => {
      document.querySelector('lvqr-dvr-player')?.setAttribute('controls', 'native');
    });

    const hidden = await page.evaluate(() => {
      const root = document.querySelector('lvqr-dvr-player')?.shadowRoot;
      const controls = root?.querySelector('.controls') as HTMLElement | null;
      const liveOverlay = root?.querySelector('.live-overlay') as HTMLElement | null;
      const v = root?.querySelector('video.main') as HTMLVideoElement | null;
      return {
        controlsHidden: controls?.hidden ?? null,
        overlayHidden: liveOverlay?.hidden ?? null,
        videoHasControlsAttr: v?.hasAttribute('controls') ?? null,
      };
    });

    expect(hidden.controlsHidden).toBe(true);
    expect(hidden.overlayHidden).toBe(true);
    expect(hidden.videoHasControlsAttr).toBe(true);
  });

  test('seek() dispatches lvqr-dvr-seek with the expected detail shape', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    // Synthesize a seekable range on the video element so the
    // `seekable()` getter returns a usable range without needing
    // a real HLS stream attached.
    const result = await page.evaluate(async () => {
      const el = document.querySelector('lvqr-dvr-player') as HTMLElement & {
        seek: (t: number) => void;
      };
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;

      // Stub `seekable` to a fixed range.
      Object.defineProperty(v, 'seekable', {
        configurable: true,
        get(): TimeRanges {
          return {
            length: 1,
            start: () => 0,
            end: () => 100,
          } as unknown as TimeRanges;
        },
      });

      // Capture the next lvqr-dvr-seek event detail.
      const detail = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(42);
      });

      return { detail, currentTime: v.currentTime };
    });

    expect(result.currentTime).toBe(42);
    expect(result.detail).toEqual({
      fromTime: 0,
      toTime: 42,
      isLiveEdge: false,
      source: 'programmatic',
    });
  });
});
