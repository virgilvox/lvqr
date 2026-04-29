import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { ConfigReloadStatus } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useConfigReloadStore = defineStore('configReload', () => {
  const status = ref<ConfigReloadStatus | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    status.value = await conn.client.configReload();
    lastFetchedAt.value = Date.now();
  }

  async function trigger(): Promise<ConfigReloadStatus> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    const next = await conn.client.triggerConfigReload();
    status.value = next;
    lastFetchedAt.value = Date.now();
    return next;
  }

  return { status, lastFetchedAt, fetch, trigger };
});
