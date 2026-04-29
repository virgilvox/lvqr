import { describe, expect, it } from 'vitest';
import {
  buildConstraints,
  resolveSessionUrl,
  whipPostInit,
  type PublishOptions,
} from '../../src/composables/useWhipPublisher';

function opts(over: Partial<PublishOptions> = {}): PublishOptions {
  return {
    whipUrl: 'http://localhost:18443/whip/live/demo',
    video: true,
    audio: true,
    source: 'camera',
    ...over,
  };
}

describe('whipPostInit', () => {
  it('always sets application/sdp content type', () => {
    const init = whipPostInit('v=0\r\n');
    const headers = init.headers as Record<string, string>;
    expect(headers['Content-Type']).toBe('application/sdp');
    expect(init.method).toBe('POST');
    expect(init.body).toBe('v=0\r\n');
  });

  it('attaches a bearer header when a token is provided', () => {
    const init = whipPostInit('v=0', 'tok-x');
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBe('Bearer tok-x');
  });

  it('omits the bearer header when the token is whitespace only', () => {
    const init = whipPostInit('v=0', '   ');
    const headers = init.headers as Record<string, string>;
    expect(headers.Authorization).toBeUndefined();
  });
});

describe('resolveSessionUrl', () => {
  it('resolves an absolute Location verbatim', () => {
    const u = resolveSessionUrl('http://x/whip/live/demo', 'http://x/whip/live/demo/abc-123');
    expect(u).toBe('http://x/whip/live/demo/abc-123');
  });

  it('resolves a relative Location against the POST URL', () => {
    const u = resolveSessionUrl('http://x:18443/whip/live/demo', '/whip/live/demo/abc-123');
    expect(u).toBe('http://x:18443/whip/live/demo/abc-123');
  });

  it('returns null on missing Location', () => {
    expect(resolveSessionUrl('http://x/whip/live/demo', null)).toBeNull();
  });

  it('does not throw on weird Location values', () => {
    // URL with a base is permissive; whatever it returns must be a string
    // or null, never an exception. This locks the no-throw contract.
    expect(() => resolveSessionUrl('http://x/whip/live/demo', '://broken')).not.toThrow();
    expect(() => resolveSessionUrl('http://x/whip/live/demo', '')).not.toThrow();
  });
});

describe('buildConstraints', () => {
  it('defaults to video + audio without resolution constraints', () => {
    const c = buildConstraints(opts());
    expect(c.audio).toBe(true);
    expect(typeof c.video).toBe('object');
  });

  it('applies width / height / frameRate as ideal constraints when set', () => {
    const c = buildConstraints(opts({ width: 1280, height: 720, frameRate: 30 }));
    const v = c.video as MediaTrackConstraints;
    expect((v.width as { ideal: number }).ideal).toBe(1280);
    expect((v.height as { ideal: number }).ideal).toBe(720);
    expect((v.frameRate as { ideal: number }).ideal).toBe(30);
  });

  it('disables video / audio when toggled off', () => {
    expect(buildConstraints(opts({ video: false })).video).toBe(false);
    expect(buildConstraints(opts({ audio: false })).audio).toBe(false);
  });
});
