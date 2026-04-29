import { describe, expect, it } from 'vitest';
import { Crypto as NodeCrypto } from '@peculiar/webcrypto';
import {
  bytesToBase64Url,
  signHmacSha256,
  signLiveUrl,
  signPlaybackUrl,
} from '../../src/composables/useHmacSign';

// jsdom's `crypto.subtle` is patchy across versions; force a real Web
// Crypto polyfill so HMAC-SHA256 actually runs in the test environment.
if (!(globalThis as unknown as { crypto?: { subtle?: object } }).crypto?.subtle) {
  (globalThis as unknown as { crypto: Crypto }).crypto = new NodeCrypto() as unknown as Crypto;
}

describe('bytesToBase64Url', () => {
  it('produces URL-safe base64 with no padding', () => {
    expect(bytesToBase64Url(new Uint8Array([0xff, 0xfe, 0xfd]))).toBe('__79');
    expect(bytesToBase64Url(new Uint8Array([0]))).toBe('AA');
    expect(bytesToBase64Url(new Uint8Array(0))).toBe('');
  });
});

describe('signHmacSha256', () => {
  it('is deterministic for a given (secret, input) pair', async () => {
    const a = await signHmacSha256('s3cret', 'hello');
    const b = await signHmacSha256('s3cret', 'hello');
    expect(a).toBe(b);
  });

  it('changes when the input changes', async () => {
    const a = await signHmacSha256('s3cret', 'hello');
    const b = await signHmacSha256('s3cret', 'world');
    expect(a).not.toBe(b);
  });

  it('changes when the secret changes', async () => {
    const a = await signHmacSha256('s3cret', 'hello');
    const b = await signHmacSha256('OTHER', 'hello');
    expect(a).not.toBe(b);
  });
});

describe('signPlaybackUrl', () => {
  it('appends both exp + sig query parameters', async () => {
    const url = await signPlaybackUrl('http://relay:8080', '/playback/live/demo', 1700000000, 's3cret');
    expect(url).toMatch(/^http:\/\/relay:8080\/playback\/live\/demo\?exp=1700000000&sig=[A-Za-z0-9_-]+$/);
  });

  it('normalises a path missing the leading slash', async () => {
    const a = await signPlaybackUrl('http://relay', 'playback/x', 1, 'k');
    const b = await signPlaybackUrl('http://relay', '/playback/x', 1, 'k');
    expect(a).toBe(b);
  });

  it('strips a trailing slash on the base URL', async () => {
    const a = await signPlaybackUrl('http://relay/', '/playback/x', 1, 'k');
    expect(a.startsWith('http://relay/playback/x?')).toBe(true);
  });
});

describe('signLiveUrl', () => {
  it('produces an HLS master playlist URL with exp + sig', async () => {
    const url = await signLiveUrl('http://relay:8888', 'hls', 'live/demo', 1700000000, 'k');
    expect(url).toMatch(/^http:\/\/relay:8888\/hls\/live%2Fdemo\/master\.m3u8\?exp=1700000000&sig=[A-Za-z0-9_-]+$/);
  });

  it('produces a DASH manifest URL with exp + sig', async () => {
    const url = await signLiveUrl('http://relay:8889', 'dash', 'live/demo', 1700000000, 'k');
    expect(url).toMatch(/^http:\/\/relay:8889\/dash\/live%2Fdemo\/manifest\.mpd\?exp=1700000000&sig=[A-Za-z0-9_-]+$/);
  });

  it('rejects cross-scheme replay (HLS sig != DASH sig under the same secret + broadcast + exp)', async () => {
    const hls = await signLiveUrl('http://r', 'hls', 'live/x', 1, 'k');
    const dash = await signLiveUrl('http://r', 'dash', 'live/x', 1, 'k');
    const hlsSig = new URL(hls).searchParams.get('sig');
    const dashSig = new URL(dash).searchParams.get('sig');
    expect(hlsSig).not.toBe(dashSig);
  });
});
