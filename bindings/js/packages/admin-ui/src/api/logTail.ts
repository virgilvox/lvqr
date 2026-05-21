import type { LogLine } from '@lvqr/core';

/** Log levels in descending severity, used for filter chips + ordering. */
export const LOG_LEVELS = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE'] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

/**
 * Parse one SSE `data` payload into a LogLine, or null when it is not a
 * well-formed log line (e.g. a keep-alive comment or malformed JSON). Pure so
 * the view's EventSource handler stays trivial and this stays unit-testable.
 */
export function parseLogLine(data: string): LogLine | null {
  try {
    const o = JSON.parse(data) as Partial<LogLine>;
    if (o && typeof o.message === 'string' && typeof o.level === 'string' && typeof o.ts_ms === 'number') {
      return {
        ts_ms: o.ts_ms,
        level: o.level,
        target: typeof o.target === 'string' ? o.target : '',
        message: o.message,
      };
    }
  } catch {
    // not JSON
  }
  return null;
}

/**
 * Append `line` to `buf`, returning a new array capped at `cap` entries
 * (oldest dropped). Returns a fresh array so Vue reactivity sees the change.
 */
export function appendCapped(buf: LogLine[], line: LogLine, cap: number): LogLine[] {
  const limit = Math.max(1, cap);
  const start = buf.length >= limit ? buf.length - limit + 1 : 0;
  const next = buf.slice(start);
  next.push(line);
  return next;
}
