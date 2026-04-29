<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import PageHeader from '@/components/ui/PageHeader.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import SloEntryCard from '@/components/widgets/SloEntryCard.vue';
import { useStreamsStore } from '@/stores/streams';
import { useSloStore } from '@/stores/slo';
import { useMeshStore } from '@/stores/mesh';
import { usePolling } from '@/composables/usePolling';

const route = useRoute();
const streams = useStreamsStore();
const slo = useSloStore();
const mesh = useMeshStore();

usePolling(() => streams.fetch(), { intervalMs: 5_000 });
usePolling(() => slo.fetch(), { intervalMs: 30_000 });
usePolling(() => mesh.fetch(), { intervalMs: 10_000 });

const broadcastName = computed(() => decodeURIComponent(String(route.params.name ?? '')));
const stream = computed(() => streams.streams.find((s) => s.name === broadcastName.value) ?? null);
const sloRows = computed(() =>
  (slo.slo?.broadcasts ?? []).filter((e) => e.broadcast === broadcastName.value),
);
</script>

<template>
  <div class="page">
    <PageHeader :crumb="`STREAMS / ${broadcastName}`">
      <template #title>{{ broadcastName }}</template>
      <template #actions>
        <RouterLink to="/streams">
          <Button variant="ghost">Back</Button>
        </RouterLink>
      </template>
    </PageHeader>

    <section class="kpis">
      <KpiTile label="Subscribers" :value="stream?.subscribers ?? 0" />
      <KpiTile
        label="Mesh peers"
        :value="(mesh.mesh?.peers ?? []).length"
        :hint="(mesh.mesh?.offload_percentage ?? 0).toFixed(1) + '% offload'"
        accent="wire"
      />
      <KpiTile label="SLO rows" :value="sloRows.length" accent="none" />
    </section>

    <Card kicker="LATENCY" title="SLO breakdown">
      <div class="slo-list">
        <SloEntryCard v-for="e in sloRows" :key="e.transport" :entry="e" />
        <p v-if="!sloRows.length" class="empty">
          No SLO samples for this broadcast yet.
        </p>
      </div>
    </Card>

    <!-- LVQR v1.x backlog: per-broadcast stop / kick-subscriber controls.
         The current /api/v1/streams route is read-only; mutating shape would
         require a new admin endpoint. -->
    <Card class="placeholder" kicker="ACTIONS" title="Lifecycle">
      <p>
        Stopping a broadcast or kicking a subscriber is not yet exposed by the LVQR admin
        surface. Operators terminate broadcasts at the publisher (OBS / ffmpeg / WHIP client)
        or restart the relay. Tracking item: future <code>POST /api/v1/streams/:name/stop</code>.
      </p>
    </Card>
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
.slo-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
.placeholder p {
  font-size: 13px;
  color: var(--ink-muted);
}
.placeholder code {
  font-family: var(--font-mono);
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
}
</style>
