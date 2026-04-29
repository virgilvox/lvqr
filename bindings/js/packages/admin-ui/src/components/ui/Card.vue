<script setup lang="ts">
defineProps<{
  /** Card title. */
  title?: string;
  /** Optional kicker rendered above the title in mono. */
  kicker?: string;
  /** Apply the wire-cyan border variant. */
  wire?: boolean;
}>();
</script>

<template>
  <section class="card" :class="{ 'card-wire': wire }">
    <header v-if="title || kicker || $slots.actions" class="card-head">
      <div>
        <div v-if="kicker" class="card-kicker">{{ kicker }}</div>
        <h2 v-if="title" class="card-title">{{ title }}</h2>
      </div>
      <div v-if="$slots.actions" class="card-actions">
        <slot name="actions" />
      </div>
    </header>
    <div class="card-body">
      <slot />
    </div>
  </section>
</template>

<style scoped>
.card {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
}
.card-wire {
  border-color: var(--wire);
  border-left-width: 3px;
}
.card-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--s-4);
  padding: var(--s-4) var(--s-5) var(--s-3);
  border-bottom: 1px solid var(--chalk);
}
.card-kicker {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-faint);
  margin-bottom: 2px;
}
.card-title {
  font-family: var(--font-display);
  font-size: 22px;
  font-weight: 400;
  line-height: 1;
  letter-spacing: -0.01em;
  color: var(--ink);
}
.card-actions {
  display: flex;
  gap: var(--s-2);
  align-items: center;
}
.card-body {
  padding: var(--s-4) var(--s-5);
}
</style>
