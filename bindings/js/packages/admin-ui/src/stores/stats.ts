import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { RelayStats } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useStatsStore = defineStore('stats', () => {
  const stats = ref<RelayStats | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    stats.value = await conn.client.stats();
    lastFetchedAt.value = Date.now();
  }

  return { stats, lastFetchedAt, fetch };
});
