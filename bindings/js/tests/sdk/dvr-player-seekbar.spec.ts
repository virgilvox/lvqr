// Unit tests for @lvqr/dvr-player seek-bar arithmetic.
//
// Pure-function tests; no DOM, no relay. Imported directly from
// the package's source so the build does not need to run before
// the tests do (Vitest handles `.ts` natively).

import { describe, expect, it } from 'vitest';
import {
  fractionToTime,
  timeToFraction,
  formatTime,
  generatePercentileLabels,
  isAtLiveEdge,
} from '../../packages/dvr-player/src/seekbar';

describe('fractionToTime / timeToFraction', () => {
  it('round-trips at the endpoints', () => {
    const range = { start: 100, end: 460 };
    expect(fractionToTime(0, range)).toBe(100);
    expect(fractionToTime(1, range)).toBe(460);
    expect(timeToFraction(100, range)).toBe(0);
    expect(timeToFraction(460, range)).toBe(1);
  });

  it('clamps fraction inputs outside [0, 1]', () => {
    const range = { start: 0, end: 100 };
    expect(fractionToTime(-0.5, range)).toBe(0);
    expect(fractionToTime(1.5, range)).toBe(100);
  });

  it('clamps time inputs outside the range', () => {
    const range = { start: 50, end: 150 };
    expect(timeToFraction(0, range)).toBe(0);
    expect(timeToFraction(200, range)).toBe(1);
  });

  it('returns zero for a degenerate (empty) range', () => {
    const range = { start: 42, end: 42 };
    expect(timeToFraction(42, range)).toBe(0);
  });
});

describe('formatTime', () => {
  it('uses MM:SS when the span is under one hour', () => {
    expect(formatTime(0, 30)).toBe('00:00');
    expect(formatTime(7, 30)).toBe('00:07');
    expect(formatTime(75, 300)).toBe('01:15');
  });

  it('uses HH:MM:SS when the span is at least one hour', () => {
    expect(formatTime(0, 3600)).toBe('00:00:00');
    expect(formatTime(75, 3600)).toBe('00:01:15');
    expect(formatTime(5025, 7200)).toBe('01:23:45');
  });

  it('clamps negative seconds to zero', () => {
    expect(formatTime(-1, 30)).toBe('00:00');
    expect(formatTime(-1, 3600)).toBe('00:00:00');
  });
});

describe('generatePercentileLabels', () => {
  it('generates five labels by default at 0/25/50/75/100% of the range', () => {
    const labels = generatePercentileLabels({ start: 0, end: 60 });
    expect(labels.map((l) => l.fraction)).toEqual([0, 0.25, 0.5, 0.75, 1]);
    expect(labels.map((l) => l.time)).toEqual([0, 15, 30, 45, 60]);
    expect(labels.map((l) => l.text)).toEqual(['00:00', '00:15', '00:30', '00:45', '01:00']);
  });

  it('switches to HH:MM:SS for ranges of at least one hour', () => {
    const labels = generatePercentileLabels({ start: 0, end: 3600 });
    expect(labels[0]!.text).toBe('00:00:00');
    expect(labels[2]!.text).toBe('00:30:00');
    expect(labels[4]!.text).toBe('01:00:00');
  });

  it('reports labels relative to range.start, not absolute time', () => {
    const labels = generatePercentileLabels({ start: 1000, end: 1060 });
    expect(labels[0]!.time).toBe(1000);
    expect(labels[0]!.text).toBe('00:00');
    expect(labels[4]!.time).toBe(1060);
    expect(labels[4]!.text).toBe('01:00');
  });

  it('honours a custom percentile list', () => {
    const labels = generatePercentileLabels({ start: 0, end: 100 }, [0, 0.5, 1]);
    expect(labels.length).toBe(3);
    expect(labels.map((l) => l.fraction)).toEqual([0, 0.5, 1]);
  });
});

describe('isAtLiveEdge', () => {
  it('is true when delta is strictly under the threshold', () => {
    expect(isAtLiveEdge(2, 6)).toBe(true);
    expect(isAtLiveEdge(0, 6)).toBe(true);
  });

  it('is false at and above the threshold', () => {
    expect(isAtLiveEdge(6, 6)).toBe(false);
    expect(isAtLiveEdge(30, 6)).toBe(false);
  });

  it('handles negative delta as live-edge (currentTime ahead of seekable.end)', () => {
    expect(isAtLiveEdge(-0.5, 6)).toBe(true);
  });
});
