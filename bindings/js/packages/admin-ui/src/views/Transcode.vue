<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import { useTranscodeStore } from '@/stores/transcode';
import { usePolling } from '@/composables/usePolling';

// Read-only introspection of the startup-configured transcode ladder via
// GET /api/v1/transcode/ladders. Runtime ladder editing (add / remove
// renditions, swap encoder) is a separate CRUD surface tracked for a later
// slice; this view lists what is configured plus live per-output counters.
const transcode = useTranscodeStore();
usePolling(() => transcode.fetch(), { intervalMs: 10_000 });

const state = computed(() => transcode.state);
const enabled = computed(() => state.value?.enabled ?? false);
const renditions = computed(() => state.value?.renditions ?? []);
const active = computed(() => state.value?.active ?? []);
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
          </div>
          <div v-for="r in renditions" :key="r.name" class="tr" role="row">
            <span role="cell"><Badge variant="tally">{{ r.name }}</Badge></span>
            <span role="cell" class="mono">{{ r.width }}x{{ r.height }}</span>
            <span role="cell" class="num">{{ r.video_bitrate_kbps.toLocaleString() }}</span>
            <span role="cell" class="num">{{ r.audio_bitrate_kbps.toLocaleString() }}</span>
          </div>
        </div>
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
  grid-template-columns: 1fr 1.2fr 1fr 1fr;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.tr.atr {
  grid-template-columns: 1.6fr 1fr 0.8fr 1fr 0.6fr;
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
