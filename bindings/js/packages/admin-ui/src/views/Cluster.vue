<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import { useClusterStore } from '@/stores/cluster';
import { usePolling } from '@/composables/usePolling';
import { formatBytes } from '@/api/url';

const cluster = useClusterStore();
usePolling(() => cluster.fetch(), { intervalMs: 15_000 });
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / INFRASTRUCTURE / CLUSTER">
      <template #title>Cluster <em>topology.</em></template>
    </PageHeader>

    <EmptyState
      v-if="!cluster.available"
      kicker="SINGLE NODE"
      title="Cluster not available on this relay."
    >
      The active relay was not started with a <code>--cluster-listen</code> address, or the
      build does not include the <code>cluster</code> feature. Single-node deployments are
      first-class; the rest of the console works without a cluster.
    </EmptyState>

    <template v-else>
      <Card kicker="NODES" :title="`${cluster.nodes.length} node${cluster.nodes.length === 1 ? '' : 's'}`">
        <div class="node-grid">
          <article v-for="n in cluster.nodes" :key="n.id">
            <header>
              <Badge variant="wire">{{ n.id.slice(0, 12) }}</Badge>
              <span class="addr">{{ n.gossip_addr }}</span>
              <span class="gen">gen {{ n.generation }}</span>
            </header>
            <div v-if="n.capacity" class="capacity">
              <span><strong>{{ n.capacity.cpu_pct.toFixed(1) }}</strong>% cpu</span>
              <span><strong>{{ formatBytes(n.capacity.rss_bytes) }}</strong> rss</span>
              <span><strong>{{ formatBytes(n.capacity.bytes_out_per_sec) }}</strong>/s out</span>
            </div>
            <div v-else class="capacity-pending">No capacity advert yet</div>
          </article>
        </div>
      </Card>

      <Card kicker="BROADCASTS" :title="`${cluster.broadcasts.length} owner lease${cluster.broadcasts.length === 1 ? '' : 's'}`">
        <div class="bc-list">
          <article v-for="b in cluster.broadcasts" :key="b.name">
            <span class="bc-name">{{ b.name }}</span>
            <span class="bc-owner">owner: {{ b.owner.slice(0, 12) }}</span>
            <span class="bc-exp">expires {{ new Date(b.expires_at_ms).toISOString().slice(11, 19) }}</span>
          </article>
          <p v-if="!cluster.broadcasts.length" class="empty">No broadcast leases.</p>
        </div>
      </Card>

      <Card kicker="CONFIG" :title="`${cluster.config.length} entr${cluster.config.length === 1 ? 'y' : 'ies'}`">
        <div class="cfg-list">
          <article v-for="c in cluster.config" :key="c.key">
            <span class="cfg-key">{{ c.key }}</span>
            <span class="cfg-val">{{ c.value }}</span>
          </article>
          <p v-if="!cluster.config.length" class="empty">No cluster-wide config set.</p>
        </div>
      </Card>
    </template>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.node-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: var(--s-3);
}
.node-grid article {
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.node-grid header {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.addr {
  margin-left: auto;
}
.gen {
  font-size: 10px;
  color: var(--ink-faint);
}
.capacity {
  display: flex;
  gap: var(--s-3);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.capacity strong {
  color: var(--ink);
  font-weight: 600;
}
.capacity-pending {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-faint);
}
.bc-list,
.cfg-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: 12px;
}
.bc-list article {
  display: grid;
  grid-template-columns: 1fr 1fr auto;
  gap: var(--s-3);
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-2) var(--s-3);
}
.bc-name {
  color: var(--ink);
}
.bc-owner,
.bc-exp {
  color: var(--ink-muted);
  font-size: 11px;
}
.cfg-list article {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--s-3);
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-2) var(--s-3);
}
.cfg-key {
  color: var(--tally-deep);
}
.cfg-val {
  color: var(--ink);
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
  padding: var(--s-3);
  text-align: center;
}
code {
  font-family: var(--font-mono);
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
}
</style>
