import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { SloSnapshot } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useSloStore = defineStore('slo', () => {
  const slo = ref<SloSnapshot | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    slo.value = await conn.client.slo();
    lastFetchedAt.value = Date.now();
  }

  return { slo, lastFetchedAt, fetch };
});
