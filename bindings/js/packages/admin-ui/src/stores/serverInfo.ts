import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { ServerInfo } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Server introspection (`GET /api/v1/server-info`): build version + features,
 * uptime, bound listener addresses, and runtime feature flags. Backs the
 * Ingest view's listener inventory; `error` captures transport failures so
 * the poll loop never crashes the view.
 */
export const useServerInfoStore = defineStore('serverInfo', () => {
  const info = ref<ServerInfo | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      info.value = await conn.client.serverInfo();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  return { info, error, lastFetchedAt, fetch };
});
