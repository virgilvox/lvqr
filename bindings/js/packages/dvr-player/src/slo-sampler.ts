// SLO client-sample helpers for `@lvqr/dvr-player`.
//
// Computes glass-to-glass latency from the wall-clock anchor on the
// HLS playlist's `#EXT-X-PROGRAM-DATE-TIME` (surfaced by hls.js via
// the standard `HTMLVideoElement.getStartDate()` method) and posts
// the sample to the relay's `POST /api/v1/slo/client-sample` route
// (session 156 follow-up).
//
// The relay's server-side stamping captures `ingest_ms -> egress_ms`
// (the relay's own contribution to latency); the client's view of
// `ingest_ms -> render_ms` additionally captures network + buffer +
// decode latency. Both numbers are valuable; this helper supplies
// the second.

/**
 * Result of [`computeLatencyMs`].
 *
 * `null` means the latency could not be computed (no PDT anchor on
 * the playlist, currentTime is NaN, or the computed value is outside
 * the plausible window). Callers treat null as "skip this sample"
 * rather than as an error.
 */
export type LatencySample = {
  ingestTsMs: number;
  renderTsMs: number;
  latencyMs: number;
} | null;

/** Drop samples whose computed latency is outside this window
 * (clock-skew filter). Mirrors the relay-side cap on the
 * `POST /api/v1/slo/client-sample` route so a sample we'd push is
 * always one the server would accept. */
export const MAX_PLAUSIBLE_LATENCY_MS = 300_000;

/**
 * Subset of `HTMLVideoElement` the latency computation needs. The
 * `getStartDate` method is a standard `HTMLMediaElement` extension
 * (HLS spec, Safari + hls.js implement it) but is absent from
 * TypeScript's stock DOM lib, so we declare a structural type that
 * matches the runtime shape both Safari and hls.js expose.
 */
export interface VideoElementWithStartDate {
  /** Wall-clock anchor for `currentTime=0`, or `null` when the
   * playlist has no `#EXT-X-PROGRAM-DATE-TIME`. */
  getStartDate(): Date | null;
  currentTime: number;
}

/**
 * Compute the glass-to-glass latency for a video element backed by
 * an HLS playlist with `#EXT-X-PROGRAM-DATE-TIME` anchors.
 *
 * Uses `getStartDate()` (HLS-spec extension on `HTMLMediaElement`,
 * implemented by Safari + hls.js) to recover the wall-clock for
 * `currentTime=0`, then offsets by `currentTime` to get the
 * wall-clock the publisher stamped on the currently-playing frame.
 *
 * Returns `null` when the playlist has no PDT anchor (getStartDate
 * returns an invalid Date), when currentTime is NaN, or when the
 * computed latency is implausible (clock skew between publisher and
 * subscriber). Callers should treat null as "skip this sample".
 */
export function computeLatencyMs(
  videoEl: VideoElementWithStartDate,
  now: () => number = Date.now,
): LatencySample {
  let startDate: Date | null;
  try {
    startDate = videoEl.getStartDate();
  } catch {
    return null;
  }
  if (!startDate) return null;
  const startMs = startDate.getTime();
  if (!Number.isFinite(startMs)) return null;
  const currentTime = videoEl.currentTime;
  if (!Number.isFinite(currentTime) || currentTime < 0) return null;
  const ingestTsMs = Math.floor(startMs + currentTime * 1000);
  const renderTsMs = now();
  const latencyMs = renderTsMs - ingestTsMs;
  if (latencyMs < 0 || latencyMs > MAX_PLAUSIBLE_LATENCY_MS) return null;
  return { ingestTsMs, renderTsMs, latencyMs };
}

/**
 * Extract the broadcast key from a dvr-player `src` URL.
 *
 * The relay's HLS surface mounts each broadcast at
 * `/hls/<app>/<key>/master.m3u8`, e.g.
 * `https://relay.example.com:8080/hls/live/cam1/master.m3u8`. The
 * server-side `LatencyTracker` keys on the full broadcast name
 * `live/cam1`, so this helper recovers `<app>/<key>` from the URL.
 *
 * Returns `null` when the URL doesn't match the expected shape
 * (operator overrode the route prefix; the SLO push fall back to
 * "off" silently because there's no broadcast to anchor the sample
 * to).
 */
export function broadcastFromHlsSrc(src: string): string | null {
  let url: URL;
  try {
    // Accept both absolute and protocol-relative URLs; relative
    // paths land here too (`/hls/live/cam1/master.m3u8`) when the
    // operator embedded the dvr-player on the same origin as the
    // relay, in which case the URL constructor needs a base.
    url = new URL(src, 'http://placeholder.invalid');
  } catch {
    return null;
  }
  const match = url.pathname.match(/^\/hls\/([^/]+)\/([^/]+)\/(?:master\.m3u8|playlist\.m3u8)$/);
  if (!match) return null;
  return `${match[1]}/${match[2]}`;
}

/** JSON body shape for the `POST /api/v1/slo/client-sample` route.
 * Mirrors the server-side `ClientLatencySample` struct; field
 * names are snake_case to match Rust's serde default. */
export interface ClientSamplePayload {
  broadcast: string;
  transport: string;
  ingest_ts_ms: number;
  render_ts_ms: number;
}

export interface PushSampleOpts {
  /** Absolute URL of the relay's `POST /api/v1/slo/client-sample`
   * route. e.g. `https://relay.example.com:8080/api/v1/slo/client-sample`. */
  endpoint: string;
  /** Broadcast key (e.g. `live/cam1`); used by the server-side
   * Subscribe-context auth check. */
  broadcast: string;
  /** Transport label rendered in the `transport` metric label.
   * For dvr-player this is always `"hls"`. */
  transport: string;
  ingestTsMs: number;
  renderTsMs: number;
  /** Optional bearer token. Sent as `Authorization: Bearer <token>`.
   * The dvr-player's existing `token` attribute supplies this so a
   * subscribe-token-bearing playback session pushes samples gated
   * on that token via the dual-auth path. */
  token?: string;
  /** Optional fetch implementation, primarily for tests. Default:
   * `globalThis.fetch`. */
  fetchImpl?: typeof fetch;
}

/**
 * POST a single latency sample to the relay's SLO endpoint.
 *
 * Best-effort: returns `true` on a 2xx response, `false` on any
 * failure (network error, validation rejection, auth failure). The
 * caller is expected to log + continue rather than treat a failed
 * push as a fatal error -- SLO sampling must never disrupt
 * playback.
 */
export async function pushSample(opts: PushSampleOpts): Promise<boolean> {
  const fetchImpl = opts.fetchImpl ?? globalThis.fetch;
  const body: ClientSamplePayload = {
    broadcast: opts.broadcast,
    transport: opts.transport,
    ingest_ts_ms: opts.ingestTsMs,
    render_ts_ms: opts.renderTsMs,
  };
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (opts.token) headers.Authorization = `Bearer ${opts.token}`;
  try {
    const res = await fetchImpl(opts.endpoint, {
      method: 'POST',
      headers,
      body: JSON.stringify(body),
    });
    return res.ok;
  } catch {
    return false;
  }
}
