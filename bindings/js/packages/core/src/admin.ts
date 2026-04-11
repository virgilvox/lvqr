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

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
  }

  /** Check if the relay is healthy. */
  async healthz(): Promise<boolean> {
    try {
      const resp = await fetch(`${this.baseUrl}/healthz`);
      return resp.ok;
    } catch {
      return false;
    }
  }

  /** Get relay statistics. */
  async stats(): Promise<RelayStats> {
    const resp = await fetch(`${this.baseUrl}/api/v1/stats`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  }

  /** List active streams. */
  async listStreams(): Promise<StreamInfo[]> {
    const resp = await fetch(`${this.baseUrl}/api/v1/streams`);
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    return resp.json();
  }
}
