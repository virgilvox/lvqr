<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import WasmChainSlot from '@/components/widgets/WasmChainSlot.vue';
import { useWasmFilterStore } from '@/stores/wasmFilter';
import { usePolling } from '@/composables/usePolling';

const filter = useWasmFilterStore();
usePolling(() => filter.fetch(), { intervalMs: 10_000 });

const enabled = computed(() => !!filter.state?.enabled);
const slots = computed(() => filter.state?.slots ?? []);
const broadcasts = computed(() => filter.state?.broadcasts ?? []);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / WASM FILTERS">
      <template #title>Filter <em>chain.</em></template>
      <template #actions>
        <Badge :variant="enabled ? 'tally' : 'neutral'">
          {{ enabled ? `${filter.state?.chain_length ?? 0} slot${slots.length === 1 ? '' : 's'}` : 'disabled' }}
        </Badge>
      </template>
    </PageHeader>

    <EmptyState
      v-if="!enabled"
      kicker="DISABLED"
      title="No WASM filter chain configured."
    >
      Configure a chain at <code>lvqr serve</code> startup with one or more
      <code>--wasm-filter &lt;path&gt;.wasm</code> flags (or the comma-delimited
      <code>LVQR_WASM_FILTER</code> env). Filters are evaluated in declaration order; the chain
      short-circuits on the first slot that returns <code>None</code>.
    </EmptyState>

    <Card v-else kicker="CHAIN" title="Slots in evaluation order">
      <div class="slots">
        <WasmChainSlot
          v-for="s in slots"
          :key="s.index"
          :slot="s"
          :total="filter.state?.chain_length ?? slots.length"
        />
      </div>
      <p class="hint">
        Chain composition is configured at process startup and is read-only here. To change
        slots, restart with new <code>--wasm-filter</code> flags. A node-graph editor for
        runtime chain editing is on the v1.x backlog.
      </p>
    </Card>

    <Card v-if="enabled" kicker="TRAFFIC" title="Per-broadcast counters">
      <div class="bc-table">
        <header>
          <span>Broadcast</span><span>Track</span><span>seen</span><span>kept</span><span>dropped</span>
        </header>
        <article v-for="b in broadcasts" :key="b.broadcast + b.track">
          <span>{{ b.broadcast }}</span>
          <span>{{ b.track }}</span>
          <span>{{ b.seen }}</span>
          <span class="kept">{{ b.kept }}</span>
          <span class="dropped">{{ b.dropped }}</span>
        </article>
        <p v-if="!broadcasts.length" class="empty">No traffic observed yet.</p>
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
.slots {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
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
  font-weight: 700;
}
.kept {
  color: var(--ready);
}
.dropped {
  color: var(--on-air);
}
.empty {
  padding: var(--s-3);
  text-align: center;
  color: var(--ink-faint);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-top: var(--s-3);
}
.hint code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
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
  .bc-table header,
  .bc-table article {
    grid-template-columns: 1fr 1fr;
  }
  .bc-table header span:nth-child(n + 3),
  .bc-table article span:nth-child(n + 3) {
    display: none;
  }
}
</style>
