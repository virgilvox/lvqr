import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { IngestState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Runtime ingest-listener inventory backing the Ingest view. `state.listeners`
 * is the live registry of bound listeners (one per ingest protocol) plus each
 * one's current `enabled` flag -- `false` after the operator stops it via
 * `DELETE /api/v1/ingest/{protocol}`. `error` captures transport failures so
 * the poll loop never crashes the view.
 *
 * STOP is one-way until the relay restarts (the entry stays in the inventory
 * with `enabled: false` so the UI can show "rtmp stopped" rather than letting
 * the listener silently vanish).
 */
export const useIngestStore = defineStore('ingest', () => {
  const state = ref<IngestState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.ingestListeners();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Stop the named listener and refresh. Throws on 404/503. */
  async function stopListener(protocol: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.stopIngestListener(protocol);
    await fetch();
  }

  return { state, error, lastFetchedAt, fetch, stopListener };
});
