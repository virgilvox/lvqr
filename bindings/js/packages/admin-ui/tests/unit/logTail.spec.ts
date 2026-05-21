import { describe, expect, it } from 'vitest';
import { LvqrAdminClient } from '@lvqr/core';
import { appendCapped, parseLogLine, LOG_LEVELS } from '../../src/api/logTail';

describe('logTail helpers', () => {
  it('parseLogLine accepts a well-formed line', () => {
    const line = parseLogLine('{"ts_ms":1,"level":"INFO","target":"lvqr","message":"hi"}');
    expect(line).not.toBeNull();
    expect(line?.level).toBe('INFO');
    expect(line?.message).toBe('hi');
    expect(line?.target).toBe('lvqr');
  });

  it('parseLogLine defaults a missing target to empty string', () => {
    const line = parseLogLine('{"ts_ms":1,"level":"WARN","message":"m"}');
    expect(line?.target).toBe('');
  });

  it('parseLogLine rejects malformed / non-log payloads', () => {
    expect(parseLogLine('not json')).toBeNull();
    expect(parseLogLine('{"level":"INFO"}')).toBeNull(); // no message/ts_ms
    expect(parseLogLine('42')).toBeNull();
  });

  it('appendCapped keeps only the most recent cap entries', () => {
    let buf: ReturnType<typeof parseLogLine>[] = [] as never;
    let lines: NonNullable<ReturnType<typeof parseLogLine>>[] = [];
    for (let i = 0; i < 5; i += 1) {
      lines = appendCapped(lines, { ts_ms: i, level: 'INFO', target: 't', message: `m${i}` }, 3);
    }
    expect(lines).toHaveLength(3);
    expect(lines[0].message).toBe('m2');
    expect(lines[2].message).toBe('m4');
    void buf;
  });

  it('appendCapped returns a new array (reactivity-safe)', () => {
    const a = [{ ts_ms: 0, level: 'INFO', target: 't', message: 'a' }];
    const b = appendCapped(a, { ts_ms: 1, level: 'INFO', target: 't', message: 'b' }, 10);
    expect(b).not.toBe(a);
    expect(b).toHaveLength(2);
  });

  it('exposes the five tracing levels', () => {
    expect([...LOG_LEVELS]).toEqual(['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE']);
  });
});

describe('LvqrAdminClient.logsStreamUrl', () => {
  it('appends the bearer token as a query param', () => {
    const c = new LvqrAdminClient('http://relay:8080/', { bearerToken: 'sec ret' });
    expect(c.logsStreamUrl()).toBe('http://relay:8080/api/v1/logs?token=sec%20ret');
  });

  it('omits the token when none is configured', () => {
    const c = new LvqrAdminClient('http://relay:8080');
    expect(c.logsStreamUrl()).toBe('http://relay:8080/api/v1/logs');
  });
});
