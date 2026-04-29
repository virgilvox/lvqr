import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { StreamKey, StreamKeySpec } from '@lvqr/core';
import { useConnectionStore } from './connection';

export const useStreamKeysStore = defineStore('streamkeys', () => {
  const keys = ref<StreamKey[]>([]);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    keys.value = await conn.client.listStreamKeys();
    lastFetchedAt.value = Date.now();
  }

  async function mint(spec: StreamKeySpec): Promise<StreamKey> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    const key = await conn.client.mintStreamKey(spec);
    await fetch();
    return key;
  }

  async function revoke(id: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.revokeStreamKey(id);
    await fetch();
  }

  async function rotate(id: string, override?: StreamKeySpec): Promise<StreamKey> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    const key = await conn.client.rotateStreamKey(id, override);
    await fetch();
    return key;
  }

  return { keys, lastFetchedAt, fetch, mint, revoke, rotate };
});
