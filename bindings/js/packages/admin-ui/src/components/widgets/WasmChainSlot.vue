<script setup lang="ts">
import type { WasmFilterSlotStats } from '@lvqr/core';

defineProps<{
  slot: WasmFilterSlotStats;
  /** Total slot count -- used to render the chain position. */
  total: number;
}>();
</script>

<template>
  <article class="slot">
    <div class="slot-pos">
      <span>{{ slot.index + 1 }}</span>
      <span class="slot-pos-of">/ {{ total }}</span>
    </div>
    <div class="slot-stats">
      <div>
        <span class="num">{{ slot.seen }}</span>
        <span class="label">seen</span>
      </div>
      <div>
        <span class="num kept">{{ slot.kept }}</span>
        <span class="label">kept</span>
      </div>
      <div>
        <span class="num dropped">{{ slot.dropped }}</span>
        <span class="label">dropped</span>
      </div>
    </div>
  </article>
</template>

<style scoped>
.slot {
  display: grid;
  grid-template-columns: 80px 1fr;
  gap: var(--s-4);
  align-items: center;
  padding: var(--s-3) var(--s-4);
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  border-left: 3px solid var(--tally);
}
.slot-pos {
  display: flex;
  flex-direction: column;
  align-items: center;
}
.slot-pos > span:first-child {
  font-family: var(--font-display);
  font-size: 28px;
  line-height: 1;
  letter-spacing: -0.02em;
  color: var(--ink);
}
.slot-pos-of {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--ink-faint);
  letter-spacing: 0.1em;
}
.slot-stats {
  display: flex;
  gap: var(--s-5);
}
.slot-stats > div {
  display: flex;
  flex-direction: column;
}
.num {
  font-family: var(--font-mono);
  font-size: 18px;
  font-weight: 600;
  color: var(--ink);
}
.kept {
  color: var(--ready);
}
.dropped {
  color: var(--on-air);
}
.label {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
</style>
