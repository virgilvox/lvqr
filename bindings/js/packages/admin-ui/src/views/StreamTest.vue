<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useRoute } from 'vue-router';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Badge from '@/components/ui/Badge.vue';
import Tally from '@/components/ui/Tally.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import PublishRecipes from '@/components/widgets/PublishRecipes.vue';
import SubscribeUrls from '@/components/widgets/SubscribeUrls.vue';
import { useConnectionStore } from '@/stores/connection';
import { useStreamKeysStore } from '@/stores/streamkeys';
import { useToast } from '@/composables/useToast';
import { useWhipPublisher, type PublishOptions, type PublishStatsSnapshot } from '@/composables/useWhipPublisher';
import { broadcastUrls, profileScheme, profileHost, DEFAULT_PROTOCOL_PORTS } from '@/api/protocolUrls';
import { formatBytes } from '@/api/url';

const route = useRoute();
const conn = useConnectionStore();
const sk = useStreamKeysStore();
const { push } = useToast();
const publisher = useWhipPublisher();

const broadcast = ref<string>(typeof route.query.broadcast === 'string' ? route.query.broadcast : 'live/demo');
const token = ref<string>(typeof route.query.token === 'string' ? route.query.token : '');
const source = ref<'camera' | 'screen'>('camera');
const enableVideo = ref(true);
const enableAudio = ref(true);
const resolution = ref<'480p' | '720p' | '1080p'>('720p');
const frameRate = ref(30);
const previewVideo = ref<HTMLVideoElement | null>(null);
// Operator override of the auto-derived WHIP URL. Empty means "use the
// computed default below". Critical when the connection profile's
// whipPort isn't set and the relay binds WHIP on a non-default port; the
// override surfaces inline so the operator does not have to detour
// through the connection drawer.
const whipOverride = ref<string>('');

const resolutionToConstraints = computed<{ width?: number; height?: number }>(() => {
  switch (resolution.value) {
    case '1080p': return { width: 1920, height: 1080 };
    case '720p': return { width: 1280, height: 720 };
    case '480p': return { width: 854, height: 480 };
  }
  return {};
});

const defaultWhipUrl = computed(() => {
  if (!conn.activeProfile) return '';
  const host = profileHost(conn.activeProfile);
  const scheme = profileScheme(conn.activeProfile) === 'https:' ? 'https' : 'http';
  const port = (conn.activeProfile as { whipPort?: number }).whipPort ?? DEFAULT_PROTOCOL_PORTS.whip;
  return `${scheme}://${host}:${port}/whip/${broadcast.value}`;
});

// Track whether the user has manually edited the field. Until they do,
// we keep the override in sync with the derived default so changing the
// broadcast / profile reflects in the input. As soon as they type, they
// own the value -- typical "managed default" pattern.
const whipUserEdited = ref(false);
const whipUrl = computed(() => whipOverride.value.trim() || defaultWhipUrl.value);

onMounted(() => {
  if (!whipOverride.value && defaultWhipUrl.value) {
    whipOverride.value = defaultWhipUrl.value;
  }
});

watch(
  [broadcast, defaultWhipUrl],
  ([, next]) => {
    if (!whipUserEdited.value && next) {
      whipOverride.value = next;
    }
  },
);

function onWhipInput(e: Event) {
  whipUserEdited.value = true;
  whipOverride.value = (e.target as HTMLInputElement).value;
}

function resetWhipToDerived() {
  whipUserEdited.value = false;
  whipOverride.value = defaultWhipUrl.value;
}

