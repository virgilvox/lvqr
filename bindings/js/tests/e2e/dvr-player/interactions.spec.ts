// @lvqr/dvr-player interaction E2E.
//
// Covers public-API + DOM-driven behaviour beyond the basic mount
// asserted in mount.spec.ts. The harness is the same -- compiled
// dist + hls.js ESM bundle loaded via routed importmap -- so the
// component runs in a real Chromium with no relay-side push, and
// each test synthesises the inner video element's `seekable` /
// `currentTime` / etc. to drive the component through state
// transitions.
//
// What this covers:
//   * goLive() jumps to seekable.end and dispatches lvqr-dvr-seek
//     with isLiveEdge: true + source: 'user'.
//   * seek() clamps inputs outside [seekable.start, seekable.end].
//   * Multiple programmatic seeks fire multiple events, each with
//     the previous currentTime as fromTime.
//   * Keyboard navigation: ArrowLeft/Right scrub +/- 5s; Home /
//     End jump to range endpoints.
//   * live-edge-threshold-secs attribute customisation drives the
//     isLiveEdge classification on programmatic seek.
//   * controls="custom" toggle restores the custom UI after a
//     prior controls="native" set.
//   * Pointer drag (pointerdown + pointermove + pointerup) on the
//     seek bar updates currentTime and fires lvqr-dvr-seek with
//     source: 'user'.
//   * Hover pointermove shows the preview overlay; pointerleave
//     hides it.
//   * getHlsInstance() returns null before any src is set.
//   * The lvqr-dvr-seek event bubbles past the host element into
//     the document, proving `bubbles: true` is honoured against
//     a real DOM (the unit spec verifies the flag; this verifies
//     the propagation).

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
    <title>lvqr-dvr-player interaction test</title>
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

// Helper that mounts the component, synthesises a seekable range
// + initial currentTime on the inner video element, and exposes
// utility hooks on `window` for the test body to drive.
async function setupSyntheticVideo(page: import('@playwright/test').Page, opts: {
  start: number;
  end: number;
  currentTime: number;
}) {
  await page.goto(TEST_HTML_URL);
  await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);
  await page.evaluate((opts) => {
    const el = document.querySelector('lvqr-dvr-player') as HTMLElement;
    // Force a layout-measurable size so getBoundingClientRect on
    // the inner seek bar returns a non-zero width for pointer-
    // drag arithmetic. Without this the host width depends on
    // body's auto width, which is fine in normal browsers but
    // brittle across viewport-policy quirks.
    el.style.display = 'block';
    el.style.width = '600px';
    el.style.height = '400px';
    const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;
    Object.defineProperty(v, 'seekable', {
      configurable: true,
      get(): TimeRanges {
        return {
          length: 1,
          start: () => opts.start,
          end: () => opts.end,
        } as unknown as TimeRanges;
      },
    });
    v.currentTime = opts.currentTime;
    (window as unknown as { __player: HTMLElement }).__player = el;
  }, opts);
}

