<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Badge from '@/components/ui/Badge.vue';
import { useArchiveStore } from '@/stores/archive';
import { usePolling } from '@/composables/usePolling';

// Read-only introspection of the DVR segment index via GET /api/v1/archive.
// Each recorded broadcast links into the /dvr scrubber.
const archive = useArchiveStore();
usePolling(() => archive.fetch(), { intervalMs: 15_000 });

const state = computed(() => archive.state);
const enabled = computed(() => state.value?.enabled ?? false);
const recordings = computed(() => state.value?.recordings ?? []);
const totalBytes = computed(() => recordings.value.reduce((a, r) => a + r.total_bytes, 0));

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

function fmtDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS / RECORDINGS">
      <template #title>Archive <em>vault.</em></template>
    </PageHeader>

    <p v-if="archive.error" class="state state-error">{{ archive.error }}</p>

    <div v-else-if="!state" class="state state-loading"><span class="spinner" /> Loading recordings...</div>

    <template v-else-if="enabled && recordings.length">
      <section class="kpis">
        <KpiTile label="Recordings" :value="recordings.length" />
        <KpiTile label="Total size" :value="fmtBytes(totalBytes)" accent="wire" />
      </section>

      <div class="rows">
        <Card v-for="r in recordings" :key="r.broadcast" :kicker="`${r.tracks.length} TRACK${r.tracks.length === 1 ? '' : 'S'}`" :title="r.broadcast">
          <template #actions>
            <RouterLink :to="{ path: '/dvr', query: { broadcast: r.broadcast } }">
              <Button variant="primary">Open in scrubber</Button>
            </RouterLink>
          </template>
          <dl class="meta">
            <div><dt>Duration</dt><dd class="mono">{{ fmtDuration(r.duration_secs) }}</dd></div>
            <div><dt>Size</dt><dd class="mono">{{ fmtBytes(r.total_bytes) }}</dd></div>
            <div><dt>Segments</dt><dd class="mono">{{ r.segment_count.toLocaleString() }}</dd></div>
          </dl>
          <div class="tracks">
            <Badge v-for="t in r.tracks" :key="t.track" variant="neutral">
              {{ t.track }} &middot; {{ fmtDuration(t.duration_secs) }} &middot; {{ fmtBytes(t.total_bytes) }}
            </Badge>
          </div>
        </Card>
      </div>
    </template>

    <EmptyState v-else-if="enabled" kicker="EMPTY" title="No recordings yet.">
      Archiving is enabled (<code>--archive-dir</code>) but the segment index is empty. Publish a
      broadcast and its segments will appear here as they are written.
    </EmptyState>

    <EmptyState v-else kicker="NOT CONFIGURED" title="Archiving is off on this relay.">
      Start the relay with <code>--archive-dir &lt;path&gt;</code> (or an S3-compatible object-store
      backend) to record broadcasts to a DVR segment index. Once enabled, recorded broadcasts list
      here and link straight into the
      <RouterLink to="/dvr">DVR scrubber</RouterLink>.
    </EmptyState>
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
.rows {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.meta {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2) var(--s-5);
  margin-bottom: var(--s-3);
}
.meta dt {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.meta dd {
  font-size: 14px;
}
.tracks {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.mono {
  font-family: var(--font-mono);
}
.state {
  font-family: var(--font-mono);
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: var(--s-3) 0;
}
.state-error {
  color: var(--on-air);
}
.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--chalk-hi);
  border-top-color: var(--tally-deep);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
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
