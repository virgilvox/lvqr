// Unit tests for @lvqr/dvr-player attribute helpers.
//
// Pure-function tests over a synthesized HTMLElement-shaped object;
// no DOM, no jsdom. The helpers do not depend on any HTMLElement
// behavior beyond hasAttribute/getAttribute/setAttribute/
// removeAttribute, so a tiny in-memory shim is sufficient.

import { describe, expect, it } from 'vitest';
import {
  getBooleanAttr,
  setBooleanAttr,
  getStringAttr,
  getNumericAttr,
} from '../../packages/dvr-player/src/internals/attrs';

function makeEl(): HTMLElement {
  const attrs = new Map<string, string>();
  const el = {
    hasAttribute: (n: string) => attrs.has(n),
    getAttribute: (n: string) => attrs.get(n) ?? null,
    setAttribute: (n: string, v: string) => {
      attrs.set(n, v);
    },
    removeAttribute: (n: string) => {
      attrs.delete(n);
    },
  } as unknown as HTMLElement;
  return el;
}

describe('getBooleanAttr', () => {
  it('returns true when the attribute is present (any value)', () => {
    const el = makeEl();
    el.setAttribute('autoplay', '');
    expect(getBooleanAttr(el, 'autoplay')).toBe(true);

    el.setAttribute('muted', 'true');
    expect(getBooleanAttr(el, 'muted')).toBe(true);

    el.setAttribute('paused', 'false');
    expect(getBooleanAttr(el, 'paused')).toBe(true);
  });

  it('returns false when the attribute is absent', () => {
    const el = makeEl();
    expect(getBooleanAttr(el, 'autoplay')).toBe(false);
  });
});

describe('setBooleanAttr', () => {
  it('adds the attribute (value "") when value is true and absent', () => {
    const el = makeEl();
    setBooleanAttr(el, 'muted', true);
    expect(el.hasAttribute('muted')).toBe(true);
    expect(el.getAttribute('muted')).toBe('');
  });

  it('is idempotent when value is true and the attribute is already present', () => {
    const el = makeEl();
    el.setAttribute('muted', 'preserved');
    setBooleanAttr(el, 'muted', true);
    expect(el.getAttribute('muted')).toBe('preserved');
  });

  it('removes the attribute when value is false', () => {
    const el = makeEl();
    el.setAttribute('muted', '');
    setBooleanAttr(el, 'muted', false);
    expect(el.hasAttribute('muted')).toBe(false);
  });

  it('is a no-op when value is false and the attribute is already absent', () => {
    const el = makeEl();
    setBooleanAttr(el, 'autoplay', false);
    expect(el.hasAttribute('autoplay')).toBe(false);
  });
});

describe('getStringAttr', () => {
  it('returns the attribute value when present', () => {
    const el = makeEl();
    el.setAttribute('src', 'https://relay/hls/x/master.m3u8');
    expect(getStringAttr(el, 'src')).toBe('https://relay/hls/x/master.m3u8');
  });

  it('returns the documented fallback when absent', () => {
    const el = makeEl();
    expect(getStringAttr(el, 'controls', 'custom')).toBe('custom');
    expect(getStringAttr(el, 'controls')).toBe('');
  });

  it('returns "" (not the fallback) when the attribute value is empty', () => {
    const el = makeEl();
    el.setAttribute('controls', '');
    expect(getStringAttr(el, 'controls', 'custom')).toBe('');
  });
});

describe('getNumericAttr', () => {
  it('returns the parsed number when the attribute is a valid numeric string', () => {
    const el = makeEl();
    el.setAttribute('threshold', '6');
    expect(getNumericAttr(el, 'threshold', 999)).toBe(6);

    el.setAttribute('rate', '1.25');
    expect(getNumericAttr(el, 'rate', 999)).toBe(1.25);

    el.setAttribute('neg', '-3');
    expect(getNumericAttr(el, 'neg', 999)).toBe(-3);
  });

  it('returns the fallback when the attribute is absent', () => {
    const el = makeEl();
    expect(getNumericAttr(el, 'threshold', 6)).toBe(6);
  });

  it('returns the fallback when the attribute value is empty', () => {
    const el = makeEl();
    el.setAttribute('threshold', '');
    expect(getNumericAttr(el, 'threshold', 6)).toBe(6);
  });

  it('returns the fallback when the attribute value is not finite', () => {
    const el = makeEl();
    el.setAttribute('threshold', 'not-a-number');
    expect(getNumericAttr(el, 'threshold', 6)).toBe(6);

    el.setAttribute('threshold', 'NaN');
    expect(getNumericAttr(el, 'threshold', 6)).toBe(6);

    el.setAttribute('threshold', 'Infinity');
    expect(getNumericAttr(el, 'threshold', 6)).toBe(6);
  });

  it('treats "0" as a valid value, not the fallback', () => {
    const el = makeEl();
    el.setAttribute('threshold', '0');
    expect(getNumericAttr(el, 'threshold', 999)).toBe(0);
  });
});
