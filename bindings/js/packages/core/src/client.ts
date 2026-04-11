/**
 * LVQR streaming client.
 *
 * Connects to an LVQR relay via WebTransport (preferred) or WebSocket (fallback)
 * and subscribes to live video tracks.
 */

import { detectTransport, type TransportType } from './transport';

export interface LvqrClientOptions {
  /** Force a specific transport. Default: auto-detect. */
  transport?: TransportType;
  /** SHA-256 fingerprint for self-signed certs (development). */
  fingerprint?: string;
}

export interface LvqrEvents {
  /** Received a video/audio frame. */
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
 * client.on('frame', (data) => { ... });
 * await client.subscribe('live/my-stream');
 * ```
 */
export class LvqrClient {
  private url: string;
  private options: LvqrClientOptions;
  private transport: WebTransport | WebSocket | null = null;
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

  /** Subscribe to a broadcast track. */
  async subscribe(_broadcast: string, _track = 'video'): Promise<void> {
    if (!this._connected) {
      throw new Error('Not connected. Call connect() first.');
    }
    // MoQ subscription will be handled here once the protocol
    // framing is implemented in the WASM module or in TypeScript.
    // For now, this sets up the subscription intent.
  }

  /** Close the connection. */
  close(): void {
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

    await wt.ready;
    this.transport = wt;
  }

  private async connectWebSocket(): Promise<void> {
    const wsUrl = this.url
      .replace(/^https:/, 'wss:')
      .replace(/^http:/, 'ws:');

    return new Promise((resolve, reject) => {
      const ws = new WebSocket(wsUrl);
      ws.binaryType = 'arraybuffer';

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
        if (event.data instanceof ArrayBuffer) {
          this.emit('frame', new Uint8Array(event.data), 'video');
        }
      };
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
