<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import { useAgentsStore } from '@/stores/agents';
import { usePolling } from '@/composables/usePolling';

// Read-only introspection of startup-configured in-process agents via
// GET /api/v1/agents. Runtime start/stop is a separate CRUD surface tracked
// for a later slice; this view lists configured agents + live attachments.
const agents = useAgentsStore();
usePolling(() => agents.fetch(), { intervalMs: 10_000 });

const state = computed(() => agents.state);
const enabled = computed(() => state.value?.enabled ?? false);
const configured = computed(() => state.value?.agents ?? []);
const active = computed(() => state.value?.active ?? []);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / AGENTS">
      <template #title>In-process <em>agents.</em></template>
    </PageHeader>

    <p v-if="agents.error" class="state state-error">{{ agents.error }}</p>

    <div v-else-if="!state" class="state state-loading"><span class="spinner" /> Loading agents...</div>

    <template v-else-if="enabled">
      <section class="kpis">
        <KpiTile label="Configured agents" :value="configured.length" />
        <KpiTile label="Live attachments" :value="active.length" accent="wire" />
      </section>

      <Card v-for="a in configured" :key="a.name" :kicker="a.kind.toUpperCase()" :title="a.name">
        <template #actions><Badge variant="ready">attached</Badge></template>
        <dl class="meta">
          <template v-if="a.model">
            <dt>Model</dt>
            <dd class="mono">{{ a.model }}</dd>
          </template>
          <template v-if="a.window_ms != null">
            <dt>Window</dt>
            <dd class="mono">{{ a.window_ms }} ms</dd>
          </template>
        </dl>
      </Card>

      <Card kicker="LIVE" title="Attachments" wire>
        <div v-if="active.length" class="atable" role="table" aria-label="Live agent attachments">
          <div class="tr th" role="row">
            <span role="columnheader">Agent</span>
            <span role="columnheader">Source</span>
            <span role="columnheader">Track</span>
            <span role="columnheader" class="num">Fragments</span>
            <span role="columnheader" class="num">Panics</span>
          </div>
          <div v-for="x in active" :key="`${x.agent}|${x.broadcast}|${x.track}`" class="tr" role="row">
            <span role="cell"><Badge variant="tally">{{ x.agent }}</Badge></span>
            <span role="cell" class="mono">{{ x.broadcast }}</span>
            <span role="cell" class="mono">{{ x.track }}</span>
            <span role="cell" class="num">{{ x.fragments_seen.toLocaleString() }}</span>
            <span role="cell" class="num" :class="{ warn: x.panics > 0 }">{{ x.panics }}</span>
          </div>
        </div>
        <p v-else class="empty">
          No agents are attached to a live broadcast yet. Agents attach when a matching source track
          starts publishing.
        </p>
      </Card>
    </template>

    <template v-else>
      <EmptyState kicker="NOT CONFIGURED" title="No in-process agents on this relay.">
        This relay has no agents configured (or was built without an agent feature such as
        <code>whisper</code>). Attach the Whisper captions agent by adding
        <code>--whisper-model &lt;path/to/ggml-tiny.en.bin&gt;</code> to <code>lvqr serve</code>
        (build with <code>--features whisper</code>). The agent emits a sibling captions track that
        <code>@lvqr/dvr-player</code> renders through the standard HLS subtitle rendition.
      </EmptyState>
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
.kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
}
.meta {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 4px var(--s-4);
  font-size: 13px;
}
.meta dt {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  align-self: center;
}
.atable {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.tr {
  display: grid;
  grid-template-columns: 1fr 1.6fr 0.8fr 1fr 0.6fr;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.mono {
  font-family: var(--font-mono);
}
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
  font-family: var(--font-mono);
}
.num.warn {
  color: var(--warn);
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
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
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
  .tr {
    grid-template-columns: 1fr 1.4fr 0.6fr 0.6fr;
  }
  .tr span:nth-child(3) {
    display: none;
  }
}
</style>
