<script setup lang="ts">
import type { SloEntry } from '@lvqr/core';

defineProps<{
  /** One row from the SLO snapshot. */
  entry: SloEntry;
}>();

function pctColor(p99_ms: number): string {
  if (p99_ms < 250) return 'var(--ready)';
  if (p99_ms < 1000) return 'var(--tally)';
  return 'var(--on-air)';
}
</script>

<template>
  <article class="slo-card">
    <header>
      <span class="slo-broadcast">{{ entry.broadcast }}</span>
      <span class="slo-transport">{{ entry.transport }}</span>
    </header>
    <div class="slo-grid">
      <div>
        <span class="slo-num" :style="{ color: pctColor(entry.p99_ms) }">{{ entry.p99_ms }}</span>
        <span class="slo-label">p99 ms</span>
      </div>
      <div>
        <span class="slo-num">{{ entry.p95_ms }}</span>
        <span class="slo-label">p95</span>
      </div>
      <div>
        <span class="slo-num">{{ entry.p50_ms }}</span>
        <span class="slo-label">p50</span>
      </div>
      <div>
        <span class="slo-num">{{ entry.max_ms }}</span>
        <span class="slo-label">max</span>
      </div>
    </div>
    <footer>
      {{ entry.sample_count }} retained / {{ entry.total_observed }} observed
    </footer>
  </article>
</template>

<style scoped>
.slo-card {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-3) var(--s-4) var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
header {
  display: flex;
  align-items: baseline;
  gap: var(--s-2);
  border-bottom: 1px solid var(--chalk);
  padding-bottom: var(--s-2);
}
.slo-broadcast {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink);
}
.slo-transport {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--tally-deep);
  margin-left: auto;
}
.slo-grid {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: var(--s-3);
}
.slo-grid > div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.slo-num {
  font-family: var(--font-display);
  font-size: 26px;
  line-height: 1;
  letter-spacing: -0.01em;
  color: var(--ink);
}
.slo-label {
  font-family: var(--font-mono);
  font-size: 9px;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
footer {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
</style>
