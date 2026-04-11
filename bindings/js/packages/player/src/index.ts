/**
 * @lvqr/player - Drop-in video player Web Component
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

    // Parse URL: "https://relay:4443/live/stream" -> url="https://relay:4443", broadcast="live/stream"
    const url = new URL(src);
    const broadcast = url.pathname.replace(/^\//, '');
    const relayUrl = `${url.protocol}//${url.host}`;

    this.setStatus('connecting...');

    try {
      this.client = new LvqrClient(relayUrl, {
        fingerprint: this.getAttribute('fingerprint') ?? undefined,
      });

      this.client.on('connected', () => {
        this.setStatus('connected');
      });

      this.client.on('error', (err) => {
        this.setStatus(`error: ${err.message}`);
      });

      this.client.on('disconnected', (reason) => {
        this.setStatus(reason ? `disconnected: ${reason}` : 'disconnected');
      });

      await this.client.connect();
      await this.client.subscribe(broadcast);

      this.setStatus('');
    } catch (err) {
      this.setStatus(`failed: ${err}`);
    }
  }

  /** Stop playback and disconnect. */
  stop(): void {
    this.client?.close();
    this.client = null;
    this.setStatus('');
  }

  private setStatus(text: string): void {
    this.statusEl.textContent = text;
  }
}

// Register the custom element
if (typeof customElements !== 'undefined' && !customElements.get('lvqr-player')) {
  customElements.define('lvqr-player', LvqrPlayerElement);
}
