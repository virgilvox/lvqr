import { computed, onBeforeUnmount, ref } from 'vue';

/**
 * Browser-side WHIP publisher per draft-ietf-wish-whip. Wraps the standard
 * dance:
 *
 *   1. getUserMedia (camera/mic, with optional resolution/frameRate constraints)
 *   2. construct an RTCPeerConnection + addTrack for each captured track
 *   3. createOffer + setLocalDescription
 *   4. wait for icegatheringstate === 'complete' (non-trickle WHIP)
 *   5. POST the SDP offer to /whip/{broadcast} with `Content-Type:
 *      application/sdp` and an optional `Authorization: Bearer <token>`
 *   6. parse the `Location` response header (the resource URL) and the SDP
 *      answer body; setRemoteDescription(answer)
 *   7. on stop, DELETE the resource URL + close the PC + stop the tracks
 *
 * The composable exposes reactive state for the view to bind against
 * (state, lastError, sessionUrl, stats), and pure helpers for testing.
 *
 * Trickle-ICE PATCH is intentionally NOT used today; LVQR's WHIP server
 * accepts non-trickle and the simpler shape buys us a smaller error surface
 * for the v1.x demo. We can layer trickle on top later by wiring an
 * RTCPeerConnection.onicecandidate handler that PATCHes
 * /whip/{broadcast}/{session_id} with `application/trickle-ice-sdpfrag`.
 */

export type PublishState =
  | 'idle'
  | 'requesting-media'
  | 'gathering-ice'
  | 'posting-offer'
  | 'connected'
  | 'stopping'
  | 'error';

export interface PublishOptions {
  /** Full WHIP endpoint URL, e.g. `http://localhost:8443/whip/live/demo`. */
  whipUrl: string;
  /** Optional bearer token; sent as `Authorization: Bearer ...` on the POST. */
  bearerToken?: string;
  /** Capture preferences. */
  video: boolean;
  audio: boolean;
  width?: number;
  height?: number;
  frameRate?: number;
  /**
   * Capture source. `camera` = getUserMedia; `screen` = getDisplayMedia
   * (screen-share, no microphone unless audio: true).
   */
  source: 'camera' | 'screen';
}

export interface PublishStatsSnapshot {
  /** Cumulative bytes sent over the peer connection. */
  bytesSent: number;
  /** Per-second bitrate, smoothed across the last sample window. */
  bitsPerSecond: number;
  /** Most recent round-trip-time in milliseconds (selected ICE pair). */
  rttMs?: number;
  /** Frame rate observed at the encoder. */
  framesPerSecond?: number;
  /** Encoder used for the outbound video track, e.g. `H264`, `VP8`. */
  encoder?: string;
  /** Outbound resolution as observed by the encoder. */
  resolution?: { width: number; height: number };
}

/**
 * Compose the FormData/headers for the WHIP POST. Pure for test coverage.
 */
export function whipPostInit(sdpOffer: string, bearerToken?: string): RequestInit {
  const headers: Record<string, string> = {
    'Content-Type': 'application/sdp',
  };
  if (bearerToken && bearerToken.trim()) {
    headers['Authorization'] = `Bearer ${bearerToken.trim()}`;
  }
  return {
    method: 'POST',
    headers,
    body: sdpOffer,
  };
}

/**
 * Parse a WHIP POST response's `Location` header into a fully-qualified
 * resource URL. WHIP servers are allowed to return a relative path
 * (`/whip/<broadcast>/<session-id>`); we resolve against the POST URL so the
 * subsequent DELETE goes to the right host.
 */
export function resolveSessionUrl(postUrl: string, location: string | null): string | null {
  if (!location) return null;
  try {
    return new URL(location, postUrl).toString();
  } catch {
    return null;
  }
}

/**
 * Build the getUserMedia / getDisplayMedia constraints. Pure for test
 * coverage; does not call into the browser API.
 */
export function buildConstraints(opts: PublishOptions): MediaStreamConstraints {
  const video = opts.video
    ? {
        width: opts.width ? { ideal: opts.width } : undefined,
        height: opts.height ? { ideal: opts.height } : undefined,
        frameRate: opts.frameRate ? { ideal: opts.frameRate } : undefined,
      }
    : false;
  const audio = opts.audio ? true : false;
  return { video, audio };
}

