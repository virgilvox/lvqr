<script setup lang="ts">
import { computed, reactive, ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import Button from '@/components/ui/Button.vue';
import { useTranscodeStore } from '@/stores/transcode';
import { usePolling } from '@/composables/usePolling';
import { useToast } from '@/composables/useToast';

// Introspection + runtime CRUD of the transcode ladder via
// /api/v1/transcode/ladders. The encoder backend stays a startup choice;
// renditions can be added / removed at runtime (new broadcasts and already-
// live sources both pick up an added rendition).
const transcode = useTranscodeStore();
const { push } = useToast();
usePolling(() => transcode.fetch(), { intervalMs: 10_000 });

const state = computed(() => transcode.state);
const enabled = computed(() => state.value?.enabled ?? false);
const renditions = computed(() => state.value?.renditions ?? []);
const active = computed(() => state.value?.active ?? []);

const form = reactive({ name: '', width: 1280, height: 720, video_bitrate_kbps: 2500, audio_bitrate_kbps: 128 });
const submitting = ref(false);
const removing = ref<string | null>(null);

async function submitAdd() {
  const name = form.name.trim();
  if (!name || name.includes('/')) {
    push('error', "rendition name is required and must not contain '/'");
    return;
  }
  submitting.value = true;
  try {
    await transcode.addRendition({
      name,
      width: form.width,
      height: form.height,
      video_bitrate_kbps: form.video_bitrate_kbps,
      audio_bitrate_kbps: form.audio_bitrate_kbps,
    });
    push('success', `added rendition ${name}`);
    form.name = '';
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    push('error', msg.includes('409') ? `rendition ${name} already exists` : msg, 6000);
  } finally {
    submitting.value = false;
  }
}

async function removeRendition(name: string) {
  removing.value = name;
  try {
    await transcode.removeRendition(name);
    push('success', `removed rendition ${name}`);
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  } finally {
    removing.value = null;
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / TRANSCODE">
      <template #title>Transcode <em>ladders.</em></template>
      <template #actions>
        <Badge v-if="enabled" variant="tally">{{ state?.encoder }}</Badge>
      </template>
    </PageHeader>

    <p v-if="transcode.error" class="state state-error">{{ transcode.error }}</p>

    <div v-else-if="!state" class="state state-loading"><span class="spinner" /> Loading ladder...</div>

    <template v-else-if="enabled">
      <section class="kpis">
        <KpiTile label="Encoder" :value="state.encoder" />
        <KpiTile label="Renditions" :value="renditions.length" accent="wire" />
        <KpiTile label="Active outputs" :value="active.length" accent="none" />
      </section>

      <Card kicker="LADDER" title="Configured renditions">
        <div class="rtable" role="table" aria-label="Configured renditions">
          <div class="tr th" role="row">
            <span role="columnheader">Name</span>
            <span role="columnheader">Resolution</span>
            <span role="columnheader" class="num">Video kbps</span>
            <span role="columnheader" class="num">Audio kbps</span>
            <span role="columnheader" class="act"></span>
          </div>
          <div v-for="r in renditions" :key="r.name" class="tr" role="row">
            <span role="cell"><Badge variant="tally">{{ r.name }}</Badge></span>
            <span role="cell" class="mono">{{ r.width }}x{{ r.height }}</span>
            <span role="cell" class="num">{{ r.video_bitrate_kbps.toLocaleString() }}</span>
            <span role="cell" class="num">{{ r.audio_bitrate_kbps.toLocaleString() }}</span>
            <span role="cell" class="act">
              <Button
                variant="ghost"
                :loading="removing === r.name"
                :aria-label="`Remove ${r.name}`"
                @click="removeRendition(r.name)"
              >
                Remove
              </Button>
            </span>
          </div>
        </div>

        <form class="addform" @submit.prevent="submitAdd">
          <div class="kicker">ADD RENDITION</div>
          <div class="fields">
            <label><span>Name</span><input v-model="form.name" placeholder="360p" required /></label>
            <label><span>Width</span><input v-model.number="form.width" type="number" min="2" required /></label>
            <label><span>Height</span><input v-model.number="form.height" type="number" min="2" required /></label>
            <label><span>Video kbps</span><input v-model.number="form.video_bitrate_kbps" type="number" min="1" required /></label>
            <label><span>Audio kbps</span><input v-model.number="form.audio_bitrate_kbps" type="number" min="1" required /></label>
            <Button variant="primary" type="submit" :loading="submitting">Add</Button>
          </div>
          <p class="note">
            New renditions use the relay's startup encoder backend
            (<code>{{ state.encoder }}</code>) and apply to new and already-live broadcasts.
          </p>
        </form>
      </Card>

      <Card kicker="LIVE" title="Active outputs" wire>
        <div v-if="active.length" class="rtable" role="table" aria-label="Active transcode outputs">
          <div class="tr th atr" role="row">
            <span role="columnheader">Source</span>
            <span role="columnheader">Rendition</span>
            <span role="columnheader">Track</span>
            <span role="columnheader" class="num">Fragments</span>
            <span role="columnheader" class="num">Panics</span>
          </div>
          <div v-for="a in active" :key="`${a.broadcast}|${a.rendition}|${a.track}`" class="tr atr" role="row">
            <span role="cell" class="mono">{{ a.broadcast }}</span>
            <span role="cell"><Badge variant="wire">{{ a.rendition }}</Badge></span>
            <span role="cell" class="mono">{{ a.track }}</span>
            <span role="cell" class="num">{{ a.fragments_seen.toLocaleString() }}</span>
            <span role="cell" class="num" :class="{ warn: a.panics > 0 }">{{ a.panics }}</span>
          </div>
        </div>
        <p v-else class="empty">No broadcasts are currently being transcoded. Publish a source stream to see outputs here.</p>
      </Card>
    </template>

    <template v-else>
      <EmptyState kicker="NOT CONFIGURED" title="No transcode ladder on this relay.">
        This relay was started without a transcode ladder (or without the <code>transcode</code> build
        feature). Configure one at startup with <code>--transcode-rendition &lt;name&gt;</code>
        (e.g. <code>--transcode-rendition 720p --transcode-rendition 480p</code>) and select the encoder
        backend with <code>--transcode-encoder software|videotoolbox|nvenc|vaapi|qsv</code>.
      </EmptyState>

      <Card kicker="REFERENCE" title="Encoder backends">
        <ul class="ref">
          <li><strong>software</strong> -- x264; portable; default.</li>
          <li><strong>videotoolbox</strong> -- macOS Apple Silicon HW; build with <code>--features hw-videotoolbox</code>.</li>
          <li><strong>nvenc</strong> -- Linux + Nvidia GPU; build with <code>--features hw-nvenc</code>.</li>
          <li><strong>vaapi</strong> -- Linux + Intel iGPU + AMD; build with <code>--features hw-vaapi</code>.</li>
          <li><strong>qsv</strong> -- Linux + Intel Quick Sync; build with <code>--features hw-qsv</code>.</li>
        </ul>
      </Card>
    </template>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
}
.rtable {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.tr {
  display: grid;
  grid-template-columns: 1fr 1.2fr 1fr 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.tr.atr {
  grid-template-columns: 1.6fr 1fr 0.8fr 1fr 0.6fr;
}
.act {
  text-align: right;
}
.addform {
  margin-top: var(--s-4);
  padding-top: var(--s-4);
  border-top: 1px solid var(--chalk-hi);
}
.addform .kicker {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  color: var(--ink-faint);
  margin-bottom: var(--s-2);
}
.fields {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: var(--s-3);
}
.fields label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.fields label span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.fields input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 6px 9px;
  font-family: var(--font-mono);
  font-size: 13px;
  width: 96px;
}
.fields input[type='text'],
.fields label:first-child input {
  width: 120px;
}
.note {
  margin-top: var(--s-2);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.mono {
  font-family: var(--font-mono);
}
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
  font-family: var(--font-mono);
}
.num.warn {
  color: var(--warn);
}
.state {
  font-family: var(--font-mono);
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: var(--s-3) 0;
}
.state-error {
  color: var(--on-air);
}
.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--chalk-hi);
  border-top-color: var(--tally-deep);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
.ref {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-muted);
}
.ref li strong {
  color: var(--tally-deep);
  margin-right: 8px;
}
code {
  font-family: var(--font-mono);
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .tr.atr {
    grid-template-columns: 1.4fr 1fr 0.8fr 0.6fr;
  }
  .tr.atr span:nth-child(4) {
    display: none;
  }
}
</style>
