/**
 * @lvqr/dvr-player - Drop-in DVR scrub web component for HLS.
 *
 * Wraps an HTML5 video element loaded via hls.js against the relay's
 * live HLS endpoint (which carries a configurable DVR window via
 * `--hls-dvr-window-secs`). Replaces the native controls with a
 * custom seek bar (HH:MM:SS time-axis labels), a LIVE pill that
 * tracks the live-edge delta, a "Go Live" button for the
 * paused-vs-live transition, and a client-side hover-thumbnail
 * strip rendered from a second hls.js instance + a canvas
 * `drawImage` capture.
 *
 * @example
 * ```html
 * <script type="module">
 *   import '@lvqr/dvr-player';
 * </script>
 * <lvqr-dvr-player
 *   src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
 *   token="..."
 *   autoplay
 *   muted
 * ></lvqr-dvr-player>
 * ```
 */

import Hls, { type Events as HlsEvents, type ErrorData, type LevelLoadedData } from 'hls.js';
import { getBooleanAttr, getNumericAttr, getStringAttr, setBooleanAttr } from './internals/attrs.js';
import { dispatchTyped } from './internals/dispatch.js';
import { fractionToTime, timeToFraction, formatTime, generatePercentileLabels, isAtLiveEdge } from './seekbar.js';
import {
  type DvrMarker,
  type DvrMarkerPair,
  type HlsDateRangeLike,
  dvrMarkersFromHlsDateRanges,
  formatDuration,
  groupOutInPairs,
  markerToFraction,
} from './markers.js';
import { broadcastFromHlsSrc, computeLatencyMs, pushSample } from './slo-sampler.js';

const ATTR_SRC = 'src';
const ATTR_AUTOPLAY = 'autoplay';
const ATTR_MUTED = 'muted';
const ATTR_TOKEN = 'token';
const ATTR_THUMBNAILS = 'thumbnails';
const ATTR_LIVE_THRESHOLD = 'live-edge-threshold-secs';
const ATTR_CONTROLS = 'controls';
const ATTR_MARKERS = 'markers';
// Session 156 follow-up: client-side glass-to-glass SLO sampling
// (see `src/slo-sampler.ts`). Default disabled; opt in by setting
// `slo-sampling="enabled"` + `slo-endpoint="<URL>"` on the host.
const ATTR_SLO_SAMPLING = 'slo-sampling';
const ATTR_SLO_ENDPOINT = 'slo-endpoint';
const ATTR_SLO_INTERVAL_SECS = 'slo-sample-interval-secs';

const DEFAULT_LIVE_THRESHOLD_SECS = 6;
const THUMBNAIL_CACHE_CAP = 60;
const LIVE_EDGE_POLL_HZ = 4;
const LIVE_BADGE_DEBOUNCE_MS = 250;
const MARKER_CROSSING_DEBOUNCE_MS = 100;
const MARKER_TOOLTIP_ID_TRUNCATE = 24;
/// Default sample period for SLO pushback. 5 s is loose enough to
/// keep server-side sample-rate metrics manageable but tight enough
/// to surface a latency regression within a few minutes of one
/// occurring. Operators can override via `slo-sample-interval-secs`.
const DEFAULT_SLO_SAMPLE_INTERVAL_SECS = 5;

