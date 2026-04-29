import { describe, expect, it } from 'vitest';
import { formatBytes, formatDuration, formatRelativeTime, joinUrl, normalizeRelayUrl } from '../../src/api/url';

describe('joinUrl', () => {
  it('joins with neither having a slash', () => {
    expect(joinUrl('http://x', 'api')).toBe('http://x/api');
  });
  it('collapses a trailing base slash', () => {
    expect(joinUrl('http://x/', 'api')).toBe('http://x/api');
  });
  it('collapses a leading path slash', () => {
    expect(joinUrl('http://x', '/api')).toBe('http://x/api');
  });
  it('collapses both', () => {
    expect(joinUrl('http://x/', '/api')).toBe('http://x/api');
  });
  it('collapses multiple slashes', () => {
    expect(joinUrl('http://x///', '///api')).toBe('http://x/api');
  });
});

describe('normalizeRelayUrl', () => {
  it('strips a trailing slash', () => {
    expect(normalizeRelayUrl('http://localhost:8080/')).toBe('http://localhost:8080');
  });
  it('preserves the path', () => {
    expect(normalizeRelayUrl('https://relay.example.com/v1/')).toBe('https://relay.example.com/v1');
  });
  it('rejects URLs without a protocol', () => {
    expect(() => normalizeRelayUrl('localhost:8080')).toThrow(/http/);
  });
});

describe('formatRelativeTime', () => {
  const now = 1_700_000_000_000;
  it('returns "never" for null / 0', () => {
    expect(formatRelativeTime(null, now)).toBe('never');
    expect(formatRelativeTime(0, now)).toBe('never');
  });
  it('returns "just now" inside the first second', () => {
    expect(formatRelativeTime(now - 500, now)).toBe('just now');
  });
  it('uses seconds, minutes, hours, days', () => {
    expect(formatRelativeTime(now - 5_000, now)).toBe('5s ago');
    expect(formatRelativeTime(now - 5 * 60_000, now)).toBe('5m ago');
    expect(formatRelativeTime(now - 5 * 3_600_000, now)).toBe('5h ago');
    expect(formatRelativeTime(now - 5 * 86_400_000, now)).toBe('5d ago');
  });
});

describe('formatBytes', () => {
  it('handles 0', () => {
    expect(formatBytes(0)).toBe('0 B');
  });
  it('handles plain bytes', () => {
    expect(formatBytes(512)).toBe('512 B');
  });
  it('uses kB for < 1 MB', () => {
    expect(formatBytes(2048)).toBe('2.0 kB');
  });
  it('uses MB for medium values', () => {
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
  });
  it('returns "-" for negative or NaN', () => {
    expect(formatBytes(-1)).toBe('-');
    expect(formatBytes(Number.NaN)).toBe('-');
  });
});

describe('formatDuration', () => {
  it('handles sub-minute', () => {
    expect(formatDuration(45)).toBe('00:45');
  });
  it('handles MM:SS', () => {
    expect(formatDuration(125)).toBe('02:05');
  });
  it('uses HH:MM:SS for >= 1h', () => {
    expect(formatDuration(3725)).toBe('1:02:05');
  });
  it('returns "-" for negative or NaN', () => {
    expect(formatDuration(-1)).toBe('-');
    expect(formatDuration(Number.NaN)).toBe('-');
  });
});
