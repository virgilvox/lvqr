import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { BroadcastSummary, ClusterNodeView, ConfigEntry, FederationStatus } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * Aggregates the four cluster routes the admin client exposes: nodes,
 * broadcasts, config, federation. Federation is split into its own panel in
 * the UI but lives in the same store because the polling cadence + the
 * 503-on-no-cluster-handle error shape are symmetric.
 */
export const useClusterStore = defineStore('cluster', () => {
  const nodes = ref<ClusterNodeView[]>([]);
  const broadcasts = ref<BroadcastSummary[]>([]);
  const config = ref<ConfigEntry[]>([]);
  const federation = ref<FederationStatus | null>(null);
  const available = ref<boolean>(true);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      const [n, b, c, f] = await Promise.all([
        conn.client.clusterNodes(),
        conn.client.clusterBroadcasts(),
        conn.client.clusterConfig(),
        conn.client.clusterFederation(),
      ]);
      nodes.value = n;
      broadcasts.value = b;
      config.value = c;
      federation.value = f;
      available.value = true;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      // Cluster routes return 500 when the feature is compiled in but no
      // Cluster handle was wired (single-node deployments). Surface that as
      // "cluster not available" rather than a hard error so the rest of the
      // UI keeps working.
      const msg = e instanceof Error ? e.message : String(e);
      if (msg.includes('500')) {
        available.value = false;
        nodes.value = [];
        broadcasts.value = [];
        config.value = [];
        federation.value = { links: [] };
      } else {
        throw e;
      }
    }
  }

  return { nodes, broadcasts, config, federation, available, lastFetchedAt, fetch };
});