function getTemplateHTML(): string {
  return /*html*/ `
    <style>
      :host {
        display: block;
        position: relative;
        background: #000;
        color: #fff;
        font-family: system-ui, -apple-system, sans-serif;
        --lvqr-accent: #ff3b30;
        --lvqr-control-bg: rgba(0, 0, 0, 0.55);
        --lvqr-thumb-color: #fff;
        --lvqr-buffered-color: rgba(255, 255, 255, 0.35);
        --lvqr-played-color: var(--lvqr-accent);
        --lvqr-marker-color: rgba(255, 200, 80, 0.45);
        --lvqr-marker-tick-color: #ffc850;
        --lvqr-marker-in-flight: rgba(255, 200, 80, 0.18);
        --lvqr-marker-tooltip-bg: rgba(0, 0, 0, 0.85);
      }
      :host([hidden]) { display: none; }
      .stage { position: relative; width: 100%; height: 100%; }
      video.main {
        width: 100%;
        height: 100%;
        object-fit: contain;
        display: block;
        background: #000;
      }
      video.thumb {
        position: absolute;
        width: 1px;
        height: 1px;
        opacity: 0;
        pointer-events: none;
        left: -9999px;
      }
      .live-overlay {
        position: absolute;
        top: 8px;
        right: 12px;
        display: flex;
        align-items: center;
        gap: 8px;
      }
      .live-badge {
        display: inline-flex;
        align-items: center;
        gap: 6px;
        padding: 4px 8px;
        background: rgba(0, 0, 0, 0.55);
        border-radius: 3px;
        font-size: 12px;
        font-weight: 600;
        letter-spacing: 0.04em;
        color: rgba(255, 255, 255, 0.6);
      }
      .live-badge::before {
        content: '';
        display: inline-block;
        width: 8px;
        height: 8px;
        border-radius: 50%;
        background: rgba(255, 255, 255, 0.4);
      }
      .live-badge.is-live {
        color: #fff;
      }
      .live-badge.is-live::before {
        background: var(--lvqr-accent);
      }
      .go-live-btn {
        background: var(--lvqr-control-bg);
        border: 0;
        color: #fff;
        padding: 4px 8px;
        font-size: 12px;
        font-weight: 600;
        border-radius: 3px;
        cursor: pointer;
        font-family: inherit;
      }
      .go-live-btn[hidden] { display: none; }
      .go-live-btn:hover { background: rgba(0, 0, 0, 0.75); }
      .controls {
        position: absolute;
        left: 0;
        right: 0;
        bottom: 0;
        padding: 32px 12px 8px 12px;
        background: linear-gradient(to top, rgba(0, 0, 0, 0.7), transparent);
        display: grid;
        grid-template-columns: auto 1fr auto;
        align-items: center;
        gap: 12px;
      }
      .ctrl-btn {
        background: transparent;
        border: 0;
        color: #fff;
        padding: 4px 6px;
        cursor: pointer;
        font-size: 14px;
        font-family: inherit;
      }
      .ctrl-btn:focus { outline: 2px solid var(--lvqr-accent); }
      .time-display {
        font-variant-numeric: tabular-nums;
        font-size: 12px;
        color: rgba(255, 255, 255, 0.85);
      }
      .seekbar-wrap {
        position: relative;
        grid-column: 1 / -1;
        height: 28px;
      }
      .seekbar {
        position: absolute;
        left: 0;
        right: 0;
        top: 12px;
        height: 4px;
        background: rgba(255, 255, 255, 0.18);
        cursor: pointer;
        border-radius: 2px;
      }
      .seekbar:hover, .seekbar.is-dragging {
        height: 6px;
        top: 11px;
      }
      .seekbar .buffered {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        background: var(--lvqr-buffered-color);
        border-radius: 2px;
      }
      .seekbar .played {
        position: absolute;
        left: 0;
        top: 0;
        bottom: 0;
        background: var(--lvqr-played-color);
        border-radius: 2px;
      }
      .seekbar .thumb {
        position: absolute;
        top: 50%;
        width: 12px;
        height: 12px;
        margin-left: -6px;
        margin-top: -6px;
        background: var(--lvqr-thumb-color);
        border-radius: 50%;
        opacity: 0;
        transition: opacity 0.1s;
        z-index: 3;
      }
      .seekbar:hover .thumb, .seekbar.is-dragging .thumb {
        opacity: 1;
      }
      .marker-layer {
        position: absolute;
        left: 0;
        right: 0;
        top: 6px;
        bottom: 6px;
        pointer-events: none;
        z-index: 1;
      }
      .marker-layer[hidden] { display: none; }
      .marker-span {
        position: absolute;
        top: 6px;
        bottom: 6px;
        background: var(--lvqr-marker-color);
        pointer-events: none;
      }
      .marker-span.is-open {
        background: var(--lvqr-marker-in-flight);
      }
      .marker {
        position: absolute;
        top: 0;
        bottom: 0;
        width: 10px;
        margin-left: -5px;
        pointer-events: auto;
        cursor: help;
      }
      .marker::before {
        content: '';
        position: absolute;
        left: 4px;
        top: 0;
        bottom: 0;
        width: 2px;
        background: var(--lvqr-marker-tick-color);
      }
      .marker[data-kind="out"]::before {
        top: -2px;
        bottom: -2px;
      }
      .marker[data-kind="unknown"]::before {
        background: rgba(255, 255, 255, 0.5);
      }
      .marker-tooltip {
        position: absolute;
        bottom: 36px;
        background: var(--lvqr-marker-tooltip-bg);
        color: #fff;
        font-size: 11px;
        line-height: 1.35;
        padding: 6px 8px;
        border-radius: 3px;
        pointer-events: none;
        white-space: nowrap;
        font-variant-numeric: tabular-nums;
        z-index: 4;
        display: none;
      }
      .marker-tooltip.is-visible { display: block; }
      .marker-tooltip strong { font-weight: 600; }
      .labels {
        position: absolute;
        left: 0;
        right: 0;
        top: 18px;
        height: 12px;
        font-size: 10px;
        color: rgba(255, 255, 255, 0.55);
        pointer-events: none;
      }
      .labels span {
        position: absolute;
        transform: translateX(-50%);
        font-variant-numeric: tabular-nums;
      }
      .labels span:first-child { transform: translateX(0); }
      .labels span:last-child { transform: translateX(-100%); }
      .preview {
        position: absolute;
        bottom: 36px;
        width: 160px;
        height: 90px;
        margin-left: -80px;
        background: #000;
        border: 1px solid rgba(255, 255, 255, 0.4);
        border-radius: 3px;
        overflow: hidden;
        display: none;
        pointer-events: none;
      }
      .preview.is-visible { display: block; }
      .preview canvas {
        width: 100%;
        height: 100%;
        display: block;
      }
      .preview .preview-time {
        position: absolute;
        left: 0;
        right: 0;
        bottom: 0;
        text-align: center;
        font-size: 11px;
        font-variant-numeric: tabular-nums;
        background: rgba(0, 0, 0, 0.6);
        padding: 2px 0;
      }
      .status {
        position: absolute;
        top: 8px;
        left: 12px;
        font-size: 12px;
        color: rgba(255, 255, 255, 0.7);
        background: rgba(0, 0, 0, 0.55);
        padding: 4px 8px;
        border-radius: 3px;
        font-family: monospace;
      }
      .status:empty { display: none; }
    </style>
    <div class="stage">
      <video class="main" part="video" playsinline></video>
      <video class="thumb" muted playsinline preload="metadata"></video>
      <div class="status" part="status"></div>
      <div class="live-overlay" part="live-overlay">
        <div class="live-badge" part="live-badge">LIVE</div>
        <button type="button" class="go-live-btn" part="go-live-button" hidden>Go Live</button>
      </div>
      <div class="controls" part="controls">
        <button type="button" class="ctrl-btn play-btn" part="play-button" aria-label="Play">▶</button>
        <div class="time-display" part="time-display">--:-- / --:--</div>
        <button type="button" class="ctrl-btn mute-btn" part="mute-button" aria-label="Mute">🔊</button>
        <div class="seekbar-wrap">
          <div class="labels" part="labels"></div>
          <div class="seekbar" part="seekbar" role="slider" tabindex="0" aria-label="Seek">
            <div class="buffered"></div>
            <div class="played"></div>
            <div class="marker-layer" part="markers"></div>
            <div class="thumb"></div>
          </div>
          <div class="marker-tooltip" part="marker-tooltip"></div>
          <div class="preview" part="preview">
            <canvas width="160" height="90"></canvas>
            <div class="preview-time">--:--</div>
          </div>
        </div>
      </div>
    </div>
  `;
}

