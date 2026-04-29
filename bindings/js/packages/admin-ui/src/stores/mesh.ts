import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { MeshState } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useMeshStore = defineStore('mesh', () => {
  const mesh = ref<MeshState | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    mesh.value = await conn.client.mesh();
    lastFetchedAt.value = Date.now();
  }

  return { mesh, lastFetchedAt, fetch };
});
