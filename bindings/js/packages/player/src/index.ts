/**
 * @lvqr/player - Drop-in video player Web Component
 *
 * Connects to an LVQR relay via MoQ-Lite over WebTransport (or WebSocket
 * fallback) and renders live video + audio using MSE (MediaSource Extensions).
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

interface TrackBuffer {
  sourceBuffer: SourceBuffer | null;
  pending: Uint8Array[];
  initReceived: boolean;
}

/**
 * `<lvqr-player>` Web Component for live video + audio playback.
 *
 * Attributes:
 * - `src`: Relay URL with stream path (required)
 * - `autoplay`: Start playback automatically
 * - `muted`: Start muted
 * - `fingerprint`: TLS cert fingerprint for development
 * - `token`: Optional viewer token for relays that require auth
 */
export class LvqrPlayerElement extends HTMLElement {
  private client: LvqrClient | null = null;
  private videoEl: HTMLVideoElement;
  private statusEl: HTMLDivElement;
  private shadow: ShadowRoot;
  private mediaSource: MediaSource | null = null;

  // One buffer per track. Track names match the LVQR convention:
  //   "0.mp4" -> video, "1.mp4" -> audio
  private buffers: Map<string, TrackBuffer> = new Map();

  static get observedAttributes(): string[] {
    return ['src', 'autoplay', 'muted', 'fingerprint', 'token'];
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
      <video part="video" playsinline></video>
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

    const url = new URL(src);
    const broadcast = url.pathname.replace(/^\//, '');
    const relayUrl = `${url.protocol}//${url.host}`;

    this.setStatus('connecting...');

    try {
      this.client = new LvqrClient(relayUrl, {
        fingerprint: this.getAttribute('fingerprint') ?? undefined,
        token: this.getAttribute('token') ?? undefined,
      });

      this.client.on('connected', () => {
        this.setStatus('connected, waiting for media...');
      });

      this.client.on('error', (err) => {
        this.setStatus(`error: ${err.message}`);
      });

      this.client.on('disconnected', (reason) => {
        this.setStatus(reason ? `disconnected: ${reason}` : 'disconnected');
      });

      // Set up MSE. SourceBuffers are added lazily as init segments arrive.
      this.mediaSource = new MediaSource();
      this.videoEl.src = URL.createObjectURL(this.mediaSource);

      await this.client.connect();

      this.client.on('frame', (data: Uint8Array, track: string) => {
        this.handleFrame(data, track);
      });

      // Subscribe to both video and audio tracks.
      await this.client.subscribe(broadcast, ['0.mp4', '1.mp4']);
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
    this.buffers.clear();
    this.setStatus('');
  }

  /** Handle an incoming fMP4 frame for a specific track. */
  private handleFrame(data: Uint8Array, track: string): void {
    let buf = this.buffers.get(track);
    if (!buf) {
      buf = { sourceBuffer: null, pending: [], initReceived: false };
      this.buffers.set(track, buf);
    }

    // Detect init segment by checking for the 'ftyp' box.
    if (!buf.initReceived && data.length > 8 && isInitSegment(data)) {
      buf.initReceived = true;

      const mimeType = mimeForTrack(track, data);
      const setupBuffer = () => this.createSourceBuffer(track, mimeType);
      if (this.mediaSource?.readyState === 'open') {
        setupBuffer();
      } else {
        this.mediaSource?.addEventListener('sourceopen', setupBuffer, { once: true });
      }
    }

    buf.pending.push(data);
    this.flushPending(track);
  }

  private createSourceBuffer(track: string, mimeType: string): void {
    const buf = this.buffers.get(track);
    if (!this.mediaSource || !buf || buf.sourceBuffer) return;

    try {
      const sb = this.mediaSource.addSourceBuffer(mimeType);
      // Video tolerates non-monotonic DTS from browser encoders in `sequence`
      // mode, which stamps frames by append order. Audio MUST stay in the
      // default `segments` mode so that MSE honors the fMP4 baseMediaDecodeTime
      // and keeps A/V lock; forcing audio into `sequence` drifts the two tracks
      // apart over time (audit finding, 2026-04-10).
      if (track === '0.mp4') {
        sb.mode = 'sequence';
      }
      sb.addEventListener('updateend', () => this.flushPending(track));
      sb.addEventListener('error', () => this.setStatus(`buffer error (${track})`));
      buf.sourceBuffer = sb;
      this.flushPending(track);
    } catch (e) {
      this.setStatus(`MSE error (${track}): ${e}`);
    }
  }

  private flushPending(track: string): void {
    const buf = this.buffers.get(track);
    if (!buf || !buf.sourceBuffer || buf.sourceBuffer.updating || buf.pending.length === 0) {
      return;
    }
    const data = buf.pending.shift()!;
    try {
      buf.sourceBuffer.appendBuffer(new Uint8Array(data) as unknown as ArrayBuffer);
      this.maybeStartPlayback();
    } catch (e) {
      if (e instanceof DOMException && e.name === 'QuotaExceededError') {
        this.trimAllBuffers();
        buf.pending.unshift(data);
      }
    }
  }

  private trimAllBuffers(): void {
    for (const buf of this.buffers.values()) {
      if (!buf.sourceBuffer || buf.sourceBuffer.updating) continue;
      const buffered = buf.sourceBuffer.buffered;
      if (buffered.length > 0) {
        const start = buffered.start(0);
        const end = buffered.end(buffered.length - 1);
        if (end - start > 10) {
          try {
            buf.sourceBuffer.remove(start, end - 10);
          } catch {
            // ignore
          }
        }
      }
    }
  }

  private maybeStartPlayback(): void {
    if (this.videoEl.paused && this.videoEl.readyState >= 2) {
      this.videoEl.play().catch(() => {});
      this.setStatus('');
    }
  }

  private setStatus(text: string): void {
    this.statusEl.textContent = text;
  }
}

/** Check if data starts with an ftyp box (fMP4 init segment). */
function isInitSegment(data: Uint8Array): boolean {
  return data[4] === 0x66 && data[5] === 0x74 && data[6] === 0x79 && data[7] === 0x70; // "ftyp"
}

/** Pick the right MSE MIME type for a track based on its name and init data. */
function mimeForTrack(track: string, initData: Uint8Array): string {
  if (track === '1.mp4') {
    // Audio: AAC-LC by convention. Could be parsed from esds, but LVQR
    // currently only emits AAC-LC.
    return 'audio/mp4; codecs="mp4a.40.2"';
  }
  // Default: video
  return `video/mp4; codecs="${extractCodecFromInit(initData)}"`;
}

/**
 * Extract H.264 codec string from an fMP4 init segment by finding the avcC box.
 * Falls back to a generic high-profile codec if parsing fails.
 */
function extractCodecFromInit(data: Uint8Array): string {
  // Scan for "avcC" box in the init segment
  for (let i = 0; i < data.length - 8; i++) {
    if (data[i + 4] === 0x61 && data[i + 5] === 0x76 && data[i + 6] === 0x63 && data[i + 7] === 0x43) {
      // avcC found at offset i. Box payload starts at i+8.
      // AVCDecoderConfigurationRecord: [version][profile][compat][level]
      const payload = i + 8;
      if (payload + 4 <= data.length) {
        const profile = data[payload + 1];
        const compat = data[payload + 2];
        const level = data[payload + 3];
        return `avc1.${hex(profile)}${hex(compat)}${hex(level)}`;
      }
    }
  }
  // Fallback: High profile, level 3.1
  return 'avc1.64001F';
}

function hex(n: number): string {
  return n.toString(16).toUpperCase().padStart(2, '0');
}

// Register the custom element
if (typeof customElements !== 'undefined' && !customElements.get('lvqr-player')) {
  customElements.define('lvqr-player', LvqrPlayerElement);
}