let templateInstance: HTMLTemplateElement | null = null;
function getTemplate(): HTMLTemplateElement {
  if (templateInstance) return templateInstance;
  const t = document.createElement('template');
  t.innerHTML = getTemplateHTML();
  templateInstance = t;
  return t;
}

export class LvqrDvrPlayerElement extends HTMLElement {
  static get observedAttributes(): string[] {
    return [
      ATTR_SRC,
      ATTR_AUTOPLAY,
      ATTR_MUTED,
      ATTR_TOKEN,
      ATTR_THUMBNAILS,
      ATTR_LIVE_THRESHOLD,
      ATTR_CONTROLS,
      ATTR_MARKERS,
      ATTR_SLO_SAMPLING,
      ATTR_SLO_ENDPOINT,
      ATTR_SLO_INTERVAL_SECS,
    ];
  }

  private shadow: ShadowRoot;
  private videoEl: HTMLVideoElement;
  private thumbVideoEl: HTMLVideoElement;
  private statusEl: HTMLDivElement;
  private liveBadgeEl: HTMLDivElement;
  private goLiveBtnEl: HTMLButtonElement;
  private playBtnEl: HTMLButtonElement;
  private muteBtnEl: HTMLButtonElement;
  private timeDisplayEl: HTMLDivElement;
  private seekBarEl: HTMLDivElement;
  private bufferedFillEl: HTMLDivElement;
  private playedFillEl: HTMLDivElement;
  private thumbHandleEl: HTMLDivElement;
  private labelsEl: HTMLDivElement;
  private previewEl: HTMLDivElement;
  private previewCanvasEl: HTMLCanvasElement;
  private previewTimeEl: HTMLDivElement;
  private markerLayerEl: HTMLDivElement;
  private markerTooltipEl: HTMLDivElement;

  private hls: Hls | null = null;
  private thumbHls: Hls | null = null;
  private targetDurationSecs = 2;
  private isDragging = false;
  private dragPointerId: number | null = null;
  private isAtLiveEdgeState = false;
  private liveEdgePollTimer: number | null = null;
  // Session 156 follow-up: client-side SLO sampling timer.
  private sloSamplingTimer: number | null = null;
  private liveBadgeDebounceTimer: number | null = null;
  private thumbCache = new Map<number, ImageBitmap>();
  private thumbSeekToken = 0;
  private markerStore = new Map<string, DvrMarker>();
  private markerSignature = '';
  private markerCrossingLastEmit = new Map<string, number>();
  private lastCrossingTime: number | null = null;
  private isMarkerHovered = false;

  constructor() {
    super();
    this.shadow = this.attachShadow({ mode: 'open' });
    this.shadow.appendChild(getTemplate().content.cloneNode(true));

    this.videoEl = this.shadow.querySelector('video.main') as HTMLVideoElement;
    this.thumbVideoEl = this.shadow.querySelector('video.thumb') as HTMLVideoElement;
    this.statusEl = this.shadow.querySelector('.status') as HTMLDivElement;
    this.liveBadgeEl = this.shadow.querySelector('.live-badge') as HTMLDivElement;
    this.goLiveBtnEl = this.shadow.querySelector('.go-live-btn') as HTMLButtonElement;
    this.playBtnEl = this.shadow.querySelector('.play-btn') as HTMLButtonElement;
    this.muteBtnEl = this.shadow.querySelector('.mute-btn') as HTMLButtonElement;
    this.timeDisplayEl = this.shadow.querySelector('.time-display') as HTMLDivElement;
    this.seekBarEl = this.shadow.querySelector('.seekbar') as HTMLDivElement;
    this.bufferedFillEl = this.shadow.querySelector('.buffered') as HTMLDivElement;
    this.playedFillEl = this.shadow.querySelector('.played') as HTMLDivElement;
    this.thumbHandleEl = this.shadow.querySelector('.thumb') as HTMLDivElement;
    this.labelsEl = this.shadow.querySelector('.labels') as HTMLDivElement;
    this.previewEl = this.shadow.querySelector('.preview') as HTMLDivElement;
    this.previewCanvasEl = this.shadow.querySelector('.preview canvas') as HTMLCanvasElement;
    this.previewTimeEl = this.shadow.querySelector('.preview-time') as HTMLDivElement;
    this.markerLayerEl = this.shadow.querySelector('.marker-layer') as HTMLDivElement;
    this.markerTooltipEl = this.shadow.querySelector('.marker-tooltip') as HTMLDivElement;

    this.bindHandlers();
  }

  connectedCallback(): void {
    if (getBooleanAttr(this, ATTR_MUTED)) this.videoEl.muted = true;
    this.applyControlsMode();
    this.applyMarkersVisibility();
    if (this.getAttribute(ATTR_SRC)) {
      void this.startPlayback();
    }
    this.startLiveEdgePoll();
  }

  disconnectedCallback(): void {
    this.stop();
  }

  attributeChangedCallback(name: string, _old: string | null, value: string | null): void {
    switch (name) {
      case ATTR_SRC:
        if (value) void this.startPlayback();
        break;
      case ATTR_MUTED:
        this.videoEl.muted = value !== null;
        this.updateMuteIcon();
        break;
      case ATTR_CONTROLS:
        this.applyControlsMode();
        break;
      case ATTR_THUMBNAILS:
        if (value === 'disabled') this.teardownThumbnails();
        break;
      case ATTR_MARKERS:
        this.applyMarkersVisibility();
        this.renderMarkers();
        break;
      case ATTR_SLO_SAMPLING:
      case ATTR_SLO_ENDPOINT:
      case ATTR_SLO_INTERVAL_SECS:
        this.applySloSampling();
        break;
    }
  }

