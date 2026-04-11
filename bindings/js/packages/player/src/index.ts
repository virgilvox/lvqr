/**
 * @lvqr/player - Drop-in video player Web Component
 *
 * Connects to an LVQR relay via MoQ-Lite over WebTransport and renders
 * live video using MSE (MediaSource Extensions).
 *
 * @example
 * ```html
 * <script type="module">
 *   import '@lvqr/player';
 * </script>
 * <lvqr-player src="https://relay.example.com:4443/live/stream"></lvqr-player>
 * ```
 */

import { LvqrClient } from '@lvqr/core';

/**
 * `<lvqr-player>` Web Component for live video playback.
 *
 * Attributes:
 * - `src`: Relay URL with stream path (required)
 * - `autoplay`: Start playback automatically
 * - `muted`: Start muted
 * - `fingerprint`: TLS cert fingerprint for development
 */
export class LvqrPlayerElement extends HTMLElement {
  private client: LvqrClient | null = null;
  private videoEl: HTMLVideoElement;
  private statusEl: HTMLDivElement;
  private shadow: ShadowRoot;
  private mediaSource: MediaSource | null = null;
  private sourceBuffer: SourceBuffer | null = null;
  private pendingBuffers: Uint8Array[] = [];
  private mimeType = 'video/mp4; codecs="avc1.64001F,mp4a.40.2"';

  static get observedAttributes(): string[] {
    return ['src', 'autoplay', 'muted', 'fingerprint'];
  }

  constructor() {
    super();
    this.shadow = this.attachShadow({ mode: 'open' });

    this.shadow.innerHTML = `
      <style>
        :host {
          display: block;
          position: relative;
          background: #000;
          overflow: hidden;
        }
        video {
          width: 100%;
          height: 100%;
          object-fit: contain;
        }
        .status {
          position: absolute;
          bottom: 8px;
          left: 8px;
          font-family: monospace;
          font-size: 12px;
          color: rgba(255,255,255,0.6);
          background: rgba(0,0,0,0.5);
          padding: 2px 6px;
          border-radius: 2px;
          pointer-events: none;
        }
        .status:empty {
          display: none;
        }
      </style>
      <video part="video"></video>
      <div class="status" part="status"></div>
    `;

    this.videoEl = this.shadow.querySelector('video')!;
    this.statusEl = this.shadow.querySelector('.status')!;
  }

  connectedCallback(): void {
    if (this.hasAttribute('muted')) {
      this.videoEl.muted = true;
    }
    if (this.hasAttribute('autoplay') && this.getAttribute('src')) {
      this.startPlayback();
    }
  }

  disconnectedCallback(): void {
    this.stop();
  }

  attributeChangedCallback(name: string, _old: string | null, value: string | null): void {
    if (name === 'src' && value && this.hasAttribute('autoplay')) {
      this.startPlayback();
    }
    if (name === 'muted') {
      this.videoEl.muted = value !== null;
    }
  }

  /** Start playback. */
  async startPlayback(): Promise<void> {
    const src = this.getAttribute('src');
    if (!src) return;

    // Parse URL: "https://relay:4443/live/stream" -> relayUrl + broadcast path
    const url = new URL(src);
    const broadcast = url.pathname.replace(/^\//, '');
    const relayUrl = `${url.protocol}//${url.host}`;

    this.setStatus('connecting...');

    try {
      this.client = new LvqrClient(relayUrl, {
        fingerprint: this.getAttribute('fingerprint') ?? undefined,
      });

      this.client.on('connected', () => {
        this.setStatus('connected, waiting for video...');
      });

      this.client.on('error', (err) => {
        this.setStatus(`error: ${err.message}`);
      });

      this.client.on('disconnected', (reason) => {
        this.setStatus(reason ? `disconnected: ${reason}` : 'disconnected');
      });

      // Set up MSE before connecting
      this.setupMediaSource();

      // Connect and subscribe
      await this.client.connect();

      // Receive fMP4 frames and feed to MSE
      this.client.on('frame', (data: Uint8Array, _track: string) => {
        this.appendToBuffer(data);
      });

      await this.client.subscribe(broadcast);
    } catch (err) {
      this.setStatus(`failed: ${err}`);
    }
  }

  /** Stop playback and disconnect. */
  stop(): void {
    this.client?.close();
    this.client = null;

    if (this.mediaSource?.readyState === 'open') {
      try {
        this.mediaSource.endOfStream();
      } catch {
        // ignore
      }
    }
    this.mediaSource = null;
    this.sourceBuffer = null;
    this.pendingBuffers = [];
    this.setStatus('');
  }

  private setupMediaSource(): void {
    this.mediaSource = new MediaSource();
    this.videoEl.src = URL.createObjectURL(this.mediaSource);

    this.mediaSource.addEventListener('sourceopen', () => {
      if (!this.mediaSource) return;

      try {
        this.sourceBuffer = this.mediaSource.addSourceBuffer(this.mimeType);
        this.sourceBuffer.mode = 'sequence';

        this.sourceBuffer.addEventListener('updateend', () => {
          this.flushPending();
        });

        this.sourceBuffer.addEventListener('error', () => {
          this.setStatus('buffer error');
        });

        // Flush any frames that arrived before sourceopen
        this.flushPending();
      } catch (e) {
        this.setStatus(`MSE error: ${e}`);
      }
    });
  }

  private appendToBuffer(data: Uint8Array): void {
    this.pendingBuffers.push(data);
    this.flushPending();
  }

  private flushPending(): void {
    if (!this.sourceBuffer || this.sourceBuffer.updating || this.pendingBuffers.length === 0) {
      return;
    }

    const data = this.pendingBuffers.shift()!;
    try {
      this.sourceBuffer.appendBuffer(new Uint8Array(data) as unknown as ArrayBuffer);

      // Auto-play once we have data
      if (this.videoEl.paused && this.videoEl.readyState >= 2) {
        this.videoEl.play().catch(() => {
          // Autoplay may be blocked; user interaction needed
        });
        this.setStatus('');
      }
    } catch (e) {
      // QuotaExceededError: trim old buffered data
      if (e instanceof DOMException && e.name === 'QuotaExceededError') {
        this.trimBuffer();
        this.pendingBuffers.unshift(data);
      }
    }
  }

  private trimBuffer(): void {
    if (!this.sourceBuffer || this.sourceBuffer.updating) return;

    const buffered = this.sourceBuffer.buffered;
    if (buffered.length > 0) {
      const start = buffered.start(0);
      const end = buffered.end(buffered.length - 1);
      // Keep only the last 10 seconds
      if (end - start > 10) {
        try {
          this.sourceBuffer.remove(start, end - 10);
        } catch {
          // ignore
        }
      }
    }
  }

  private setStatus(text: string): void {
    this.statusEl.textContent = text;
  }
}

// Register the custom element
if (typeof customElements !== 'undefined' && !customElements.get('lvqr-player')) {
  customElements.define('lvqr-player', LvqrPlayerElement);
}
