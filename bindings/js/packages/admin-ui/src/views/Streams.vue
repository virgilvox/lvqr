<script setup lang="ts">
import { computed, ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import StreamRow from '@/components/widgets/StreamRow.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import { useStreamsStore } from '@/stores/streams';
import { usePolling } from '@/composables/usePolling';

const streams = useStreamsStore();
usePolling(() => streams.fetch(), { intervalMs: 5_000 });

const query = ref('');
const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return streams.streams;
  return streams.streams.filter((s) => s.name.toLowerCase().includes(q));
});
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS / LIVE STREAMS">
      <template #title>Streams <em>on the wire.</em></template>
      <template #actions>
        <div class="search">
          <Icon name="search" :size="12" />
          <input v-model="query" placeholder="filter by name..." />
        </div>
        <Button variant="ghost" @click="streams.fetch()"><Icon name="reload" :size="12" /> Reload</Button>
      </template>
    </PageHeader>

    <div class="rows">
      <StreamRow v-for="s in filtered" :key="s.name" :stream="s" />
      <p v-if="!filtered.length" class="empty">
        {{ streams.streams.length ? 'No streams match the filter.' : 'No active streams.' }}
      </p>
    </div>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
}
.search {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: 4px 10px;
  font-family: var(--font-mono);
  font-size: 12px;
}
.search input {
  border: none;
  outline: none;
  background: transparent;
  width: 240px;
}
.rows {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.empty {
  padding: var(--s-5);
  text-align: center;
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .search input {
    width: 160px;
  }
}
</style>
