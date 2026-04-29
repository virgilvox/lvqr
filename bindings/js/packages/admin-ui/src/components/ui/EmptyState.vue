<script setup lang="ts">
defineProps<{
  /** Title above the description. */
  title: string;
  /** Optional kicker rendered above the title in mono. */
  kicker?: string;
}>();
</script>

<template>
  <div class="empty">
    <div class="empty-inner">
      <div v-if="kicker" class="empty-kicker">{{ kicker }}</div>
      <h3 class="empty-title">{{ title }}</h3>
      <div v-if="$slots.default" class="empty-body">
        <slot />
      </div>
      <div v-if="$slots.actions" class="empty-actions">
        <slot name="actions" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.empty {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-7) var(--s-5);
  background: var(--paper);
  border: 1px dashed var(--chalk-hi);
}
.empty-inner {
  max-width: 60ch;
  text-align: center;
}
.empty-kicker {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.2em;
  text-transform: uppercase;
  color: var(--ink-faint);
  margin-bottom: var(--s-2);
}
.empty-title {
  font-family: var(--font-display);
  font-size: 28px;
  line-height: 1.1;
  letter-spacing: -0.01em;
  color: var(--ink);
  margin-bottom: var(--s-3);
}
.empty-title :deep(em) {
  color: var(--tally-deep);
}
.empty-body {
  color: var(--ink-muted);
  font-size: 14px;
  line-height: 1.65;
  margin-bottom: var(--s-4);
}
.empty-actions {
  display: inline-flex;
  gap: var(--s-2);
  margin-top: var(--s-2);
}
</style>
