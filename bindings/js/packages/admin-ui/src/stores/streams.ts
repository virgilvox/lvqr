import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { StreamInfo } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useStreamsStore = defineStore('streams', () => {
  const streams = ref<StreamInfo[]>([]);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    streams.value = await conn.client.listStreams();
    lastFetchedAt.value = Date.now();
  }

  return { streams, lastFetchedAt, fetch };
});
