<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Button from '@/components/ui/Button.vue';
import { useStatsStore } from '@/stores/stats';
import { useSloStore } from '@/stores/slo';
import { useConnectionStore } from '@/stores/connection';
import { usePolling } from '@/composables/usePolling';
import { joinUrl, formatBytes } from '@/api/url';

const stats = useStatsStore();
const slo = useSloStore();
const conn = useConnectionStore();

usePolling(() => stats.fetch(), { intervalMs: 5_000 });
usePolling(() => slo.fetch(), { intervalMs: 30_000 });

const metricsUrl = computed(() =>
  conn.activeProfile ? joinUrl(conn.activeProfile.baseUrl, '/metrics') : '#',
);
const transports = computed(() => {
  const set = new Set<string>();
  for (const e of slo.slo?.broadcasts ?? []) set.add(e.transport);
  return Array.from(set);
});
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / SYSTEM / OBSERVABILITY">
      <template #title>Telemetry <em>spine.</em></template>
      <template #actions>
        <a :href="metricsUrl" target="_blank" rel="noopener">
          <Button variant="ghost">Open /metrics</Button>
        </a>
      </template>
    </PageHeader>

    <section class="kpis">
      <KpiTile label="Subscribers" :value="stats.stats?.subscribers ?? 0" />
      <KpiTile label="Bytes out" :value="formatBytes(stats.stats?.bytes_sent ?? 0)" accent="wire" />
      <KpiTile label="Bytes in" :value="formatBytes(stats.stats?.bytes_received ?? 0)" accent="none" />
      <KpiTile label="SLO transports" :value="transports.length" />
    </section>

    <Card kicker="PROMETHEUS" title="Scrape recipe">
      <pre>scrape_configs:
  - job_name: lvqr
    static_configs:
      - targets: ['{{ conn.activeProfile?.baseUrl?.replace(/^https?:\/\//, '') ?? '&lt;relay&gt;' }}']
    metrics_path: /metrics</pre>
      <p class="hint">
        <code>/metrics</code> is unauthenticated by design (a Prometheus scraper is typically
        a low-privilege internal service); operators that need auth on the scrape route should
        front the relay with a network-side gate.
      </p>
    </Card>

    <Card kicker="OTEL" title="OTLP exporter">
      <p>
        For trace + metric export to a sidecar collector, boot the relay with
        <code>--otlp-endpoint &lt;url&gt;</code>. See <code>docs/observability.md</code>.
      </p>
    </Card>

    <!-- LVQR v1.x backlog: a JSON metrics route + Grafana iframe panel. The
         /metrics route returns Prometheus text-format; rendering charts here
         would require a JSON adapter that does not exist server-side today. -->
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
.kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
}
pre {
  background: var(--ink);
  color: var(--paper);
  font-family: var(--font-mono);
  font-size: 11px;
  padding: var(--s-3) var(--s-4);
  overflow-x: auto;
}
p {
  font-size: 13px;
  color: var(--ink-muted);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-top: var(--s-2);
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