  // Public API.

  play(): Promise<void> {
    return this.videoEl.play();
  }

  pause(): void {
    this.videoEl.pause();
  }

  seek(time: number): void {
    const range = this.seekable();
    if (!range) return;
    const fromTime = this.videoEl.currentTime;
    const toTime = Math.max(range.start, Math.min(range.end, time));
    this.videoEl.currentTime = toTime;
    dispatchTyped(this, 'lvqr-dvr-seek', {
      fromTime,
      toTime,
      isLiveEdge: isAtLiveEdge(range.end - toTime, this.threshold()),
      source: 'programmatic',
    });
  }

  goLive(): void {
    const range = this.seekable();
    if (!range) return;
    const fromTime = this.videoEl.currentTime;
    this.videoEl.currentTime = range.end;
    if (this.videoEl.paused) void this.videoEl.play().catch(() => {});
    dispatchTyped(this, 'lvqr-dvr-seek', {
      fromTime,
      toTime: range.end,
      isLiveEdge: true,
      source: 'user',
    });
  }

  getHlsInstance(): Hls | null {
    return this.hls;
  }

  getMarkers(): { markers: DvrMarker[]; pairs: DvrMarkerPair[] } {
    const markers = [...this.markerStore.values()].sort(
      (a, b) => a.startTime - b.startTime || (a.id < b.id ? -1 : 1),
    );
    const pairs = groupOutInPairs(markers);
    return { markers, pairs };
  }

  // Internals.

  private bindHandlers(): void {
    this.playBtnEl.addEventListener('click', () => this.togglePlay());
    this.muteBtnEl.addEventListener('click', () => this.toggleMute());
    this.goLiveBtnEl.addEventListener('click', () => this.goLive());

    this.videoEl.addEventListener('play', () => (this.playBtnEl.textContent = '⏸'));
    this.videoEl.addEventListener('pause', () => (this.playBtnEl.textContent = '▶'));
    this.videoEl.addEventListener('volumechange', () => this.updateMuteIcon());
    this.videoEl.addEventListener('timeupdate', () => this.onTimeUpdate());
    this.videoEl.addEventListener('progress', () => this.updateBufferedFill());
    this.videoEl.addEventListener('loadedmetadata', () => this.onTimeUpdate());

    this.seekBarEl.addEventListener('pointerdown', (e) => this.onSeekDown(e));
    this.seekBarEl.addEventListener('pointermove', (e) => this.onSeekMove(e));
    this.seekBarEl.addEventListener('pointerup', (e) => this.onSeekUp(e));
    this.seekBarEl.addEventListener('pointercancel', (e) => this.onSeekUp(e));
    this.seekBarEl.addEventListener('pointerleave', () => {
      this.hidePreview();
      this.hideMarkerTooltip();
    });
    this.seekBarEl.addEventListener('keydown', (e) => this.onSeekKey(e));

    this.markerLayerEl.addEventListener('pointerover', (e) => this.onMarkerPointerOver(e));
    this.markerLayerEl.addEventListener('pointerout', (e) => this.onMarkerPointerOut(e));
  }

  private applyControlsMode(): void {
    const mode = getStringAttr(this, ATTR_CONTROLS, 'custom');
    if (mode === 'native') {
      this.videoEl.setAttribute('controls', '');
      const controls = this.shadow.querySelector('.controls') as HTMLElement | null;
      const liveOverlay = this.shadow.querySelector('.live-overlay') as HTMLElement | null;
      if (controls) controls.hidden = true;
      if (liveOverlay) liveOverlay.hidden = true;
    } else {
      this.videoEl.removeAttribute('controls');
      const controls = this.shadow.querySelector('.controls') as HTMLElement | null;
      const liveOverlay = this.shadow.querySelector('.live-overlay') as HTMLElement | null;
      if (controls) controls.hidden = false;
      if (liveOverlay) liveOverlay.hidden = false;
    }
  }

  private async startPlayback(): Promise<void> {
    this.stop();
    this.clearMarkerStore();
    const src = this.getAttribute(ATTR_SRC);
    if (!src) return;

    this.setStatus('connecting...');

    if (this.videoEl.canPlayType('application/vnd.apple.mpegurl') && !Hls.isSupported()) {
      this.videoEl.src = this.applyTokenToUrl(src);
      if (getBooleanAttr(this, ATTR_AUTOPLAY)) void this.videoEl.play().catch(() => {});
      this.setStatus('');
      return;
    }

    if (!Hls.isSupported()) {
      this.setStatus('hls.js not supported in this browser');
      dispatchTyped(this, 'lvqr-dvr-error', {
        code: 'unsupported',
        message: 'hls.js cannot run in this browser',
        fatal: true,
        source: 'component',
      });
      return;
    }

    const token = getStringAttr(this, ATTR_TOKEN);
    this.hls = new Hls({
      lowLatencyMode: true,
      backBufferLength: 60,
      xhrSetup: (xhr: XMLHttpRequest) => {
        if (token) xhr.setRequestHeader('Authorization', `Bearer ${token}`);
      },
    });
    this.hls.on(Hls.Events.LEVEL_LOADED as HlsEvents.LEVEL_LOADED, (_e, data: LevelLoadedData) => {
      this.onLevelLoaded(data);
    });
    this.hls.on(Hls.Events.ERROR as HlsEvents.ERROR, (_e, data: ErrorData) => {
      this.onHlsError(data);
    });
    this.hls.on(Hls.Events.MANIFEST_PARSED as HlsEvents.MANIFEST_PARSED, () => {
      this.setStatus('');
      if (getBooleanAttr(this, ATTR_AUTOPLAY)) void this.videoEl.play().catch(() => {});
    });
    this.hls.loadSource(src);
    this.hls.attachMedia(this.videoEl);
  }

