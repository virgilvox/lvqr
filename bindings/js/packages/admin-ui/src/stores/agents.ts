import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { AgentState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * In-process agent introspection backing the Agents view. `state` holds the
 * configured agent set + live counters; `error` captures transport failures
 * so the poll loop never crashes the view.
 */
export const useAgentsStore = defineStore('agents', () => {
  const state = ref<AgentState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.agents();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  return { state, error, lastFetchedAt, fetch };
});
