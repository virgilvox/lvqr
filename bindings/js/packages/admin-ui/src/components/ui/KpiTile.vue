<script setup lang="ts">
defineProps<{
  /** Tile label (e.g. "Active streams"). */
  label: string;
  /** Primary value rendered in display type. */
  value: string | number;
  /** Optional unit / suffix rendered next to the value. */
  unit?: string;
  /** Optional secondary line (delta, p99, etc.). */
  hint?: string;
  /** Tile accent: `tally` (amber, default), `wire` (cyan), or `none`. */
  accent?: 'tally' | 'wire' | 'none';
}>();
</script>

<template>
  <article class="kpi" :class="`kpi-accent-${accent ?? 'tally'}`">
    <div class="kpi-label">{{ label }}</div>
    <div class="kpi-value">
      <span>{{ value }}</span>
      <span v-if="unit" class="kpi-unit">{{ unit }}</span>
    </div>
    <div v-if="hint" class="kpi-hint">{{ hint }}</div>
    <slot name="visual" />
  </article>
</template>

<style scoped>
.kpi {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-4) var(--s-5);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  position: relative;
  min-height: 120px;
}
.kpi-accent-tally {
  border-left: 3px solid var(--tally);
}
.kpi-accent-wire {
  border-left: 3px solid var(--wire);
}
.kpi-accent-none {
  border-left: 3px solid transparent;
}
.kpi-label {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.kpi-value {
  font-family: var(--font-display);
  font-size: 36px;
  line-height: 1;
  letter-spacing: -0.02em;
  color: var(--ink);
  display: flex;
  align-items: baseline;
  gap: var(--s-2);
}
.kpi-unit {
  font-family: var(--font-mono);
  font-size: 12px;
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: var(--ink-muted);
}
.kpi-hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
</style>
