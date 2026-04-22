/**
 * LVQR Admin API client.
 * Works in both browser and Node.js environments.
 */

export interface RelayStats {
  publishers: number;
  subscribers: number;
  tracks: number;
  bytes_received: number;
  bytes_sent: number;
  uptime_secs: number;
}

export interface StreamInfo {
  name: string;
  subscribers: number;
}

export interface LvqrAdminClientOptions {
  /**
   * Per-request deadline in milliseconds. Applied to every admin
   * HTTP call so a misbehaving server that accepts the TCP
   * connection but never responds does not hang the Promise
   * forever. Defaults to 10_000 (10 s). Set to 0 to disable (not
   * recommended for production).
   */
  fetchTimeoutMs?: number;
}

const DEFAULT_FETCH_TIMEOUT_MS = 10_000;

/**
 * Client for the LVQR admin HTTP API.
 *
 * @example
 * ```ts
 * const admin = new LvqrAdminClient('http://localhost:8080');
 * const streams = await admin.listStreams();
 * const stats = await admin.stats();
 * ```
 */
export class LvqrAdminClient {
  private baseUrl: string;
  private options: LvqrAdminClientOptions;

  constructor(baseUrl: string, options: LvqrAdminClientOptions = {}) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.options = options;
  }

  /** Check if the relay is healthy. */
  async healthz(): Promise<boolean> {
    try {
      const resp = await this.fetchWithTimeout(`${this.baseUrl}/healthz`);
      return resp.ok;
    } catch {
      return false;
    }
  }

  /** Get relay statistics. */
  async stats(): Promise<RelayStats> {
    const resp = await this.fetchWithTimeout(`${this.baseUrl}/api/v1/stats`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  }

  /** List active streams. */
  async listStreams(): Promise<StreamInfo[]> {
    const resp = await this.fetchWithTimeout(`${this.baseUrl}/api/v1/streams`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  }

  /**
   * `fetch` wrapper that enforces the configured fetch timeout via
   * an AbortController. A timeout cancels the in-flight request and
   * rejects with a descriptive AbortError so callers can distinguish
   * timeout from network failure via `e.name === 'AbortError'`.
   */
  private async fetchWithTimeout(url: string, init: RequestInit = {}): Promise<Response> {
    const timeoutMs = this.options.fetchTimeoutMs ?? DEFAULT_FETCH_TIMEOUT_MS;
    if (timeoutMs <= 0) {
      return fetch(url, init);
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      return await fetch(url, { ...init, signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
  }
}