  private stop(): void {
    if (this.hls) {
      try {
        this.hls.destroy();
      } catch {
        // ignore
      }
      this.hls = null;
    }
    this.teardownThumbnails();
    this.videoEl.removeAttribute('src');
    this.videoEl.load();
    if (this.liveEdgePollTimer !== null) {
      clearInterval(this.liveEdgePollTimer);
      this.liveEdgePollTimer = null;
    }
    if (this.liveBadgeDebounceTimer !== null) {
      clearTimeout(this.liveBadgeDebounceTimer);
      this.liveBadgeDebounceTimer = null;
    }
    this.stopSloSampling();
  }

  // Session 156 follow-up: client-side glass-to-glass SLO sampling.
  // Driven by the `slo-sampling` + `slo-endpoint` attributes plus
  // the existing `token` attribute (used as the bearer token; the
  // server's dual-auth path validates it as either an admin or
  // subscribe scope). Default off; opt in by setting both attrs.

  private applySloSampling(): void {
    const enabled = getStringAttr(this, ATTR_SLO_SAMPLING, 'disabled') === 'enabled';
    const endpoint = getStringAttr(this, ATTR_SLO_ENDPOINT, '');
    if (!enabled || !endpoint) {
      this.stopSloSampling();
      return;
    }
    this.startSloSampling();
  }

  private startSloSampling(): void {
    if (this.sloSamplingTimer !== null) return;
    const intervalSecs = getNumericAttr(this, ATTR_SLO_INTERVAL_SECS, DEFAULT_SLO_SAMPLE_INTERVAL_SECS);
    const intervalMs = Math.max(1_000, Math.floor(intervalSecs * 1000));
    this.sloSamplingTimer = window.setInterval(() => {
      void this.fireSloSample();
    }, intervalMs);
  }

  private stopSloSampling(): void {
    if (this.sloSamplingTimer === null) return;
    clearInterval(this.sloSamplingTimer);
    this.sloSamplingTimer = null;
  }

  private async fireSloSample(): Promise<void> {
    // Best-effort: any failure is silently dropped so SLO push
    // cannot disrupt playback. The server-side endpoint emits its
    // own `lvqr_auth_failures_total` + `lvqr_slo_client_samples_total`
    // counters so operators see push success / failure on the
    // metrics surface, not on the client console.
    if (this.videoEl.paused || this.videoEl.readyState < 2) return;
    // `getStartDate()` is a standard HLS extension on
    // HTMLMediaElement (Safari + hls.js implement it) but absent
    // from TypeScript's stock DOM lib; the local interface
    // VideoElementWithStartDate captures the runtime shape.
    const sample = computeLatencyMs(this.videoEl as unknown as Parameters<typeof computeLatencyMs>[0]);
    if (!sample) return;
    const src = this.getAttribute(ATTR_SRC);
    if (!src) return;
    const broadcast = broadcastFromHlsSrc(src);
    if (!broadcast) return;
    const endpoint = getStringAttr(this, ATTR_SLO_ENDPOINT, '');
    if (!endpoint) return;
    const token = this.getAttribute(ATTR_TOKEN) ?? undefined;
    await pushSample({
      endpoint,
      broadcast,
      transport: 'hls',
      ingestTsMs: sample.ingestTsMs,
      renderTsMs: sample.renderTsMs,
      token,
    });
  }

  private teardownThumbnails(): void {
    if (this.thumbHls) {
      try {
        this.thumbHls.destroy();
      } catch {
        // ignore
      }
      this.thumbHls = null;
    }
    this.thumbCache.clear();
    this.thumbVideoEl.removeAttribute('src');
    this.thumbVideoEl.load();
  }

  private onLevelLoaded(data: LevelLoadedData): void {
    const td = data?.details?.targetduration;
    if (typeof td === 'number' && td > 0) this.targetDurationSecs = td;
    this.refreshMarkerStore(data);
  }

  private refreshMarkerStore(data: LevelLoadedData): void {
    const raw = (data?.details as { dateRanges?: Record<string, HlsDateRangeLike | undefined> } | undefined)?.dateRanges
      ?? {};
    const next = dvrMarkersFromHlsDateRanges(raw);
    const sig = this.markerSignatureFor(next);
    if (sig === this.markerSignature) {
      // No change vs the previous LEVEL_LOADED pass; still
      // re-render in case the seekable range moved (the played
      // fill drifts; marker fractions follow).
      this.renderMarkers();
      return;
    }
    this.markerSignature = sig;
    this.markerStore.clear();
    for (const m of next) this.markerStore.set(markerKey(m), m);
    // Drop crossing-throttle entries for evicted markers so a
    // re-emitted ID is not silently suppressed.
    for (const k of [...this.markerCrossingLastEmit.keys()]) {
      if (!this.markerStore.has(k)) this.markerCrossingLastEmit.delete(k);
    }
    this.renderMarkers();
    const pairs = groupOutInPairs(next);
    dispatchTyped(this, 'lvqr-dvr-markers-changed', { markers: next, pairs });
  }

  private markerSignatureFor(markers: ReadonlyArray<DvrMarker>): string {
    return markers
      .map((m) => `${m.id}|${m.kind}|${m.startTime}|${m.durationSecs ?? ''}|${m.scte35Hex ?? ''}`)
      .join('\n');
  }

  private clearMarkerStore(): void {
    if (this.markerStore.size === 0 && this.markerSignature === '') return;
    this.markerStore.clear();
    this.markerSignature = '';
    this.markerCrossingLastEmit.clear();
    this.lastCrossingTime = null;
    this.renderMarkers();
    dispatchTyped(this, 'lvqr-dvr-markers-changed', { markers: [], pairs: [] });
  }