// True when the override is empty AND the default URL points at the LVQR
// default WHIP port (8443) while the admin URL is on a non-default port.
// That combination almost always means the operator forgot to set
// whipPort on their connection profile, so we surface a one-liner hint
// rather than letting them eat a "Failed to fetch" later.
const portMismatchHint = computed(() => {
  if (!conn.activeProfile) return null;
  if (whipOverride.value.trim()) return null;
  const overrides = conn.activeProfile as { whipPort?: number };
  if (overrides.whipPort != null) return null;
  try {
    const adminUrl = new URL(conn.activeProfile.baseUrl);
    if (!adminUrl.port) return null;
    const adminPort = parseInt(adminUrl.port, 10);
    if (Number.isNaN(adminPort)) return null;
    // 8080 is the documented admin default; only flag when admin is on a
    // non-default port (suggesting the rest are non-default too).
    if (adminPort === 8080) return null;
    return `Admin is on :${adminPort} but WHIP defaults to :${DEFAULT_PROTOCOL_PORTS.whip}. If your relay's WHIP listener is elsewhere, set the WHIP port in the connection profile (Advanced) or paste the full URL in the override field.`;
  } catch {
    return null;
  }
});

async function mintKey() {
  try {
    const k = await sk.mint({ label: `streamtest-${broadcast.value}`, broadcast: broadcast.value });
    token.value = k.token;
    push('success', 'minted stream key for this broadcast');
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}

async function start() {
  if (!conn.activeProfile) {
    push('error', 'no active connection');
    return;
  }
  if (!enableVideo.value && !enableAudio.value) {
    push('error', 'enable at least one of video / audio');
    return;
  }
  const opts: PublishOptions = {
    whipUrl: whipUrl.value,
    bearerToken: token.value || undefined,
    video: enableVideo.value,
    audio: enableAudio.value,
    width: resolutionToConstraints.value.width,
    height: resolutionToConstraints.value.height,
    frameRate: frameRate.value,
    source: source.value,
  };
  try {
    await publisher.start(opts);
    // Bind the local capture to the preview video element after the
    // publisher has the stream ready.
    if (previewVideo.value) {
      previewVideo.value.srcObject = publisher.previewStream();
      void previewVideo.value.play().catch(() => {});
    }
    push('success', `publishing to ${broadcast.value}`);
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 8000);
  }
}

async function stop() {
  await publisher.stop();
  if (previewVideo.value) {
    previewVideo.value.srcObject = null;
  }
  push('info', 'stopped');
}

onBeforeUnmount(() => {
  void publisher.stop();
});

// Bind preview when the underlying stream changes (e.g. start() succeeded
// before the video element was mounted).
watch(
  () => publisher.state.value,
  () => {
    const stream = publisher.previewStream();
    if (previewVideo.value && stream) {
      previewVideo.value.srcObject = stream;
      void previewVideo.value.play().catch(() => {});
    }
  },
);

const stateBadge = computed(() => {
  switch (publisher.state.value) {
    case 'idle': return { variant: 'neutral' as const, label: 'IDLE' };
    case 'requesting-media': return { variant: 'warn' as const, label: 'PERMISSION' };
    case 'gathering-ice': return { variant: 'warn' as const, label: 'ICE' };
    case 'posting-offer': return { variant: 'wire' as const, label: 'OFFER' };
    case 'connected': return { variant: 'on-air' as const, label: 'ON AIR' };
    case 'stopping': return { variant: 'warn' as const, label: 'STOPPING' };
    case 'error': return { variant: 'on-air' as const, label: 'ERROR' };
  }
  return { variant: 'neutral' as const, label: 'IDLE' };
});

function formatStats(s: PublishStatsSnapshot): { bytes: string; rate: string; rtt: string; fps: string; res: string; codec: string } {
  return {
    bytes: formatBytes(s.bytesSent),
    rate: s.bitsPerSecond ? `${(s.bitsPerSecond / 1000).toFixed(0)} kbps` : '-',
    rtt: s.rttMs != null ? `${s.rttMs.toFixed(0)} ms` : '-',
    fps: s.framesPerSecond != null ? `${s.framesPerSecond.toFixed(0)} fps` : '-',
    res: s.resolution ? `${s.resolution.width}x${s.resolution.height}` : '-',
    codec: s.encoder ?? '-',
  };
}

