// Unit tests for @lvqr/dvr-player SCTE-35 marker arithmetic.
//
// Pure-function tests; no DOM, no relay, no hls.js import. The
// marker pipeline accepts an `HlsDateRangeLike` structural type
// so tests construct stub daterange records directly.

import { describe, expect, it } from 'vitest';
import {
  classifyMarker,
  dvrMarkersFromHlsDateRanges,
  formatDuration,
  groupOutInPairs,
  markerToFraction,
  type DvrMarker,
  type HlsDateRangeLike,
} from '../../packages/dvr-player/src/markers';

function stub(
  id: string,
  attr: Record<string, string | undefined>,
  startTime: number,
  duration: number | null = null,
  klass?: string,
): HlsDateRangeLike {
  return {
    id,
    class: klass,
    startTime,
    startDate: new Date('2026-04-25T00:00:00Z'),
    duration,
    attr,
  };
}

function asMarker(m: Partial<DvrMarker>): DvrMarker {
  return {
    id: 'x',
    kind: 'cmd',
    startTime: 0,
    startDate: new Date(0),
    durationSecs: null,
    class: null,
    scte35Hex: null,
    ...m,
  };
}

describe('classifyMarker', () => {
  it('returns "out" when SCTE35-OUT is present', () => {
    expect(classifyMarker(stub('a', { 'SCTE35-OUT': '0xFC' }, 1))).toBe('out');
  });
  it('returns "in" when SCTE35-IN is present', () => {
    expect(classifyMarker(stub('a', { 'SCTE35-IN': '0xFC' }, 1))).toBe('in');
  });
  it('returns "cmd" when SCTE35-CMD is present', () => {
    expect(classifyMarker(stub('a', { 'SCTE35-CMD': '0xFC' }, 1))).toBe('cmd');
  });
  it('returns "unknown" when no SCTE35 attribute is present', () => {
    expect(classifyMarker(stub('a', { CLASS: 'com.apple.hls.interstitial' }, 1))).toBe('unknown');
  });
  it('treats an empty-string SCTE35-* attribute as absent', () => {
    expect(classifyMarker(stub('a', { 'SCTE35-OUT': '' }, 1))).toBe('unknown');
  });
});

describe('markerToFraction', () => {
  const range = { start: 100, end: 200 };

  it('maps an in-range marker to a [0, 1] fraction', () => {
    const m = asMarker({ startTime: 150 });
    expect(markerToFraction(m, range)).toBeCloseTo(0.5);
  });
  it('returns null for a NaN startTime', () => {
    const m = asMarker({ startTime: NaN });
    expect(markerToFraction(m, range)).toBeNull();
  });
  it('returns null for a startTime below the range', () => {
    const m = asMarker({ startTime: 50 });
    expect(markerToFraction(m, range)).toBeNull();
  });
  it('returns null for a startTime above the range', () => {
    const m = asMarker({ startTime: 250 });
    expect(markerToFraction(m, range)).toBeNull();
  });
  it('clamps the live edge to fraction 1', () => {
    const m = asMarker({ startTime: 200 });
    expect(markerToFraction(m, range)).toBe(1);
  });
});

