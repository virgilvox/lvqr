import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { ArchiveState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Archive-recording introspection backing the Recordings view. `state` holds
 * the relay's recorded broadcasts; `error` captures transport failures so the
 * poll loop never crashes the view.
 */
export const useArchiveStore = defineStore('archive', () => {
  const state = ref<ArchiveState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.archive();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  return { state, error, lastFetchedAt, fetch };
});
