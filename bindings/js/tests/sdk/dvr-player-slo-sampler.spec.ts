// Unit tests for the @lvqr/dvr-player SLO client-sample helpers
// (session 156 follow-up). Pure functions; no DOM, no hls.js, no
// real fetch.

import { describe, expect, it, vi } from 'vitest';
import {
  broadcastFromHlsSrc,
  computeLatencyMs,
  pushSample,
  MAX_PLAUSIBLE_LATENCY_MS,
} from '../../packages/dvr-player/src/slo-sampler.js';

function videoStub(currentTime: number, startDate: Date | null): {
  getStartDate(): Date | null;
  currentTime: number;
} {
  return {
    getStartDate: () => startDate,
    currentTime,
  };
}

describe('computeLatencyMs', () => {
  it('returns the wall-clock delta between getStartDate+currentTime and now', () => {
    const startDate = new Date('2026-04-26T00:00:00Z');
    const v = videoStub(10, startDate); // 10 s into the stream
    // ingestTsMs = startDate + 10s
    // now = startDate + 10s + 250ms -> latency = 250
    const latency = computeLatencyMs(v, () => startDate.getTime() + 10_000 + 250);
    expect(latency).not.toBeNull();
    expect(latency!.latencyMs).toBe(250);
    expect(latency!.ingestTsMs).toBe(startDate.getTime() + 10_000);
    expect(latency!.renderTsMs).toBe(startDate.getTime() + 10_000 + 250);
  });

  it('returns null when the playlist has no PDT anchor (getStartDate returns null)', () => {
    const v = videoStub(10, null);
    expect(computeLatencyMs(v, () => 1000)).toBeNull();
  });

  it('returns null when getStartDate throws (older browser fallback)', () => {
    const v = {
      get currentTime() {
        return 10;
      },
      getStartDate(): Date | null {
        throw new Error('unsupported');
      },
    };
    expect(computeLatencyMs(v, () => 1000)).toBeNull();
  });

  it('returns null when currentTime is NaN', () => {
    const startDate = new Date('2026-04-26T00:00:00Z');
    const v = videoStub(Number.NaN, startDate);
    expect(computeLatencyMs(v, () => startDate.getTime() + 250)).toBeNull();
  });

  it('returns null when startDate is invalid (NaN time)', () => {
    const v = videoStub(10, new Date('not-a-date'));
    expect(computeLatencyMs(v, () => 1000)).toBeNull();
  });

  it('returns null on negative latency (clock skew: render before ingest)', () => {
    const startDate = new Date('2026-04-26T00:00:00Z');
    const v = videoStub(10, startDate);
    // now = before startDate+10s -> negative latency
    expect(computeLatencyMs(v, () => startDate.getTime() + 5_000)).toBeNull();
  });

  it('returns null on implausibly large latency (filters bad samples)', () => {
    const startDate = new Date('2026-04-26T00:00:00Z');
    const v = videoStub(0, startDate);
    // 10 minutes after ingest = past the cap
    expect(
      computeLatencyMs(v, () => startDate.getTime() + MAX_PLAUSIBLE_LATENCY_MS + 1),
    ).toBeNull();
    // Right at the cap is accepted (boundary inclusive).
    const ok = computeLatencyMs(v, () => startDate.getTime() + MAX_PLAUSIBLE_LATENCY_MS);
    expect(ok).not.toBeNull();
    expect(ok!.latencyMs).toBe(MAX_PLAUSIBLE_LATENCY_MS);
  });
});

describe('broadcastFromHlsSrc', () => {
  it('extracts <app>/<key> from a master.m3u8 URL', () => {
    expect(broadcastFromHlsSrc('https://relay.example.com:8080/hls/live/cam1/master.m3u8')).toBe('live/cam1');
  });

  it('extracts <app>/<key> from a playlist.m3u8 URL', () => {
    expect(broadcastFromHlsSrc('http://127.0.0.1:18190/hls/live/dvr-test/playlist.m3u8')).toBe('live/dvr-test');
  });

  it('extracts from path-relative URLs', () => {
    expect(broadcastFromHlsSrc('/hls/live/cam1/master.m3u8')).toBe('live/cam1');
  });

  it('returns null for non-matching paths', () => {
    expect(broadcastFromHlsSrc('https://example.com/other/path')).toBeNull();
    expect(broadcastFromHlsSrc('https://example.com/hls/just-app/master.m3u8')).toBeNull();
    expect(broadcastFromHlsSrc('https://example.com/hls/live/cam1/index.m3u8')).toBeNull();
  });

  it('returns null on invalid URL', () => {
    expect(broadcastFromHlsSrc('::not a url::')).toBeNull();
  });
});

describe('pushSample', () => {
  it('POSTs JSON with snake_case fields the server expects', async () => {
    const fetchImpl = vi.fn(async (_url: string | URL | Request, _init?: RequestInit) => {
      return new Response(null, { status: 204 });
    });
    const ok = await pushSample({
      endpoint: 'https://relay.example.com:8080/api/v1/slo/client-sample',
      broadcast: 'live/cam1',
      transport: 'hls',
      ingestTsMs: 1_000,
      renderTsMs: 1_120,
      token: 'subtoken',
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    expect(ok).toBe(true);
    expect(fetchImpl).toHaveBeenCalledTimes(1);
    const [url, init] = fetchImpl.mock.calls[0];
    expect(url).toBe('https://relay.example.com:8080/api/v1/slo/client-sample');
    expect(init?.method).toBe('POST');
    const headers = init?.headers as Record<string, string>;
    expect(headers['Content-Type']).toBe('application/json');
    expect(headers.Authorization).toBe('Bearer subtoken');
    const body = JSON.parse(init?.body as string);
    expect(body).toEqual({
      broadcast: 'live/cam1',
      transport: 'hls',
      ingest_ts_ms: 1_000,
      render_ts_ms: 1_120,
    });
  });

  it('omits the Authorization header when no token is supplied', async () => {
    const fetchImpl = vi.fn(async () => new Response(null, { status: 204 }));
    await pushSample({
      endpoint: 'https://relay.example.com/api/v1/slo/client-sample',
      broadcast: 'live/cam1',
      transport: 'hls',
      ingestTsMs: 1_000,
      renderTsMs: 1_120,
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    const [, init] = fetchImpl.mock.calls[0];
    const headers = init?.headers as Record<string, string>;
    expect(headers.Authorization).toBeUndefined();
  });

  it('returns false on non-2xx (server validation rejection)', async () => {
    const fetchImpl = vi.fn(async () => new Response('{"error":"bogus"}', { status: 400 }));
    const ok = await pushSample({
      endpoint: 'https://relay.example.com/api/v1/slo/client-sample',
      broadcast: 'live/cam1',
      transport: 'hls',
      ingestTsMs: 1_000,
      renderTsMs: 1_120,
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    expect(ok).toBe(false);
  });

  it('returns false on network error (best-effort: never throws)', async () => {
    const fetchImpl = vi.fn(async () => {
      throw new Error('net down');
    });
    const ok = await pushSample({
      endpoint: 'https://relay.example.com/api/v1/slo/client-sample',
      broadcast: 'live/cam1',
      transport: 'hls',
      ingestTsMs: 1_000,
      renderTsMs: 1_120,
      fetchImpl: fetchImpl as unknown as typeof fetch,
    });
    expect(ok).toBe(false);
  });
});
