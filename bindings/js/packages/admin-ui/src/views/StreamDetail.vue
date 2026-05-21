<script setup lang="ts">
import { computed, watch } from 'vue';
import { useRoute } from 'vue-router';
import PageHeader from '@/components/ui/PageHeader.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Badge from '@/components/ui/Badge.vue';
import Icon from '@/components/ui/Icon.vue';
import SloEntryCard from '@/components/widgets/SloEntryCard.vue';
import PublishRecipes from '@/components/widgets/PublishRecipes.vue';
import SubscribeUrls from '@/components/widgets/SubscribeUrls.vue';
import { useSloStore } from '@/stores/slo';
import { useMeshStore } from '@/stores/mesh';
import { useStreamDetailStore } from '@/stores/streamDetail';
import { usePolling } from '@/composables/usePolling';

const route = useRoute();
const slo = useSloStore();
const mesh = useMeshStore();
const detail = useStreamDetailStore();

const broadcastName = computed(() => decodeURIComponent(String(route.params.name ?? '')));

usePolling(() => detail.fetch(broadcastName.value), { intervalMs: 5_000 });
usePolling(() => slo.fetch(), { intervalMs: 30_000 });
usePolling(() => mesh.fetch(), { intervalMs: 10_000 });

// Navigating /streams/a -> /streams/b reuses this component, so re-fetch
// on the param change rather than waiting for the next poll tick.
watch(broadcastName, (name) => void detail.fetch(name));

const tracks = computed(() => detail.detail?.tracks ?? []);
const subscribers = computed(() => detail.detail?.subscribers ?? 0);
const sloRows = computed(() =>
  (slo.slo?.broadcasts ?? []).filter((e) => e.broadcast === broadcastName.value),
);

const KIND_VARIANT: Record<string, 'tally' | 'wire' | 'ready' | 'warn' | 'neutral'> = {
  video: 'tally',
  audio: 'wire',
  captions: 'ready',
  scte35: 'warn',
  timing: 'neutral',
  catalog: 'neutral',
  data: 'neutral',
};
function kindVariant(kind: string) {
  return KIND_VARIANT[kind] ?? 'neutral';
}
</script>

<template>
  <div class="page">
    <PageHeader :crumb="`STREAMS / ${broadcastName}`">
      <template #title>{{ broadcastName }}</template>
      <template #actions>
        <RouterLink :to="{ path: '/stream-test', query: { broadcast: broadcastName } }">
          <Button variant="primary"><Icon name="rec" :size="12" /> Test this stream</Button>
        </RouterLink>
        <RouterLink to="/streams">
          <Button variant="ghost">Back</Button>
        </RouterLink>
      </template>
    </PageHeader>

    <section class="kpis">
      <KpiTile label="Subscribers" :value="subscribers" />
      <KpiTile label="Tracks" :value="tracks.length" accent="wire" />
      <KpiTile
        label="Mesh peers"
        :value="(mesh.mesh?.peers ?? []).length"
        :hint="(mesh.mesh?.offload_percentage ?? 0).toFixed(1) + '% offload'"
        accent="none"
      />
    </section>

    <Card kicker="MEDIA" title="Tracks">
      <template #actions>
        <span v-if="!detail.offline && !detail.error" class="badge-dot" :class="{ live: tracks.length }">
          {{ tracks.length ? 'LIVE' : 'IDLE' }}
        </span>
      </template>

      <!-- error: transport / auth failure -->
      <p v-if="detail.error" class="state state-error">{{ detail.error }}</p>
      <!-- first load -->
      <div v-else-if="detail.loading && !detail.detail" class="state state-loading">
        <span class="spinner" /> Loading track detail...
      </div>
      <!-- offline: relay has no track for this broadcast (404) -->
      <p v-else-if="detail.offline" class="state state-empty">
        Not publishing. No active track for <code>{{ broadcastName }}</code> on this relay.
        Start the broadcast (OBS / ffmpeg / WHIP) or
        <RouterLink :to="{ path: '/stream-test', query: { broadcast: broadcastName } }">test it</RouterLink>.
      </p>
      <!-- populated -->
      <div v-else class="track-table" role="table" aria-label="Tracks">
        <div class="tr th" role="row">
          <span role="columnheader">Track</span>
          <span role="columnheader">Kind</span>
          <span role="columnheader">Codec</span>
          <span role="columnheader" class="num">Fragments</span>
          <span role="columnheader" class="num">Subs</span>
          <span role="columnheader" class="num">Lag</span>
        </div>
        <div v-for="t in tracks" :key="t.track" class="tr" role="row">
          <span role="cell" class="mono">{{ t.track }}</span>
          <span role="cell"><Badge :variant="kindVariant(t.kind)">{{ t.kind }}</Badge></span>
          <span role="cell" class="mono codec">{{ t.codec || '--' }}</span>
          <span role="cell" class="num">{{ t.fragments.toLocaleString() }}</span>
          <span role="cell" class="num">{{ t.subscribers }}</span>
          <span role="cell" class="num" :class="{ warn: t.lagged_skips > 0 }">{{ t.lagged_skips }}</span>
        </div>
      </div>
    </Card>

    <Card kicker="LATENCY" title="SLO breakdown">
      <div class="slo-list">
        <SloEntryCard v-for="e in sloRows" :key="e.transport" :entry="e" />
        <p v-if="!sloRows.length" class="empty">No SLO samples for this broadcast yet.</p>
      </div>
    </Card>

    <div class="dual">
      <Card kicker="PUBLISH" title="Publisher URLs" wire>
        <PublishRecipes :broadcast="broadcastName" />
      </Card>
      <Card kicker="SUBSCRIBE" title="Subscriber URLs">
        <SubscribeUrls :broadcast="broadcastName" />
      </Card>
    </div>
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
.track-table {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.tr {
  display: grid;
  grid-template-columns: 1.4fr 0.9fr 1.6fr 1fr 0.6fr 0.6fr;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
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
.codec {
  color: var(--ink-muted);
  font-size: 12px;
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
.state-empty {
  color: var(--ink-faint);
}
.state code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
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
.badge-dot {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  color: var(--ink-faint);
}
.badge-dot.live {
  color: var(--on-air);
}
.slo-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.dual {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--s-4);
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .dual {
    grid-template-columns: 1fr;
  }
  .tr {
    grid-template-columns: 1.2fr 0.8fr 0.6fr 0.6fr;
  }
  .tr .codec,
  .tr.th span:nth-child(3) {
    display: none;
  }
}
</style>
