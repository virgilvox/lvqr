<script setup lang="ts">
import { computed } from 'vue';
import type { SloEntry } from '@lvqr/core';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import SloEntryCard from '@/components/widgets/SloEntryCard.vue';
import { useSloStore } from '@/stores/slo';
import { useStatsStore } from '@/stores/stats';
import { usePolling } from '@/composables/usePolling';

const slo = useSloStore();
const stats = useStatsStore();

usePolling(() => slo.fetch(), { intervalMs: 30_000 });
usePolling(() => stats.fetch(), { intervalMs: 5_000 });

const grouped = computed<Array<[string, SloEntry[]]>>(() => {
  const map = new Map<string, SloEntry[]>();
  for (const e of slo.slo?.broadcasts ?? []) {
    const list = map.get(e.transport) ?? [];
    list.push(e);
    map.set(e.transport, list);
  }
  return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b));
});
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / EGRESS">
      <template #title>Egress <em>endpoints.</em></template>
    </PageHeader>

    <section v-if="grouped.length" class="transport-grid">
      <Card v-for="[transport, entries] in grouped" :key="transport" :kicker="transport.toUpperCase()" :title="`${entries.length} broadcast${entries.length === 1 ? '' : 's'}`">
        <div class="rows">
          <SloEntryCard v-for="e in entries" :key="e.broadcast" :entry="e" />
        </div>
      </Card>
    </section>

    <Card v-else kicker="EMPTY" title="No SLO samples yet">
      <p>
        Egress traffic populates the per-(broadcast, transport) latency tracker
        automatically. <code>HLS</code> + <code>DASH</code> egress lift their wall-clock anchor
        from <code>#EXT-X-PROGRAM-DATE-TIME</code>; <code>MoQ</code> egress reads the sidecar
        <code>0.timing</code> track via <code>@lvqr/admin-ui</code>'s built-in subscriber, when
        the operator wires <code>POST /api/v1/slo/client-sample</code> to a sample-pushing
        client.
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
.transport-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
  gap: var(--s-4);
}
.rows {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
p {
  font-size: 13px;
  color: var(--ink-muted);
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
}
</style>