  private applyMarkersVisibility(): void {
    const mode = getStringAttr(this, ATTR_MARKERS, 'visible');
    this.markerLayerEl.hidden = mode === 'hidden';
    if (mode === 'hidden') {
      this.markerTooltipEl.classList.remove('is-visible');
    }
  }

  private renderMarkers(): void {
    const range = this.seekable();
    if (!range || this.markerLayerEl.hidden) {
      this.markerLayerEl.replaceChildren();
      return;
    }
    const markers = [...this.markerStore.values()].sort(
      (a, b) => a.startTime - b.startTime || (a.id < b.id ? -1 : 1),
    );
    const pairs = groupOutInPairs(markers);
    const span = range.end - range.start;
    const frags = document.createDocumentFragment();
    for (const g of pairs) {
      if (g.kind === 'pair' && g.out && g.in) {
        const f1 = markerToFraction(g.out, range);
        const f2 = markerToFraction(g.in, range);
        if (f1 !== null && f2 !== null) {
          frags.appendChild(this.buildSpan(f1, f2, false));
        }
        if (f1 !== null) frags.appendChild(this.buildTick(g.out, f1));
        if (f2 !== null) frags.appendChild(this.buildTick(g.in, f2));
      } else if (g.kind === 'open' && g.out) {
        const f1 = markerToFraction(g.out, range);
        if (f1 !== null) {
          frags.appendChild(this.buildSpan(f1, 1, true));
          frags.appendChild(this.buildTick(g.out, f1));
        }
      } else if (g.kind === 'in-only' && g.in) {
        const f = markerToFraction(g.in, range);
        if (f !== null) frags.appendChild(this.buildTick(g.in, f));
      } else if (g.kind === 'singleton' && g.out) {
        const f = markerToFraction(g.out, range);
        if (f !== null) frags.appendChild(this.buildTick(g.out, f));
      }
    }
    this.markerLayerEl.replaceChildren(frags);
    // Keep a hidden span helper for tests/ poll; tooltip refresh
    // is interaction-driven via pointermove.
    void span;
  }

  private buildTick(marker: DvrMarker, fraction: number): HTMLDivElement {
    const el = document.createElement('div');
    el.className = 'marker';
    el.dataset.id = marker.id;
    el.dataset.kind = marker.kind;
    el.dataset.key = markerKey(marker);
    el.style.left = `${(fraction * 100).toFixed(3)}%`;
    el.setAttribute('role', 'note');
    el.setAttribute(
      'aria-label',
      `SCTE-35 ${marker.kind} marker ${marker.id}`,
    );
    return el;
  }

  private buildSpan(f1: number, f2: number, open: boolean): HTMLDivElement {
    const el = document.createElement('div');
    el.className = open ? 'marker-span is-open' : 'marker-span';
    const lo = Math.min(f1, f2);
    const hi = Math.max(f1, f2);
    el.style.left = `${(lo * 100).toFixed(3)}%`;
    el.style.width = `${((hi - lo) * 100).toFixed(3)}%`;
    return el;
  }

  private showMarkerTooltip(marker: DvrMarker, anchorFraction: number): void {
    const range = this.seekable();
    if (!range) return;
    const span = range.end - range.start;
    const truncatedId =
      marker.id.length > MARKER_TOOLTIP_ID_TRUNCATE
        ? `${marker.id.slice(0, MARKER_TOOLTIP_ID_TRUNCATE)}...`
        : marker.id;
    const kindLabel: Record<string, string> = {
      out: 'Out',
      in: 'In',
      cmd: 'Cue',
      unknown: 'Marker',
    };
    const lines = [
      `<strong>${escapeHtml(kindLabel[marker.kind] ?? 'Marker')}</strong>`,
      `id: ${escapeHtml(truncatedId)}`,
      `t: ${escapeHtml(formatTime(marker.startTime - range.start, span))}`,
    ];
    if (marker.durationSecs !== null) lines.push(`dur: ${formatDuration(marker.durationSecs)}`);
    if (marker.class && marker.class !== 'urn:scte:scte35:2014:bin') {
      lines.push(`class: ${escapeHtml(marker.class)}`);
    }
    this.markerTooltipEl.innerHTML = lines.join('<br>');
    this.markerTooltipEl.style.left = `${(anchorFraction * 100).toFixed(3)}%`;
    this.markerTooltipEl.style.transform = 'translateX(-50%)';
    this.markerTooltipEl.classList.add('is-visible');
    // Suppress thumbnail preview while a marker tooltip is up.
    this.previewEl.classList.remove('is-visible');
  }

  private hideMarkerTooltip(): void {
    this.markerTooltipEl.classList.remove('is-visible');
  }

  private onMarkerPointerOver(e: PointerEvent): void {
    const target = e.target as HTMLElement | null;
    if (!target || !target.classList.contains('marker')) return;
    const key = target.dataset.key;
    if (!key) return;
    const marker = this.markerStore.get(key);
    if (!marker) return;
    const range = this.seekable();
    if (!range) return;
    const f = markerToFraction(marker, range);
    if (f === null) return;
    this.isMarkerHovered = true;
    this.showMarkerTooltip(marker, f);
  }

  private onMarkerPointerOut(e: PointerEvent): void {
    const related = e.relatedTarget as HTMLElement | null;
    if (related && related.classList.contains('marker')) return;
    this.isMarkerHovered = false;
    this.hideMarkerTooltip();
  }

