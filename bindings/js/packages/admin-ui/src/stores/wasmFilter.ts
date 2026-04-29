import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { WasmFilterState } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useWasmFilterStore = defineStore('wasmFilter', () => {
  const state = ref<WasmFilterState | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    state.value = await conn.client.wasmFilter();
    lastFetchedAt.value = Date.now();
  }

  return { state, lastFetchedAt, fetch };
});