test.describe('@lvqr/dvr-player interactions', () => {
  test('goLive() jumps to seekable.end and dispatches user-source lvqr-dvr-seek', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 30 });

    const result = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { goLive: () => void } }).__player;
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;
      const detail = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.goLive();
      });
      return { detail, currentTime: v.currentTime };
    });

    expect(result.currentTime).toBe(100);
    expect(result.detail).toEqual({
      fromTime: 30,
      toTime: 100,
      isLiveEdge: true,
      source: 'user',
    });
  });

  test('seek() clamps below seekable.start and above seekable.end', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 50, end: 150, currentTime: 100 });

    const result = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { seek: (t: number) => void } }).__player;
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;
      const before = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(0); // below range
      });
      const t1 = v.currentTime;

      const after = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(9999); // above range
      });
      const t2 = v.currentTime;

      return { before, after, t1, t2 };
    });

    expect(result.t1).toBe(50);
    expect((result.before as { toTime: number }).toTime).toBe(50);
    expect(result.t2).toBe(150);
    expect((result.after as { toTime: number }).toTime).toBe(150);
  });

  test('multiple programmatic seeks fire multiple events with chained fromTime', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 0 });

    const events = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { seek: (t: number) => void } }).__player;
      const captured: Array<{ fromTime: number; toTime: number }> = [];
      el.addEventListener('lvqr-dvr-seek', (e: Event) => {
        const d = (e as CustomEvent).detail as { fromTime: number; toTime: number };
        captured.push({ fromTime: d.fromTime, toTime: d.toTime });
      });
      el.seek(20);
      el.seek(60);
      el.seek(80);
      return captured;
    });

    expect(events).toEqual([
      { fromTime: 0, toTime: 20 },
      { fromTime: 20, toTime: 60 },
      { fromTime: 60, toTime: 80 },
    ]);
  });

  test('keyboard ArrowLeft / ArrowRight scrubs by 5 seconds', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 50 });

    const after = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement }).__player;
      const seekbar = el.shadowRoot?.querySelector('.seekbar') as HTMLElement;
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;

      const events: Array<{ fromTime: number; toTime: number }> = [];
      el.addEventListener('lvqr-dvr-seek', (e: Event) => {
        const d = (e as CustomEvent).detail as { fromTime: number; toTime: number };
        events.push({ fromTime: d.fromTime, toTime: d.toTime });
      });

      seekbar.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowLeft', bubbles: true }));
      const afterLeft = v.currentTime;
      seekbar.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
      const afterRight = v.currentTime;
      return { afterLeft, afterRight, events };
    });

    expect(after.afterLeft).toBe(45);
    expect(after.afterRight).toBe(50);
    expect(after.events.length).toBe(2);
  });

  test('keyboard Home / End jumps to the range endpoints', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 100, end: 200, currentTime: 150 });

    const after = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement }).__player;
      const seekbar = el.shadowRoot?.querySelector('.seekbar') as HTMLElement;
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;

      seekbar.dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', bubbles: true }));
      const afterHome = v.currentTime;
      seekbar.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));
      const afterEnd = v.currentTime;
      return { afterHome, afterEnd };
    });

    expect(after.afterHome).toBe(100);
    expect(after.afterEnd).toBe(200);
  });

  test('live-edge-threshold-secs attribute drives the isLiveEdge classification', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 50 });

    // Default threshold (6s) -- a seek to t=98 (delta 2) is live;
    // a seek to t=92 (delta 8) is NOT live.
    const dflt = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { seek: (t: number) => void } }).__player;
      const detailA = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(98);
      });
      const detailB = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(92);
      });
      return { isLiveA: (detailA as { isLiveEdge: boolean }).isLiveEdge, isLiveB: (detailB as { isLiveEdge: boolean }).isLiveEdge };
    });
    expect(dflt.isLiveA).toBe(true);
    expect(dflt.isLiveB).toBe(false);

    // Custom threshold of 30s -- t=85 (delta 15) is live, t=50 is NOT.
    const custom = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { seek: (t: number) => void } }).__player;
      el.setAttribute('live-edge-threshold-secs', '30');
      const detailA = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(85);
      });
      const detailB = await new Promise<Record<string, unknown>>((resolve) => {
        el.addEventListener(
          'lvqr-dvr-seek',
          (e: Event) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(50);
      });
      return { isLiveA: (detailA as { isLiveEdge: boolean }).isLiveEdge, isLiveB: (detailB as { isLiveEdge: boolean }).isLiveEdge };
    });
    expect(custom.isLiveA).toBe(true);
    expect(custom.isLiveB).toBe(false);
  });

  test('controls="custom" toggle restores the custom UI after a prior native set', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    const states = await page.evaluate(() => {
      const el = document.querySelector('lvqr-dvr-player') as HTMLElement;
      const root = el.shadowRoot;
      const controls = root?.querySelector('.controls') as HTMLElement;
      const liveOverlay = root?.querySelector('.live-overlay') as HTMLElement;
      const v = root?.querySelector('video.main') as HTMLVideoElement;

      el.setAttribute('controls', 'native');
      const native = {
        controlsHidden: controls.hidden,
        overlayHidden: liveOverlay.hidden,
        videoControls: v.hasAttribute('controls'),
      };

      el.setAttribute('controls', 'custom');
      const custom = {
        controlsHidden: controls.hidden,
        overlayHidden: liveOverlay.hidden,
        videoControls: v.hasAttribute('controls'),
      };

      el.removeAttribute('controls');
      const dflt = {
        controlsHidden: controls.hidden,
        overlayHidden: liveOverlay.hidden,
        videoControls: v.hasAttribute('controls'),
      };

      return { native, custom, dflt };
    });

    expect(states.native).toEqual({ controlsHidden: true, overlayHidden: true, videoControls: true });
    expect(states.custom).toEqual({ controlsHidden: false, overlayHidden: false, videoControls: false });
    expect(states.dflt).toEqual({ controlsHidden: false, overlayHidden: false, videoControls: false });
  });

  test('pointer drag on seek bar updates currentTime and fires user-source lvqr-dvr-seek', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 0 });

    const result = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement }).__player;
      const seekbar = el.shadowRoot?.querySelector('.seekbar') as HTMLElement;
      const v = el.shadowRoot?.querySelector('video.main') as HTMLVideoElement;

      const rect = seekbar.getBoundingClientRect();
      const events: Array<{ source: string; toTime: number }> = [];
      el.addEventListener('lvqr-dvr-seek', (e: Event) => {
        const d = (e as CustomEvent).detail as { source: string; toTime: number };
        events.push({ source: d.source, toTime: d.toTime });
      });

      // pointerdown at 25% of the bar.
      const x25 = rect.left + rect.width * 0.25;
      const x75 = rect.left + rect.width * 0.75;
      seekbar.dispatchEvent(
        new PointerEvent('pointerdown', { clientX: x25, clientY: rect.top + rect.height / 2, button: 0, pointerId: 1, bubbles: true }),
      );
      const t1 = v.currentTime;

      // drag to 75%.
      seekbar.dispatchEvent(
        new PointerEvent('pointermove', { clientX: x75, clientY: rect.top + rect.height / 2, pointerId: 1, bubbles: true }),
      );
      const t2 = v.currentTime;

      // release.
      seekbar.dispatchEvent(
        new PointerEvent('pointerup', { clientX: x75, clientY: rect.top + rect.height / 2, pointerId: 1, bubbles: true }),
      );

      return { t1, t2, events };
    });

    // Allow ~1s of float drift from getBoundingClientRect rounding.
    expect(result.t1).toBeGreaterThanOrEqual(24);
    expect(result.t1).toBeLessThanOrEqual(26);
    expect(result.t2).toBeGreaterThanOrEqual(74);
    expect(result.t2).toBeLessThanOrEqual(76);
    // pointerdown -> seek; pointermove (with drag) -> seek; pointerup -> seek
    expect(result.events.length).toBeGreaterThanOrEqual(2);
    for (const e of result.events) {
      expect(e.source).toBe('user');
    }
  });

  test('hover pointermove shows the preview overlay; pointerleave hides it', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 0 });

    const states = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement }).__player;
      const seekbar = el.shadowRoot?.querySelector('.seekbar') as HTMLElement;
      const preview = el.shadowRoot?.querySelector('.preview') as HTMLElement;

      const rect = seekbar.getBoundingClientRect();
      const x = rect.left + rect.width * 0.5;
      const y = rect.top + rect.height / 2;

      seekbar.dispatchEvent(
        new PointerEvent('pointermove', { clientX: x, clientY: y, pointerId: 99, bubbles: true }),
      );
      const onMove = preview.classList.contains('is-visible');

      seekbar.dispatchEvent(new PointerEvent('pointerleave', { pointerId: 99, bubbles: true }));
      const onLeave = preview.classList.contains('is-visible');

      return { onMove, onLeave };
    });

    expect(states.onMove).toBe(true);
    expect(states.onLeave).toBe(false);
  });

  test('getHlsInstance() returns null before any src is set', async ({ page }) => {
    await page.goto(TEST_HTML_URL);
    await page.waitForFunction(() => customElements.get('lvqr-dvr-player') !== undefined);

    const isNull = await page.evaluate(() => {
      const el = document.querySelector('lvqr-dvr-player') as HTMLElement & { getHlsInstance: () => unknown };
      return el.getHlsInstance() === null;
    });
    expect(isNull).toBe(true);
  });

  test('lvqr-dvr-seek bubbles past the host element to the document', async ({ page }) => {
    await setupSyntheticVideo(page, { start: 0, end: 100, currentTime: 0 });

    const detail = await page.evaluate(async () => {
      const el = (window as unknown as { __player: HTMLElement & { seek: (t: number) => void } }).__player;
      return await new Promise<Record<string, unknown>>((resolve) => {
        document.addEventListener(
          'lvqr-dvr-seek',
          (e) => resolve((e as CustomEvent).detail as Record<string, unknown>),
          { once: true },
        );
        el.seek(42);
      });
    });

    expect(detail).toEqual({
      fromTime: 0,
      toTime: 42,
      isLiveEdge: false,
      source: 'programmatic',
    });
  });
});
