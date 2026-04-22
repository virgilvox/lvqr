/**
 * LVQR streaming client.
 *
 * Connects to an LVQR relay via WebTransport (preferred) or WebSocket (fallback)
 * and subscribes to live video/audio tracks using the MoQ-Lite protocol.
 */

import { detectTransport, type TransportType } from './transport';
import { MoqSubscriber } from './moq';

export interface LvqrClientOptions {
  /** Force a specific transport. Default: auto-detect. */
  transport?: TransportType;
  /** SHA-256 fingerprint for self-signed certs (development). */
  fingerprint?: string;
  /** Optional viewer token for relays that require authentication. */
  token?: string;
  /**
   * Per-connect attempt deadline in milliseconds. Applied to
   * WebTransport `ready` and the WebSocket `open` handshake so a
   * server that accepts the TCP connection but never completes the
   * upgrade (common on misconfigured reverse proxies) does not hang
   * the Promise forever. Defaults to 10_000 (10 s). Set to 0 to
   * disable (not recommended for production).
   */
  connectTimeoutMs?: number;
}

const DEFAULT_CONNECT_TIMEOUT_MS = 10_000;

export interface LvqrEvents {
  /** Received an fMP4 frame (init segment or moof+mdat). */
  frame: (data: Uint8Array, track: string) => void;
  /** Connection established. */
  connected: () => void;
  /** Connection closed. */
  disconnected: (reason?: string) => void;
  /** Error occurred. */
  error: (error: Error) => void;
}

type EventName = keyof LvqrEvents;

/**
 * Client for connecting to an LVQR relay and subscribing to streams.
 *
 * @example
 * ```ts
 * const client = new LvqrClient('https://relay.example.com:4443');
 * await client.connect();
 * client.on('frame', (data, track) => {
 *   // data is fMP4: init segment (ftyp+moov) or media segment (moof+mdat)
 *   sourceBuffer.appendBuffer(data);
 * });
 * await client.subscribe('live/my-stream');
 * ```
 */
export class LvqrClient {
  private url: string;
  private options: LvqrClientOptions;
  private transport: WebTransport | WebSocket | null = null;
  private moqSubscriber: MoqSubscriber | null = null;
  private listeners: Map<string, Set<Function>> = new Map();
  private _connected = false;

  constructor(url: string, options: LvqrClientOptions = {}) {
    this.url = url;
    this.options = options;
  }

  /** Register an event listener. */
  on<E extends EventName>(event: E, callback: LvqrEvents[E]): this {
    if (!this.listeners.has(event)) {
      this.listeners.set(event, new Set());
    }
    this.listeners.get(event)!.add(callback);
    return this;
  }

  /** Remove an event listener. */
  off<E extends EventName>(event: E, callback: LvqrEvents[E]): this {
    this.listeners.get(event)?.delete(callback);
    return this;
  }

  private emit<E extends EventName>(event: E, ...args: Parameters<LvqrEvents[E]>): void {
    for (const cb of this.listeners.get(event) ?? []) {
      try {
        (cb as Function)(...args);
      } catch (e) {
        console.error(`LVQR event handler error (${event}):`, e);
      }
    }
  }

  /** Whether the client is currently connected. */
  get connected(): boolean {
    return this._connected;
  }

  /** Connect to the relay. */
  async connect(): Promise<void> {
    const transportType = this.options.transport ?? detectTransport();

    switch (transportType) {
      case 'webtransport':
        await this.connectWebTransport();
        break;
      case 'websocket':
        await this.connectWebSocket();
        break;
      default:
        throw new Error('No supported transport available');
    }

    this._connected = true;
    this.emit('connected');
  }

  /**
   * Subscribe to a broadcast's video and audio tracks.
   *
   * The broadcast path is e.g. "live/my-stream". This subscribes to both
   * the "0.mp4" (video) and "1.mp4" (audio) CMAF tracks. Frame data is
   * emitted via the 'frame' event as fMP4 segments (init + moof+mdat).
   */
  async subscribe(broadcast: string, tracks?: string[]): Promise<void> {
    if (!this._connected) {
      throw new Error('Not connected. Call connect() first.');
    }

    // Default: subscribe to video AND audio tracks. Pass an explicit list to
    // override (e.g. ['0.mp4'] for video only).
    const trackNames = tracks ?? ['0.mp4', '1.mp4'];

    if (this.moqSubscriber) {
      for (const trackName of trackNames) {
        await this.moqSubscriber.subscribe(broadcast, trackName, (data) => {
          this.emit('frame', data, trackName);
        });
      }
    } else if (this.transport instanceof WebSocket) {
      // WebSocket fallback: reconnect to the broadcast-specific WS endpoint.
      // The server's /ws/{broadcast} endpoint sends fMP4 frames as binary
      // messages prefixed with a 1-byte track ID (0=video, 1=audio).
      const ws = this.transport;
      ws.close(); // close the generic connection

      const wsBase = this.url.replace(/^https:/, 'wss:').replace(/^http:/, 'ws:');
      await this.connectWebSocketBroadcast(`${wsBase}/ws/${broadcast}`);
    }
  }

