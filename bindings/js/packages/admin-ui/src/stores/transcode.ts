import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { RenditionInfo, TranscodeState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Transcode-ladder introspection backing the Transcode view. `state` holds
 * the relay's configured ladder + live counters; `error` captures transport
 * failures so the view can surface them without crashing the poll loop.
 */
export const useTranscodeStore = defineStore('transcode', () => {
  const state = ref<TranscodeState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.transcodeLadders();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Add a rendition at runtime, then refresh. Throws on failure (e.g. 409). */
  async function addRendition(spec: RenditionInfo): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.addRendition(spec);
    await fetch();
  }

  /** Remove a rendition at runtime, then refresh. Throws on failure (e.g. 404). */
  async function removeRendition(name: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.removeRendition(name);
    await fetch();
  }

  return { state, error, lastFetchedAt, fetch, addRendition, removeRendition };
});