const browserUrlHint = computed(() => urls.value?.subscribe.hls ?? '');
const urls = computed(() =>
  conn.activeProfile ? broadcastUrls(conn.activeProfile, broadcast.value, token.value || undefined) : null,
);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / STREAM TEST">
      <template #title>Test <em>stream.</em></template>
      <template #actions>
        <Tally
          :status="stateBadge.variant === 'on-air' ? 'on-air' : stateBadge.variant === 'warn' ? 'warn' : stateBadge.variant === 'wire' ? 'ready' : 'idle'"
          :label="stateBadge.label"
        />
      </template>
    </PageHeader>

    <EmptyState
      v-if="!conn.activeProfile"
      kicker="WAITING"
      title="No relay selected"
    >
      Add a connection profile in the topbar to enable the WHIP publisher.
    </EmptyState>

    <template v-else>
      <div class="layout">
        <div>
          <Card kicker="PREVIEW" :title="broadcast">
            <div class="preview-wrap">
              <video ref="previewVideo" muted playsinline autoplay class="preview-video" />
              <div class="preview-overlay" :class="`is-${publisher.state.value}`">
                <span v-if="publisher.state.value === 'idle'">click <strong>Start</strong> to publish from this browser</span>
                <span v-else-if="publisher.state.value === 'requesting-media'">requesting camera/mic permission</span>
                <span v-else-if="publisher.state.value === 'gathering-ice'">gathering ICE candidates</span>
                <span v-else-if="publisher.state.value === 'posting-offer'">posting WHIP offer</span>
                <span v-else-if="publisher.state.value === 'error'">error: {{ publisher.lastError.value?.message }}</span>
              </div>
              <div v-if="publisher.state.value === 'connected'" class="preview-on-air">
                <span class="dot" /> ON AIR
              </div>
            </div>
            <div class="stats" v-if="publisher.state.value === 'connected'">
              <div><span>bitrate</span><strong>{{ formatStats(publisher.stats.value).rate }}</strong></div>
              <div><span>fps</span><strong>{{ formatStats(publisher.stats.value).fps }}</strong></div>
              <div><span>rtt</span><strong>{{ formatStats(publisher.stats.value).rtt }}</strong></div>
              <div><span>res</span><strong>{{ formatStats(publisher.stats.value).res }}</strong></div>
              <div><span>codec</span><strong>{{ formatStats(publisher.stats.value).codec }}</strong></div>
              <div><span>sent</span><strong>{{ formatStats(publisher.stats.value).bytes }}</strong></div>
            </div>
          </Card>

          <Card kicker="SUBSCRIBE" title="Watch this stream" wire v-if="publisher.state.value === 'connected'">
            <p class="hint">
              Open any of these in another tab / device while you're publishing.
              The HLS variant works in any browser; MoQ + WHEP need the LVQR
              relay's web client surface.
            </p>
            <SubscribeUrls :broadcast="broadcast" :bearer-token="token" />
            <p class="hint" style="margin-top: var(--s-3)">
              Quick test: <a :href="browserUrlHint" target="_blank" rel="noopener">open the LL-HLS playlist</a>
              in a new tab and play it with VLC or hls.js.
            </p>
          </Card>
        </div>

        <div>
          <Card kicker="CAPTURE" title="Source + constraints">
            <div class="form">
              <label>
                <span>Broadcast</span>
                <input v-model="broadcast" :disabled="publisher.isPublishing.value" placeholder="live/demo" />
              </label>
              <label>
                <span>Bearer token (optional)</span>
                <div class="inline">
                  <input v-model="token" :disabled="publisher.isPublishing.value" placeholder="lvqr_sk_..." type="password" />
                  <Button variant="ghost" small :disabled="publisher.isPublishing.value" @click="mintKey">
                    Mint
                  </Button>
                </div>
              </label>
              <label>
                <span>Source</span>
                <div class="radio-row">
                  <label><input type="radio" value="camera" v-model="source" :disabled="publisher.isPublishing.value" /> camera + mic</label>
                  <label><input type="radio" value="screen" v-model="source" :disabled="publisher.isPublishing.value" /> screen share</label>
                </div>
              </label>
              <label>
                <span>Tracks</span>
                <div class="check-row">
                  <label><input type="checkbox" v-model="enableVideo" :disabled="publisher.isPublishing.value" /> video</label>
                  <label><input type="checkbox" v-model="enableAudio" :disabled="publisher.isPublishing.value" /> audio</label>
                </div>
              </label>
              <label>
                <span>Resolution</span>
                <select v-model="resolution" :disabled="publisher.isPublishing.value">
                  <option value="480p">480p</option>
                  <option value="720p">720p</option>
                  <option value="1080p">1080p</option>
                </select>
              </label>
              <label>
                <span>Frame rate</span>
                <select v-model.number="frameRate" :disabled="publisher.isPublishing.value">
                  <option :value="15">15</option>
                  <option :value="24">24</option>
                  <option :value="30">30</option>
                  <option :value="60">60</option>
                </select>
              </label>
            </div>

            <div class="ctrl">
              <Button v-if="!publisher.isPublishing.value" variant="primary" @click="start">Start publishing</Button>
              <Button v-else variant="danger" @click="stop">Stop</Button>
            </div>

            <div class="endpoint">
              <label>
                <span>WHIP endpoint</span>
                <div class="endpoint-row">
                  <input
                    :value="whipOverride"
                    @input="onWhipInput"
                    :placeholder="defaultWhipUrl"
                    :disabled="publisher.isPublishing.value"
                    spellcheck="false"
                    autocomplete="off"
                  />
                  <button
                    type="button"
                    class="endpoint-reset"
                    :disabled="publisher.isPublishing.value || whipOverride === defaultWhipUrl"
                    @click="resetWhipToDerived"
                    title="Reset to URL derived from the connection profile"
                  >
                    reset
                  </button>
                </div>
              </label>
              <p class="endpoint-effective" v-if="whipUserEdited && whipOverride !== defaultWhipUrl">
                edited; click <strong>reset</strong> to restore <code>{{ defaultWhipUrl }}</code>
              </p>
              <p class="endpoint-effective" v-else>
                derived from connection profile; edit inline to override
              </p>
              <p v-if="portMismatchHint" class="warn-hint">
                {{ portMismatchHint }}
              </p>
            </div>
            <p v-if="publisher.lastError.value" class="err">
              {{ publisher.lastError.value.message }}
              <span v-if="/Failed to fetch/i.test(publisher.lastError.value.message)" class="err-hint">
                <br />The browser could not reach the WHIP endpoint. Common
                fixes: (1) confirm the WHIP port matches your relay
                (<code>{{ whipUrl }}</code>); (2) the relay's WHIP listener
                may not be enabled (boot with <code>--whip-port</code>);
                (3) cross-origin block (LVQR v1.0.0 had no CORS layer on
                WHIP -- rebuild from <code>main</code> for the fix).
              </span>
            </p>
          </Card>

          <Card kicker="PUBLISH" title="External clients" v-if="urls">
            <p class="hint">
              Same broadcast, alternate publishers (OBS, ffmpeg, broadcast
              encoders). Bearer tokens substitute where the protocol expects
              them; WHIP/WHEP carry the token in the
              <code>Authorization</code> header.
            </p>
            <PublishRecipes :broadcast="broadcast" :bearer-token="token" />
            <Badge variant="tally" v-if="token">
              <Icon name="check" :size="10" />&nbsp;TOKEN BAKED IN
            </Badge>
          </Card>
        </div>
      </div>
    </template>
  </div>
