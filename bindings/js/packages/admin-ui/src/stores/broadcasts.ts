import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { BroadcastSessionsState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Live publisher-session inventory backing the broadcasts list. Different
 * from the `streams` store: this is the **publisher** view (one row per
 * connected ingest session) while `streams` is the **registry** view
 * (broadcasts that have data, whether or not their publisher is still
 * connected). Slice 6.
 */
export const useBroadcastsStore = defineStore('broadcasts', () => {
  const state = ref<BroadcastSessionsState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.broadcasts();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Kill the named broadcast's live publisher session and refresh. Throws on 404/503. */
  async function stopBroadcast(name: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.stopBroadcast(name);
    await fetch();
  }

  return { state, error, lastFetchedAt, fetch, stopBroadcast };
});
