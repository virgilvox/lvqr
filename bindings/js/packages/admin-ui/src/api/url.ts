/**
 * URL helpers shared across the admin UI. Pure functions; unit-tested.
 */

/**
 * Join a base URL with a path, collapsing any duplicate slashes between them.
 * Handles trailing slashes on the base + leading slashes on the path
 * symmetrically so `joinUrl("http://x/", "/api")` and `joinUrl("http://x", "api")`
 * both produce `"http://x/api"`.
 */
export function joinUrl(base: string, path: string): string {
  const b = base.replace(/\/+$/, '');
  const p = path.replace(/^\/+/, '');
  return `${b}/${p}`;
}

/** Normalize a relay base URL: strips trailing slash, validates the protocol. */
export function normalizeRelayUrl(url: string): string {
  const trimmed = url.trim().replace(/\/+$/, '');
  if (!/^https?:\/\//i.test(trimmed)) {
    throw new Error(`relay URL must start with http:// or https://, got: ${url}`);
  }
  return trimmed;
}

/**
 * Pretty-format a UNIX-ms timestamp for the status bar / dashboards. Returns
 * `"never"` when the input is `null`, `undefined`, or `0`.
 */
export function formatRelativeTime(ms: number | null | undefined, now: number = Date.now()): string {
  if (!ms) return 'never';
  const delta = Math.max(0, now - ms);
  if (delta < 1_000) return 'just now';
  if (delta < 60_000) return `${Math.floor(delta / 1_000)}s ago`;
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;
  return `${Math.floor(delta / 86_400_000)}d ago`;
}

/** Format a byte count as a compact string (e.g. `"4.3 MB"`, `"812 kB"`). */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '-';
  const units = ['B', 'kB', 'MB', 'GB', 'TB'];
  let n = bytes;
  let i = 0;
  while (n >= 1024 && i < units.length - 1) {
    n /= 1024;
    i++;
  }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

/** Format a duration in seconds as `HH:MM:SS` or `MM:SS` (sub-hour). */
export function formatDuration(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return '-';
  const s = Math.floor(secs % 60);
  const m = Math.floor((secs / 60) % 60);
  const h = Math.floor(secs / 3600);
  const pad = (n: number) => n.toString().padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}
