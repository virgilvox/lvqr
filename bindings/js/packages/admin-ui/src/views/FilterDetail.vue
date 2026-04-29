<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Button from '@/components/ui/Button.vue';
import WasmChainSlot from '@/components/widgets/WasmChainSlot.vue';
import { useWasmFilterStore } from '@/stores/wasmFilter';
import { usePolling } from '@/composables/usePolling';

const route = useRoute();
const filter = useWasmFilterStore();
usePolling(() => filter.fetch(), { intervalMs: 10_000 });

const slotIndex = computed(() => Number(route.params.index ?? 0));
const slot = computed(() => filter.state?.slots?.find((s) => s.index === slotIndex.value));
const broadcasts = computed(() => filter.state?.broadcasts ?? []);
</script>

<template>
  <div class="page">
    <PageHeader :crumb="`FILTERS / SLOT ${slotIndex + 1}`">
      <template #title>Slot <em>{{ slotIndex + 1 }}.</em></template>
      <template #actions>
        <RouterLink to="/filters">
          <Button variant="ghost">Back to chain</Button>
        </RouterLink>
      </template>
    </PageHeader>

    <Card v-if="slot" kicker="COUNTERS" title="Slot stats">
      <WasmChainSlot :slot="slot" :total="filter.state?.chain_length ?? 1" />
    </Card>

    <EmptyState
      v-else
      kicker="UNKNOWN"
      title="Slot not found."
    >
      The configured chain has no slot at index {{ slotIndex }}.
    </EmptyState>

    <Card kicker="BROADCASTS" title="Per-broadcast traffic">
      <p class="hint">
        Per-slot broadcast attribution is not split per slot in the current
        <code>/api/v1/wasm-filter</code> shape; the table below shows aggregate per-(broadcast,
        track) counters across the full chain.
      </p>
      <div class="bc-table">
        <header><span>Broadcast</span><span>Track</span><span>seen</span><span>kept</span><span>dropped</span></header>
        <article v-for="b in broadcasts" :key="b.broadcast + b.track">
          <span>{{ b.broadcast }}</span><span>{{ b.track }}</span>
          <span>{{ b.seen }}</span>
          <span class="kept">{{ b.kept }}</span>
          <span class="dropped">{{ b.dropped }}</span>
        </article>
      </div>
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
.bc-table {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  font-family: var(--font-mono);
  font-size: 12px;
}
.bc-table header,
.bc-table article {
  display: grid;
  grid-template-columns: 1.5fr 1fr 80px 80px 80px;
  gap: var(--s-3);
  padding: var(--s-2) var(--s-3);
  border-bottom: 1px solid var(--chalk);
}
.bc-table header {
  background: var(--chalk-lo);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.kept { color: var(--ready); }
.dropped { color: var(--on-air); }
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-bottom: var(--s-3);
}
.hint code {
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