</template>

<script lang="ts">
import Icon from '@/components/ui/Icon.vue';
export default { components: { Icon } };
</script>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.layout {
  display: grid;
  grid-template-columns: 1.4fr 1fr;
  gap: var(--s-4);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .layout {
    grid-template-columns: 1fr;
  }
}
.preview-wrap {
  position: relative;
  background: var(--ink);
  aspect-ratio: 16 / 9;
  width: 100%;
  overflow: hidden;
}
.preview-video {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
}
.preview-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(135deg, rgba(232, 117, 26, 0.18), rgba(14, 116, 144, 0.18));
  color: var(--paper);
  font-family: var(--font-mono);
  font-size: 12px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  text-align: center;
  padding: var(--s-5);
  pointer-events: none;
  transition: opacity 0.2s;
}
.preview-overlay.is-connected {
  opacity: 0;
}
.preview-overlay strong {
  color: var(--tally-bright);
}
.preview-on-air {
  position: absolute;
  top: 12px;
  left: 12px;
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--on-air);
  color: var(--paper);
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  font-weight: 700;
  padding: 4px 10px;
}
.preview-on-air .dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--paper);
  animation: pulse 1.2s ease-in-out infinite;
}
.stats {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
  gap: var(--s-3);
  padding: var(--s-3) var(--s-4) 0;
  font-family: var(--font-mono);
  font-size: 11px;
}
.stats > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.stats span {
  color: var(--ink-faint);
  text-transform: uppercase;
  letter-spacing: 0.15em;
  font-size: 10px;
}
.stats strong {
  color: var(--ink);
  font-size: 14px;
}
.form {
  display: grid;
  gap: var(--s-3);
}
.form label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.form span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.form input,
.form select {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 7px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.form input:disabled,
.form select:disabled {
  opacity: 0.6;
}
.inline {
  display: flex;
  gap: 6px;
  align-items: stretch;
}
.inline input {
  flex: 1;
}
.radio-row,
.check-row {
  display: flex;
  gap: var(--s-4);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-light);
}
.radio-row label,
.check-row label {
  flex-direction: row;
  align-items: center;
  gap: 6px;
}
.radio-row input,
.check-row input {
  border: none;
  padding: 0;
  background: none;
  width: auto;
}
.ctrl {
  display: flex;
  gap: var(--s-2);
  margin-top: var(--s-4);
}
.endpoint {
  margin-top: var(--s-3);
  padding-top: var(--s-3);
  border-top: 1px solid var(--chalk);
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.endpoint label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.endpoint label > span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.endpoint input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 5px 8px;
  font-size: 11px;
  font-family: var(--font-mono);
  flex: 1;
  min-width: 0;
}
.endpoint-row {
  display: flex;
  gap: 6px;
  align-items: stretch;
}
.endpoint-reset {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  padding: 4px 10px;
  border: 1px solid var(--chalk-hi);
  background: var(--paper);
  color: var(--ink-muted);
  cursor: pointer;
}
.endpoint-reset:hover:not(:disabled) {
  background: var(--ink);
  color: var(--paper);
  border-color: var(--ink);
}
.endpoint-reset:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.endpoint-effective {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--ink-muted);
}
.endpoint-effective code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
.warn-hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--warn);
  background: rgba(217, 119, 6, 0.06);
  border: 1px solid var(--warn);
  padding: var(--s-2);
  line-height: 1.55;
}
.err-hint {
  display: block;
  margin-top: 4px;
  color: var(--ink-muted);
  font-size: 10px;
  font-weight: 400;
}
.err-hint code {
  background: rgba(220, 38, 38, 0.06);
  padding: 1px 5px;
  border: 1px solid var(--on-air);
  color: var(--ink);
}
.err {
  margin-top: var(--s-3);
  padding: var(--s-2);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--on-air);
  background: rgba(220, 38, 38, 0.06);
  border: 1px solid var(--on-air);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  line-height: 1.65;
}
.hint code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
.hint a {
  color: var(--wire-deep);
  text-decoration: underline;
  text-underline-offset: 2px;
}
</style>