  private maybeEmitCrossings(): void {
    if (this.markerStore.size === 0) {
      this.lastCrossingTime = this.videoEl.currentTime;
      return;
    }
    const t = this.videoEl.currentTime;
    const prev = this.lastCrossingTime;
    this.lastCrossingTime = t;
    if (prev === null || prev === t) return;
    const direction: 'forward' | 'backward' = t > prev ? 'forward' : 'backward';
    const lo = Math.min(prev, t);
    const hi = Math.max(prev, t);
    const now = Date.now();
    for (const [key, marker] of this.markerStore) {
      const st = marker.startTime;
      if (!Number.isFinite(st)) continue;
      // strict-low / inclusive-high so a single tick is emitted
      // per crossing (no double-fire when t lands exactly on st).
      if (st <= lo || st > hi) continue;
      const last = this.markerCrossingLastEmit.get(key) ?? 0;
      if (now - last < MARKER_CROSSING_DEBOUNCE_MS) continue;
      this.markerCrossingLastEmit.set(key, now);
      dispatchTyped(this, 'lvqr-dvr-marker-crossed', {
        marker,
        direction,
        currentTime: t,
      });
    }
  }

  private onHlsError(data: ErrorData): void {
    if (!data.fatal) return;
    this.setStatus(`error: ${data.details}`);
    dispatchTyped(this, 'lvqr-dvr-error', {
      code: data.details ?? 'unknown',
      message: (data.error as Error | undefined)?.message ?? String(data.details ?? 'hls error'),
      fatal: true,
      source: 'hls.js',
    });
  }

  private onTimeUpdate(): void {
    this.updatePlayedFill();
    this.updateLabels();
    this.updateTimeDisplay();
    this.maybeUpdateLiveBadge();
    this.maybeEmitCrossings();
  }

  private updatePlayedFill(): void {
    const range = this.seekable();
    if (!range) {
      this.playedFillEl.style.width = '0%';
      return;
    }
    const f = timeToFraction(this.videoEl.currentTime, range);
    this.playedFillEl.style.width = `${(f * 100).toFixed(2)}%`;
    this.thumbHandleEl.style.left = `${(f * 100).toFixed(2)}%`;
  }

  private updateBufferedFill(): void {
    const range = this.seekable();
    if (!range || this.videoEl.buffered.length === 0) {
      this.bufferedFillEl.style.width = '0%';
      return;
    }
    const lastEnd = this.videoEl.buffered.end(this.videoEl.buffered.length - 1);
    const f = timeToFraction(lastEnd, range);
    this.bufferedFillEl.style.width = `${(f * 100).toFixed(2)}%`;
  }

  private updateLabels(): void {
    const range = this.seekable();
    if (!range) {
      this.labelsEl.innerHTML = '';
      return;
    }
    const labels = generatePercentileLabels(range);
    this.labelsEl.innerHTML = labels
      .map((l) => `<span style="left:${(l.fraction * 100).toFixed(2)}%">${escapeHtml(l.text)}</span>`)
      .join('');
  }

  private updateTimeDisplay(): void {
    const range = this.seekable();
    if (!range) {
      this.timeDisplayEl.textContent = '--:-- / --:--';
      return;
    }
    const span = range.end - range.start;
    const cur = formatTime(this.videoEl.currentTime - range.start, span);
    const total = formatTime(span, span);
    this.timeDisplayEl.textContent = `${cur} / ${total}`;
  }

  private maybeUpdateLiveBadge(): void {
    if (this.liveBadgeDebounceTimer !== null) return;
    this.liveBadgeDebounceTimer = window.setTimeout(() => {
      this.liveBadgeDebounceTimer = null;
      this.updateLiveBadge();
    }, LIVE_BADGE_DEBOUNCE_MS);
  }

  private updateLiveBadge(): void {
    const range = this.seekable();
    if (!range) return;
    const delta = range.end - this.videoEl.currentTime;
    const threshold = this.threshold();
    const live = isAtLiveEdge(delta, threshold);
    if (live === this.isAtLiveEdgeState) return;
    this.isAtLiveEdgeState = live;
    this.liveBadgeEl.classList.toggle('is-live', live);
    this.goLiveBtnEl.hidden = live;
    dispatchTyped(this, 'lvqr-dvr-live-edge-changed', {
      isAtLiveEdge: live,
      deltaSecs: delta,
      thresholdSecs: threshold,
    });
  }

  private startLiveEdgePoll(): void {
    if (this.liveEdgePollTimer !== null) return;
    const intervalMs = Math.max(50, Math.floor(1000 / LIVE_EDGE_POLL_HZ));
    this.liveEdgePollTimer = window.setInterval(() => this.updateLiveBadge(), intervalMs);
  }

  private togglePlay(): void {
    if (this.videoEl.paused) void this.videoEl.play().catch(() => {});
    else this.videoEl.pause();
  }

  private toggleMute(): void {
    this.videoEl.muted = !this.videoEl.muted;
    setBooleanAttr(this, ATTR_MUTED, this.videoEl.muted);
  }

  private updateMuteIcon(): void {
    this.muteBtnEl.textContent = this.videoEl.muted ? '🔇' : '🔊';
  }

  private onSeekDown(e: PointerEvent): void {
    if (e.button !== 0) return;
    this.isDragging = true;
    this.dragPointerId = e.pointerId;
    this.seekBarEl.classList.add('is-dragging');
    this.seekBarEl.setPointerCapture(e.pointerId);
    this.seekFromPointer(e, 'user');
    e.preventDefault();
  }

  private onSeekMove(e: PointerEvent): void {
    this.maybeShowPreview(e);
    if (!this.isDragging) return;
    this.seekFromPointer(e, 'user');
  }

  private onSeekUp(e: PointerEvent): void {
    if (!this.isDragging) return;
    this.isDragging = false;
    this.seekBarEl.classList.remove('is-dragging');
    if (this.dragPointerId !== null && this.seekBarEl.hasPointerCapture(this.dragPointerId)) {
      this.seekBarEl.releasePointerCapture(this.dragPointerId);
    }
    this.dragPointerId = null;
    this.seekFromPointer(e, 'user');
  }