  /** Close the connection. */
  close(): void {
    if (this.moqSubscriber) {
      this.moqSubscriber.close();
      this.moqSubscriber = null;
    }
    if (this.transport instanceof WebTransport) {
      this.transport.close();
    } else if (this.transport instanceof WebSocket) {
      this.transport.close();
    }
    this.transport = null;
    this._connected = false;
    this.emit('disconnected');
  }

  private async connectWebTransport(): Promise<void> {
    const options: WebTransportOptions = {
      allowPooling: false,
      congestionControl: 'low-latency' as any,
    };

    if (this.options.fingerprint) {
      const hashBytes = hexToBytes(this.options.fingerprint);
      (options as any).serverCertificateHashes = [
        { algorithm: 'sha-256', value: hashBytes.buffer },
      ];
    }

    const wt = new WebTransport(this.url, options);
    wt.closed
      .then(() => {
        this._connected = false;
        this.emit('disconnected', 'transport closed');
      })
      .catch((e: Error) => {
        this._connected = false;
        this.emit('error', e);
        this.emit('disconnected', e.message);
      });

    await this.withConnectTimeout(wt.ready, 'WebTransport', () => {
      try {
        wt.close();
      } catch {
        // ignore: wt may already be failed
      }
    });
    this.transport = wt;
    this.moqSubscriber = new MoqSubscriber(wt);
  }

  /**
   * Race `promise` against this client's connect-timeout. On
   * timeout, invoke `onCancel` (e.g. to close an in-flight
   * WebSocket / WebTransport) and reject with a descriptive Error.
   * `timeoutMs === 0` disables the deadline.
   */
  private async withConnectTimeout<T>(
    promise: Promise<T>,
    label: string,
    onCancel: () => void,
  ): Promise<T> {
    const timeoutMs = this.options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
    if (timeoutMs <= 0) {
      return promise;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        try {
          onCancel();
        } catch {
          // ignore cancel-failure: we still want the timeout to win.
        }
        reject(new Error(`${label} connect timed out after ${timeoutMs} ms`));
      }, timeoutMs);
    });
    try {
      return await Promise.race([promise, timeout]);
    } finally {
      if (timer !== undefined) {
        clearTimeout(timer);
      }
    }
  }

  private async connectWebSocket(): Promise<void> {
    const wsUrl = this.url
      .replace(/^https:/, 'wss:')
      .replace(/^http:/, 'ws:');

    const ws = new WebSocket(wsUrl);
    ws.binaryType = 'arraybuffer';

    const openPromise = new Promise<void>((resolve, reject) => {
      ws.onopen = () => {
        this.transport = ws;
        resolve();
      };
      ws.onerror = () => {
        reject(new Error('WebSocket connection failed'));
      };
      ws.onclose = () => {
        this._connected = false;
        this.emit('disconnected', 'websocket closed');
      };
      ws.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer && event.data.byteLength > 0) {
          const view = new Uint8Array(event.data);
          const trackId = view[0];
          const payload = view.subarray(1);
          const trackName = trackId === 1 ? '1.mp4' : '0.mp4';
          this.emit('frame', payload, trackName);
        }
      };
    });

    await this.withConnectTimeout(openPromise, 'WebSocket', () => {
      try {
        ws.close();
      } catch {
        // ignore
      }
    });
  }

  /** Connect to a broadcast-specific WS endpoint that streams fMP4 frames.
   *
   * Wire format: each binary message is `[u8 track_id][fMP4 payload]`,
   * where track_id 0 is video (0.mp4) and track_id 1 is audio (1.mp4).
   *
   * If a token is configured, it is sent as a `Sec-WebSocket-Protocol`
   * subprotocol in the form `lvqr.bearer.<token>`. The server parses the
   * header, strips the prefix, and echoes the exact subprotocol back so the
   * upgrade handshake completes. This keeps tokens out of request URLs and
   * access logs.
   */
  private async connectWebSocketBroadcast(url: string): Promise<void> {
    const protocols = this.options.token
      ? [`lvqr.bearer.${this.options.token}`]
      : undefined;
    const ws = protocols ? new WebSocket(url, protocols) : new WebSocket(url);
    ws.binaryType = 'arraybuffer';

    const openPromise = new Promise<void>((resolve, reject) => {
      ws.onopen = () => {
        this.transport = ws;
        resolve();
      };
      ws.onerror = () => {
        reject(new Error('WebSocket broadcast connection failed'));
      };
      ws.onclose = () => {
        this._connected = false;
        this.emit('disconnected', 'websocket closed');
      };
      ws.onmessage = (event) => {
        if (event.data instanceof ArrayBuffer && event.data.byteLength > 0) {
          const view = new Uint8Array(event.data);
          const trackId = view[0];
          const payload = view.subarray(1);
          const trackName = trackId === 1 ? '1.mp4' : '0.mp4';
          this.emit('frame', payload, trackName);
        }
      };
    });

    await this.withConnectTimeout(openPromise, 'WebSocket broadcast', () => {
      try {
        ws.close();
      } catch {
        // ignore
      }
    });
  }
}

/** Convert hex string to Uint8Array. */
function hexToBytes(hex: string): Uint8Array {
  hex = hex.replace(/[:\- ]/g, '');
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < hex.length; i += 2) {
    bytes[i / 2] = parseInt(hex.substring(i, i + 2), 16);
  }
  return bytes;
}
