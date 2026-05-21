import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { StreamDetailInfo } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Per-broadcast detail backing the StreamDetail view. Distinguishes three
 * states the view renders differently: `detail` populated (live), `offline`
 * true (relay returned 404 -- broadcast not currently publishing), and
 * `error` set (transport / auth failure). `loading` is only true on the
 * first fetch for a name so polling refreshes don't flash the skeleton.
 */
export const useStreamDetailStore = defineStore('streamDetail', () => {
  const detail = ref<StreamDetailInfo | null>(null);
  const offline = ref(false);
  const loading = ref(false);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);
  const current = ref<string | null>(null);

  async function fetch(name: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    if (current.value !== name) {
      // Switching broadcasts: reset so we don't show the previous one's data.
      current.value = name;
      detail.value = null;
      offline.value = false;
      loading.value = true;
    }
    try {
      const result = await conn.client.streamDetail(name);
      detail.value = result;
      offline.value = result === null;
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    } finally {
      loading.value = false;
    }
  }

  return { detail, offline, loading, error, lastFetchedAt, current, fetch };
});