  private onSeekKey(e: KeyboardEvent): void {
    const range = this.seekable();
    if (!range) return;
    let delta = 0;
    if (e.key === 'ArrowLeft') delta = -5;
    else if (e.key === 'ArrowRight') delta = 5;
    else if (e.key === 'Home') delta = range.start - this.videoEl.currentTime;
    else if (e.key === 'End') delta = range.end - this.videoEl.currentTime;
    else return;
    e.preventDefault();
    this.seek(this.videoEl.currentTime + delta);
  }

  private seekFromPointer(e: PointerEvent, source: 'user' | 'programmatic'): void {
    const range = this.seekable();
    if (!range) return;
    const rect = this.seekBarEl.getBoundingClientRect();
    const fraction = (e.clientX - rect.left) / rect.width;
    const fromTime = this.videoEl.currentTime;
    const toTime = fractionToTime(fraction, range);
    this.videoEl.currentTime = toTime;
    dispatchTyped(this, 'lvqr-dvr-seek', {
      fromTime,
      toTime,
      isLiveEdge: isAtLiveEdge(range.end - toTime, this.threshold()),
      source,
    });
  }

  private maybeShowPreview(e: PointerEvent): void {
    const mode = getStringAttr(this, ATTR_THUMBNAILS, 'enabled');
    if (mode === 'disabled') return;
    if (this.isMarkerHovered) return;
    const range = this.seekable();
    if (!range) return;
    const rect = this.seekBarEl.getBoundingClientRect();
    const fraction = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    const previewTime = fractionToTime(fraction, range);
    const span = range.end - range.start;
    this.previewEl.style.left = `${(fraction * 100).toFixed(2)}%`;
    this.previewEl.classList.add('is-visible');
    this.previewTimeEl.textContent = formatTime(previewTime - range.start, span);
    void this.renderThumbnail(Math.round(previewTime));
  }

  private hidePreview(): void {
    this.previewEl.classList.remove('is-visible');
  }

  private async renderThumbnail(timeRounded: number): Promise<void> {
    const ctx = this.previewCanvasEl.getContext('2d');
    if (!ctx) return;
    const cached = this.thumbCache.get(timeRounded);
    if (cached) {
      ctx.drawImage(cached, 0, 0, this.previewCanvasEl.width, this.previewCanvasEl.height);
      return;
    }
    const src = this.getAttribute(ATTR_SRC);
    if (!src) return;
    if (!this.thumbHls && Hls.isSupported()) {
      const token = getStringAttr(this, ATTR_TOKEN);
      this.thumbHls = new Hls({
        backBufferLength: 30,
        maxBufferLength: 10,
        xhrSetup: (xhr: XMLHttpRequest) => {
          if (token) xhr.setRequestHeader('Authorization', `Bearer ${token}`);
        },
      });
      this.thumbHls.loadSource(src);
      this.thumbHls.attachMedia(this.thumbVideoEl);
    }
    const myToken = ++this.thumbSeekToken;
    try {
      this.thumbVideoEl.currentTime = timeRounded;
      await waitForSeek(this.thumbVideoEl);
      if (myToken !== this.thumbSeekToken) return;
      ctx.drawImage(this.thumbVideoEl, 0, 0, this.previewCanvasEl.width, this.previewCanvasEl.height);
      if (typeof createImageBitmap === 'function') {
        const bitmap = await createImageBitmap(this.previewCanvasEl);
        if (this.thumbCache.size >= THUMBNAIL_CACHE_CAP) {
          const oldest = this.thumbCache.keys().next().value;
          if (oldest !== undefined) {
            this.thumbCache.get(oldest)?.close?.();
            this.thumbCache.delete(oldest);
          }
        }
        this.thumbCache.set(timeRounded, bitmap);
      }
    } catch {
      // ignore -- best-effort preview
    }
  }

  private seekable(): { start: number; end: number } | null {
    if (this.videoEl.seekable.length === 0) return null;
    return {
      start: this.videoEl.seekable.start(0),
      end: this.videoEl.seekable.end(this.videoEl.seekable.length - 1),
    };
  }

  private threshold(): number {
    const explicit = getNumericAttr(this, ATTR_LIVE_THRESHOLD, NaN);
    if (Number.isFinite(explicit) && explicit > 0) return explicit;
    return Math.max(DEFAULT_LIVE_THRESHOLD_SECS, 3 * this.targetDurationSecs);
  }

  private applyTokenToUrl(src: string): string {
    const token = getStringAttr(this, ATTR_TOKEN);
    if (!token) return src;
    try {
      const url = new URL(src);
      url.searchParams.set('token', token);
      return url.toString();
    } catch {
      return src;
    }
  }

  private setStatus(text: string): void {
    this.statusEl.textContent = text;
  }
}

function waitForSeek(v: HTMLVideoElement): Promise<void> {
  return new Promise((resolve) => {
    if (v.readyState >= 2 && !v.seeking) {
      resolve();
      return;
    }
    const onSeeked = () => {
      v.removeEventListener('seeked', onSeeked);
      resolve();
    };
    v.addEventListener('seeked', onSeeked);
    setTimeout(() => {
      v.removeEventListener('seeked', onSeeked);
      resolve();
    }, 1500);
  });
}

function markerKey(m: { id: string; kind: string }): string {
  return `${m.id}|${m.kind}`;
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case '&': return '&amp;';
      case '<': return '&lt;';
      case '>': return '&gt;';
      case '"': return '&quot;';
      case "'": return '&#39;';
      default: return c;
    }
  });
}

if (typeof customElements !== 'undefined' && !customElements.get('lvqr-dvr-player')) {
  customElements.define('lvqr-dvr-player', LvqrDvrPlayerElement);
}

declare global {
  interface HTMLElementTagNameMap {
    'lvqr-dvr-player': LvqrDvrPlayerElement;
  }
}
