import { defineStore } from 'pinia';
import { ref } from 'vue';
import { useConnectionStore } from './connection';

export const useHealthStore = defineStore('health', () => {
  const healthy = ref<boolean>(false);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) {
      healthy.value = false;
      return;
    }
    healthy.value = await conn.client.healthz();
    lastFetchedAt.value = Date.now();
  }

  return { healthy, lastFetchedAt, fetch };
});