describe('dvrMarkersFromHlsDateRanges', () => {
  it('drops entries with non-finite startTime', () => {
    const ranges = {
      a: stub('a', { 'SCTE35-OUT': '0xFC' }, NaN),
      b: stub('b', { 'SCTE35-IN': '0xFD' }, 5),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    expect(out.map((m) => m.id)).toEqual(['b']);
  });

  it('synthesizes a derived IN marker from an OUT carrying DURATION', () => {
    const ranges = {
      ad: stub('ad', { 'SCTE35-OUT': '0xAA' }, 30, 15),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    expect(out).toHaveLength(2);
    const o = out.find((m) => m.kind === 'out');
    const i = out.find((m) => m.kind === 'in');
    expect(o?.startTime).toBe(30);
    expect(o?.scte35Hex).toBe('0xAA');
    expect(o?.durationSecs).toBe(15);
    expect(i?.startTime).toBe(45);
    expect(i?.durationSecs).toBeNull();
    // No real SCTE35-IN on the merged DateRange (hls.js dropped
    // it on START-DATE conflict); the synthesized IN surfaces
    // the OUT's hex as a fallback reference.
    expect(i?.scte35Hex).toBe('0xAA');
  });

  it('does not synthesize an IN when the OUT has no DURATION', () => {
    const ranges = {
      ad: stub('ad', { 'SCTE35-OUT': '0xAA' }, 30),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    expect(out).toHaveLength(1);
    expect(out[0]?.kind).toBe('out');
  });

  it('uses the merged SCTE35-IN hex when hls.js managed to merge it', () => {
    const ranges = {
      ad: stub('ad', { 'SCTE35-OUT': '0xAA', 'SCTE35-IN': '0xBB' }, 30, 15),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    const i = out.find((m) => m.kind === 'in');
    expect(i?.scte35Hex).toBe('0xBB');
  });

  it('sorts by ascending startTime then ascending id', () => {
    const ranges = {
      z: stub('z', { 'SCTE35-CMD': '0xA' }, 30),
      m: stub('m', { 'SCTE35-CMD': '0xB' }, 10),
      a: stub('a', { 'SCTE35-CMD': '0xC' }, 30),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    expect(out.map((m) => m.id)).toEqual(['m', 'a', 'z']);
  });

  it('preserves the SCTE35-* hex on the kind-specific field', () => {
    const ranges = {
      o: stub('o', { 'SCTE35-OUT': '0xAA' }, 1),
      i: stub('i', { 'SCTE35-IN': '0xBB' }, 2),
      c: stub('c', { 'SCTE35-CMD': '0xCC' }, 3),
      u: stub('u', { CLASS: 'other' }, 4),
    };
    const out = dvrMarkersFromHlsDateRanges(ranges);
    const byId = Object.fromEntries(out.map((m) => [m.id, m]));
    expect(byId.o.kind).toBe('out');
    expect(byId.o.scte35Hex).toBe('0xAA');
    expect(byId.i.kind).toBe('in');
    expect(byId.i.scte35Hex).toBe('0xBB');
    expect(byId.c.kind).toBe('cmd');
    expect(byId.c.scte35Hex).toBe('0xCC');
    expect(byId.u.kind).toBe('unknown');
    expect(byId.u.scte35Hex).toBeNull();
  });

  it('preserves duration and class when present', () => {
    const ranges = {
      d: stub('d', { 'SCTE35-OUT': '0xAA' }, 5, 30, 'urn:scte:scte35:2014:bin'),
    };
    const [marker] = dvrMarkersFromHlsDateRanges(ranges);
    expect(marker?.durationSecs).toBe(30);
    expect(marker?.class).toBe('urn:scte:scte35:2014:bin');
  });
});

describe('groupOutInPairs', () => {
  it('pairs OUT and IN that share an ID', () => {
    const out = asMarker({ id: '1', kind: 'out', startTime: 10 });
    const in_ = asMarker({ id: '1', kind: 'in', startTime: 25 });
    const groups = groupOutInPairs([out, in_]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.kind).toBe('pair');
    expect(groups[0]?.out?.startTime).toBe(10);
    expect(groups[0]?.in?.startTime).toBe(25);
  });

  it('emits an "open" group for an OUT without a matching IN', () => {
    const out = asMarker({ id: '1', kind: 'out', startTime: 10 });
    const groups = groupOutInPairs([out]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.kind).toBe('open');
    expect(groups[0]?.out?.id).toBe('1');
    expect(groups[0]?.in).toBeNull();
  });

  it('emits an "in-only" group for an IN without a matching OUT', () => {
    const in_ = asMarker({ id: '1', kind: 'in', startTime: 25 });
    const groups = groupOutInPairs([in_]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.kind).toBe('in-only');
    expect(groups[0]?.in?.id).toBe('1');
    expect(groups[0]?.out).toBeNull();
  });

  it('emits a "singleton" group for CMD entries', () => {
    const cmd = asMarker({ id: 'c', kind: 'cmd', startTime: 5 });
    const groups = groupOutInPairs([cmd]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.kind).toBe('singleton');
    expect(groups[0]?.out?.kind).toBe('cmd');
  });

  it('emits a "singleton" group for unknown entries with the marker on out', () => {
    const u = asMarker({ id: 'u', kind: 'unknown', startTime: 5 });
    const groups = groupOutInPairs([u]);
    expect(groups[0]?.kind).toBe('singleton');
    expect(groups[0]?.out?.kind).toBe('unknown');
  });

  it('swaps reversed pair times so out.startTime <= in.startTime', () => {
    const out = asMarker({ id: '1', kind: 'out', startTime: 30 });
    const in_ = asMarker({ id: '1', kind: 'in', startTime: 10 });
    const groups = groupOutInPairs([out, in_]);
    expect(groups[0]?.kind).toBe('pair');
    expect(groups[0]?.out?.startTime).toBe(10);
    expect(groups[0]?.in?.startTime).toBe(30);
  });

  it('orders groups by ascending earliest startTime', () => {
    const m = [
      asMarker({ id: 'late', kind: 'out', startTime: 100 }),
      asMarker({ id: 'late', kind: 'in', startTime: 110 }),
      asMarker({ id: 'early', kind: 'cmd', startTime: 5 }),
    ];
    const groups = groupOutInPairs(m);
    expect(groups.map((g) => g.id)).toEqual(['early', 'late']);
  });
});

describe('formatDuration', () => {
  it('formats sub-minute values with three decimal seconds', () => {
    expect(formatDuration(0)).toBe('0.000s');
    expect(formatDuration(12)).toBe('12.000s');
    expect(formatDuration(59.9)).toBe('59.900s');
  });
  it('formats minute-scale values as M:SS', () => {
    expect(formatDuration(60)).toBe('1:00');
    expect(formatDuration(90)).toBe('1:30');
    expect(formatDuration(599)).toBe('9:59');
  });
  it('formats hour-scale values as H:MM:SS', () => {
    expect(formatDuration(3600)).toBe('1:00:00');
    expect(formatDuration(5400)).toBe('1:30:00');
    expect(formatDuration(7322)).toBe('2:02:02');
  });
  it('clamps negative values to zero', () => {
    expect(formatDuration(-1)).toBe('0.000s');
  });
});
