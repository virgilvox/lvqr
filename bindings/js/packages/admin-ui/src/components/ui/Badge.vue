<script setup lang="ts">
defineProps<{
  /** Visual variant. */
  variant?: 'neutral' | 'tally' | 'wire' | 'ready' | 'warn' | 'on-air';
}>();
</script>

<template>
  <span class="badge" :class="`badge-${variant ?? 'neutral'}`">
    <slot />
  </span>
</template>

<style scoped>
.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  padding: 2px 8px;
  border: 1px solid;
}
.badge-neutral {
  border-color: var(--chalk-hi);
  background: var(--chalk-lo);
  color: var(--ink-muted);
}
.badge-tally {
  border-color: var(--tally-deep);
  background: var(--tally-wash);
  color: var(--tally-deep);
}
.badge-wire {
  border-color: var(--wire-deep);
  background: var(--wire-wash);
  color: var(--wire-deep);
}
.badge-ready {
  border-color: var(--ready);
  background: rgba(22, 163, 74, 0.1);
  color: var(--ready);
}
.badge-warn {
  border-color: var(--warn);
  background: rgba(217, 119, 6, 0.1);
  color: var(--warn);
}
.badge-on-air {
  border-color: var(--on-air);
  background: rgba(220, 38, 38, 0.12);
  color: var(--on-air);
}
</style>