/**
 * Reactive WHIP publisher state. View calls `start()` to publish and
 * `stop()` to tear down. The peer connection is shared across the
 * composable's lifetime; multiple `start()` calls in a row implicitly
 * stop the previous session.
 */
export function useWhipPublisher() {
  const state = ref<PublishState>('idle');
  const lastError = ref<Error | null>(null);
  const sessionUrl = ref<string | null>(null);
  const stats = ref<PublishStatsSnapshot>({ bytesSent: 0, bitsPerSecond: 0 });

  let pc: RTCPeerConnection | null = null;
  let stream: MediaStream | null = null;
  let statsTimer: number | undefined;
  let priorBytes = 0;
  let priorTs = 0;

  const isPublishing = computed(() => state.value === 'connected' || state.value === 'gathering-ice' || state.value === 'posting-offer');

  /** The raw stream the view should bind to a `<video>` element for preview. */
  function previewStream(): MediaStream | null {
    return stream;
  }

  async function waitForIceGatheringComplete(peer: RTCPeerConnection, timeoutMs = 5000): Promise<void> {
    if (peer.iceGatheringState === 'complete') return;
    return new Promise<void>((resolve) => {
      const onChange = () => {
        if (peer.iceGatheringState === 'complete') {
          peer.removeEventListener('icegatheringstatechange', onChange);
          resolve();
        }
      };
      peer.addEventListener('icegatheringstatechange', onChange);
      // Some browsers fire icecandidate(null) without flipping the state;
      // also bound the wait so a flaky network does not hang the publisher.
      window.setTimeout(() => {
        peer.removeEventListener('icegatheringstatechange', onChange);
        resolve();
      }, timeoutMs);
    });
  }

  async function start(opts: PublishOptions): Promise<void> {
    await stop();
    state.value = 'requesting-media';
    lastError.value = null;

    try {
      stream = opts.source === 'screen'
        ? await navigator.mediaDevices.getDisplayMedia(buildConstraints({ ...opts, source: 'camera' }))
        : await navigator.mediaDevices.getUserMedia(buildConstraints(opts));

      pc = new RTCPeerConnection({
        iceServers: [{ urls: ['stun:stun.l.google.com:19302'] }],
      });

      // Add transceivers explicitly so we can pin codec preferences
      // BEFORE the offer is created. LVQR's WHIP bridge only knows
      // how to build init segments for H264 / HEVC video; Chrome's
      // WebRTC default of VP8 leaves the bridge unable to publish
      // anything to the FragmentBroadcasterRegistry, so the
      // broadcast never appears in /api/v1/streams or any other
      // admin view. Pinning H264 (or HEVC if the system has it)
      // up-front keeps the entire egress + admin surface coherent.
      const videoTrack = stream.getVideoTracks()[0];
      const audioTrack = stream.getAudioTracks()[0];
      if (videoTrack) {
        const tx = pc.addTransceiver(videoTrack, { direction: 'sendonly', streams: [stream] });
        try {
          const caps = (RTCRtpSender as unknown as {
            getCapabilities?: (kind: string) => RTCRtpCapabilities | null;
          }).getCapabilities?.('video');
          if (caps) {
            const preferred = caps.codecs.filter((c) => /\/h26[45]$/i.test(c.mimeType));
            if (preferred.length && 'setCodecPreferences' in tx) {
              (tx as unknown as {
                setCodecPreferences: (cs: RTCRtpCodec[]) => void;
              }).setCodecPreferences(preferred);
            }
          }
        } catch {
          // setCodecPreferences is best-effort; if it throws (older
          // Safari, niche WebRTC stacks) the negotiation falls back
          // to the browser's default codec list and the WHIP bridge
          // may not pick up the broadcast. Surface this in the UI
          // when we detect VP8 in the stats panel rather than
          // failing here.
        }
      }
      if (audioTrack) {
        pc.addTransceiver(audioTrack, { direction: 'sendonly', streams: [stream] });
      }

      // Stop the publisher when the peer connection drops.
      pc.addEventListener('connectionstatechange', () => {
        if (!pc) return;
        if (pc.connectionState === 'failed' || pc.connectionState === 'disconnected') {
          state.value = 'error';
          lastError.value = new Error(`peer connection ${pc.connectionState}`);
        } else if (pc.connectionState === 'connected') {
          state.value = 'connected';
        }
      });

      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);

      state.value = 'gathering-ice';
      await waitForIceGatheringComplete(pc);

      const sdpOffer = pc.localDescription?.sdp;
      if (!sdpOffer) {
        throw new Error('local description had no SDP after ICE gathering');
      }

      state.value = 'posting-offer';
      const resp = await fetch(opts.whipUrl, whipPostInit(sdpOffer, opts.bearerToken));
      if (!resp.ok) {
        throw new Error(`WHIP POST ${opts.whipUrl}: HTTP ${resp.status} ${resp.statusText}`);
      }

      const location = resp.headers.get('Location');
      sessionUrl.value = resolveSessionUrl(opts.whipUrl, location);

      const answer = await resp.text();
      await pc.setRemoteDescription({ type: 'answer', sdp: answer });

      // The connectionstatechange handler will flip state to `connected`
      // once ICE/DTLS finishes. In the meantime stay in `posting-offer`.
      startStatsLoop();
    } catch (e) {
      state.value = 'error';
      lastError.value = e instanceof Error ? e : new Error(String(e));
      await stop();
      throw lastError.value;
    }
  }

  function startStatsLoop() {
    stopStatsLoop();
    priorBytes = 0;
    priorTs = 0;
    statsTimer = window.setInterval(() => {
      if (!pc) return;
      void pc.getStats().then((report) => {
        let bytesSent = 0;
        let rttMs: number | undefined;
        let fps: number | undefined;
        let encoder: string | undefined;
        let resolution: { width: number; height: number } | undefined;
        report.forEach((entry) => {
          const r = entry as Record<string, unknown>;
          if (r.type === 'outbound-rtp' && r.kind === 'video') {
            if (typeof r.bytesSent === 'number') bytesSent += r.bytesSent;
            if (typeof r.framesPerSecond === 'number') fps = r.framesPerSecond;
            if (typeof r.frameWidth === 'number' && typeof r.frameHeight === 'number') {
              resolution = { width: r.frameWidth, height: r.frameHeight };
            }
          }
          if (r.type === 'outbound-rtp' && r.kind === 'audio') {
            if (typeof r.bytesSent === 'number') bytesSent += r.bytesSent;
          }
          if (r.type === 'codec' && typeof r.mimeType === 'string') {
            const slash = r.mimeType.lastIndexOf('/');
            if (slash >= 0) encoder = r.mimeType.slice(slash + 1);
          }
          if (r.type === 'candidate-pair' && r.state === 'succeeded') {
            if (typeof r.currentRoundTripTime === 'number') {
              rttMs = r.currentRoundTripTime * 1000;
            }
          }
        });
        const now = Date.now();
        const dt = priorTs ? (now - priorTs) / 1000 : 0;
        const dBytes = priorBytes ? bytesSent - priorBytes : 0;
        const bitsPerSecond = dt > 0 ? Math.max(0, (dBytes * 8) / dt) : 0;
        priorBytes = bytesSent;
        priorTs = now;
        stats.value = { bytesSent, bitsPerSecond, rttMs, framesPerSecond: fps, encoder, resolution };
      });
    }, 1000);
  }

  function stopStatsLoop() {
    if (statsTimer !== undefined) {
      window.clearInterval(statsTimer);
      statsTimer = undefined;
    }
  }

  async function stop(): Promise<void> {
    if (state.value === 'idle') return;
    state.value = 'stopping';
    stopStatsLoop();
    const url = sessionUrl.value;
    sessionUrl.value = null;
    try {
      if (url) {
        // Best-effort tear-down on the server side. Failures are swallowed
        // so a stale or unreachable resource URL does not block local
        // cleanup of the peer connection + media tracks.
        await fetch(url, { method: 'DELETE' }).catch(() => {});
      }
    } finally {
      try {
        pc?.close();
      } catch {
        // ignore
      }
      pc = null;
      stream?.getTracks().forEach((t) => t.stop());
      stream = null;
      stats.value = { bytesSent: 0, bitsPerSecond: 0 };
      // Preserve `error` if start() flipped us there before stop() ran;
      // otherwise normal completion lands at `idle`.
      const finalState: PublishState = (state.value as PublishState) === 'error' ? 'error' : 'idle';
      state.value = finalState;
    }
  }

  onBeforeUnmount(() => {
    void stop();
  });

  return {
    state,
    lastError,
    sessionUrl,
    stats,
    isPublishing,
    previewStream,
    start,
    stop,
  };
}
