<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import StreamRow from '@/components/widgets/StreamRow.vue';
import SloEntryCard from '@/components/widgets/SloEntryCard.vue';
import { useStatsStore } from '@/stores/stats';
import { useStreamsStore } from '@/stores/streams';
import { useSloStore } from '@/stores/slo';
import { usePolling } from '@/composables/usePolling';
import { formatBytes, formatDuration, formatRelativeTime } from '@/api/url';

const stats = useStatsStore();
const streams = useStreamsStore();
const slo = useSloStore();

usePolling(() => stats.fetch(), { intervalMs: 5_000 });
usePolling(() => streams.fetch(), { intervalMs: 10_000 });
usePolling(() => slo.fetch(), { intervalMs: 30_000 });

const topSloEntries = computed(() => (slo.slo?.broadcasts ?? []).slice(0, 4));
const topStreams = computed(() => streams.streams.slice(0, 6));
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS">
      <template #title>Tallyboard <em>overview.</em></template>
      <template #actions>
        <Button variant="ghost" @click="stats.fetch(); streams.fetch(); slo.fetch()">
          <Icon name="reload" :size="12" /> Reload
        </Button>
      </template>
    </PageHeader>

    <section class="kpi-grid">
      <KpiTile label="Subscribers" :value="stats.stats?.subscribers ?? 0" />
      <KpiTile label="Active broadcasts" :value="stats.stats?.publishers ?? 0" accent="wire" />
      <KpiTile label="Tracks" :value="stats.stats?.tracks ?? 0" />
      <KpiTile
        label="Bytes out"
        :value="formatBytes(stats.stats?.bytes_sent ?? 0)"
        :hint="`in: ${formatBytes(stats.stats?.bytes_received ?? 0)}`"
        accent="wire"
      />
      <KpiTile
        label="Uptime"
        :value="formatDuration(stats.stats?.uptime_secs ?? 0)"
        :hint="`updated ${formatRelativeTime(stats.lastFetchedAt)}`"
        accent="none"
      />
    </section>

    <section class="dual">
      <Card kicker="LIVE" title="Top streams" wire>
        <div class="streams-list">
          <StreamRow v-for="s in topStreams" :key="s.name" :stream="s" />
          <p v-if="!topStreams.length" class="empty">
            No active streams. Push to <code>rtmp://&lt;relay&gt;/live/&lt;key&gt;</code>.
          </p>
        </div>
        <template #actions>
          <RouterLink to="/streams">
            <Button small variant="ghost">All streams</Button>
          </RouterLink>
        </template>
      </Card>

      <Card kicker="SLO" title="Top latency rows">
        <div class="slo-list">
          <SloEntryCard v-for="e in topSloEntries" :key="e.broadcast + e.transport" :entry="e" />
          <p v-if="!topSloEntries.length" class="empty">
            No SLO samples yet. Egress traffic populates this card automatically.
          </p>
        </div>
        <template #actions>
          <RouterLink to="/egress">
            <Button small variant="ghost">SLO detail</Button>
          </RouterLink>
        </template>
      </Card>
    </section>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
}
.kpi-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
  margin-bottom: var(--s-6);
}
.dual {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--s-4);
}
.streams-list,
.slo-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.empty {
  padding: var(--s-3);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-faint);
}
.empty code {
  color: var(--tally-deep);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .dual {
    grid-template-columns: 1fr;
  }
}
</style>
