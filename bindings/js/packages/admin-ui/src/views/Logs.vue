<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Card from '@/components/ui/Card.vue';

// LVQR v1.x backlog: SSE / WebSocket admin route for live log tail. Today
// log lines are stdout / file-side only; the right operational pattern is
// `journalctl -u lvqr.service -f` against the host or `kubectl logs -f`
// against the pod.
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / SYSTEM / LOGS">
      <template #title>Live <em>tail.</em></template>
    </PageHeader>

    <EmptyState
      kicker="V1.X BACKLOG"
      title="No live tail route yet."
    >
      LVQR writes structured logs to stdout (configurable via <code>RUST_LOG</code>). Tail at
      the host level: <code>journalctl -u lvqr.service -f</code> on systemd hosts, or
      <code>kubectl logs -f &lt;pod&gt;</code> on Kubernetes. A <code>/api/v1/logs</code> SSE
      stream is on the v1.x backlog.
    </EmptyState>

    <Card kicker="REFERENCE" title="Useful log targets">
      <ul class="ref">
        <li><code>RUST_LOG=lvqr=info</code> -- standard operator level.</li>
        <li><code>RUST_LOG=lvqr=debug,lvqr_relay=trace</code> -- subscriber-side debugging.</li>
        <li><code>RUST_LOG=lvqr=info,lvqr_ingest::rtmp=debug</code> -- RTMP-ingest debugging.</li>
        <li><code>RUST_LOG=lvqr_cluster=debug</code> -- cluster gossip + claim debugging.</li>
      </ul>
    </Card>
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
.ref {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-muted);
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
