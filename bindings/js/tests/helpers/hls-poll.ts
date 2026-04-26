// Live HLS playlist polling helpers (session 155).
//
// Used by the live-RTMP Playwright tests in
// `bindings/js/tests/e2e/dvr-player/markers.spec.ts` to wait for
// the relay to serve a non-empty variant playlist BEFORE setting
// `src` on the dvr-player. Without this pre-check hls.js's first
// variant fetch races the relay's segment-finalize window and
// fires a fatal `manifestLoadError` on the first attempt.
//
// All helpers are pure Node-side `fetch` + regex; no browser
// context needed (mirrors the Node-side polling pattern in
// markers.spec.ts's existing live-RTMP test).

/** A successful variant-playlist probe. */
export interface LiveVariantInfo {
  /** Absolute URL of the master playlist that was probed. */
  masterUrl: string;
  /** Absolute URL of the first variant playlist resolved from the master. */
  variantUrl: string;
  /** Latest variant playlist body. */
  variantBody: string;
  /** Number of `#EXTINF` entries in the variant body. */
  extinfCount: number;
}

export interface WaitForLiveVariantPlaylistOptions {
  /** Master playlist URL (e.g. `http://127.0.0.1:18190/hls/live/dvr-test/master.m3u8`). */
  masterUrl: string;
  /** Total wait budget in ms (master + variant combined). Default 90_000. */
  timeoutMs?: number;
  /** Minimum `#EXTINF` count required in the variant body. Default 2. */
  minExtinfCount?: number;
  /** Poll interval in ms. Default 500. */
  pollIntervalMs?: number;
}

/**
 * Poll the master playlist until it carries `#EXT-X-STREAM-INF` +
 * a variant URI, then poll that variant playlist until it carries
 * at least `minExtinfCount` `#EXTINF` entries. Resolves with both
 * URLs + the latest variant body.
 *
 * Throws (rejects) when the deadline elapses without the conditions
 * being met. The error message includes the last response status
 * and a body excerpt so the failing Playwright assertion is
 * actionable.
 */
export async function waitForLiveVariantPlaylist(opts: WaitForLiveVariantPlaylistOptions): Promise<LiveVariantInfo> {
  const timeoutMs = opts.timeoutMs ?? 90_000;
  const minExtinfCount = opts.minExtinfCount ?? 2;
  const pollIntervalMs = opts.pollIntervalMs ?? 500;
  const start = Date.now();

  const masterUrl = opts.masterUrl;
  let lastMasterStatus = 0;
  let lastMasterBody = '';
  let variantUrl: string | null = null;

  // Phase 1: master playlist becomes readable + carries a variant URI.
  while (Date.now() - start < timeoutMs) {
    try {
      const r = await fetch(masterUrl, { cache: 'no-store' });
      lastMasterStatus = r.status;
      lastMasterBody = await r.text();
      if (r.ok && lastMasterBody.includes('#EXT-X-STREAM-INF')) {
        variantUrl = resolveFirstVariantUri(masterUrl, lastMasterBody);
        if (variantUrl) break;
      }
    } catch {
      // keep polling
    }
    await sleep(pollIntervalMs);
  }
  if (!variantUrl) {
    throw new Error(
      `waitForLiveVariantPlaylist: master never carried #EXT-X-STREAM-INF + variant URI within ${timeoutMs}ms.\n` +
        `last status: ${lastMasterStatus}\nlast body:\n${lastMasterBody}`,
    );
  }

  // Phase 2: variant playlist carries at least `minExtinfCount` segments.
  let lastVariantStatus = 0;
  let lastVariantBody = '';
  while (Date.now() - start < timeoutMs) {
    try {
      const r = await fetch(variantUrl, { cache: 'no-store' });
      lastVariantStatus = r.status;
      lastVariantBody = await r.text();
      const extinfCount = countOccurrences(lastVariantBody, '#EXTINF:');
      if (r.ok && extinfCount >= minExtinfCount) {
        return {
          masterUrl,
          variantUrl,
          variantBody: lastVariantBody,
          extinfCount,
        };
      }
    } catch {
      // keep polling
    }
    await sleep(pollIntervalMs);
  }
  throw new Error(
    `waitForLiveVariantPlaylist: variant playlist never carried >= ${minExtinfCount} #EXTINF entries within ${timeoutMs}ms.\n` +
      `variant url: ${variantUrl}\nlast status: ${lastVariantStatus}\nlast body:\n${lastVariantBody}`,
  );
}

/** Resolve the first variant URI in a master playlist body relative to the master URL. */
function resolveFirstVariantUri(masterUrl: string, masterBody: string): string | null {
  const lines = masterBody.split(/\r?\n/);
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].startsWith('#EXT-X-STREAM-INF')) {
      // The next non-blank, non-comment line is the variant URI.
      for (let j = i + 1; j < lines.length; j++) {
        const line = lines[j].trim();
        if (line.length === 0 || line.startsWith('#')) continue;
        return new URL(line, masterUrl).toString();
      }
    }
  }
  return null;
}

function countOccurrences(haystack: string, needle: string): number {
  if (needle.length === 0) return 0;
  let count = 0;
  let idx = 0;
  while ((idx = haystack.indexOf(needle, idx)) !== -1) {
    count += 1;
    idx += needle.length;
  }
  return count;
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

/** Path to the `scte35-rtmp-push` bin in the workspace target dir. */
export function scte35RtmpPushBinPath(): string {
  // The bin lands at `target/debug/scte35-rtmp-push` after
  // `cargo build -p lvqr-test-utils --bins`. This helper file lives at
  // `bindings/js/tests/helpers/hls-poll.ts`, so the workspace root is
  // four levels up. The path matches `playwright.config.ts`'s
  // assumption that `../../target/debug/lvqr` is the relay binary
  // (resolved from `bindings/js/playwright.config.ts`, two levels up).
  return require('node:path').resolve(__dirname, '../../../../target/debug/scte35-rtmp-push');
}
