<script setup lang="ts">
defineProps<{
  /** Optional crumb rendered above the title in mono. */
  crumb?: string;
  /** Page title. Use <em> in the slot for the italic accent if needed. */
  title?: string;
  /** Optional 1-2 sentence description. */
  description?: string;
}>();
</script>

<template>
  <header class="ph">
    <div class="ph-text">
      <div v-if="crumb" class="ph-crumb">{{ crumb }}</div>
      <h1 class="ph-title">
        <slot name="title">{{ title }}</slot>
      </h1>
      <p v-if="description" class="ph-desc">{{ description }}</p>
    </div>
    <div v-if="$slots.actions" class="ph-actions">
      <slot name="actions" />
    </div>
  </header>
</template>

<style scoped>
.ph {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--s-5);
  flex-wrap: wrap;
  margin-bottom: var(--s-6);
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--chalk-hi);
}
.ph-crumb {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  color: var(--ink-faint);
  text-transform: uppercase;
  margin-bottom: 6px;
}
.ph-title {
  font-family: var(--font-display);
  font-size: clamp(28px, 5vw, 48px);
  font-weight: 400;
  line-height: 1;
  letter-spacing: -0.02em;
  color: var(--ink);
}
.ph-title :deep(em) {
  font-style: italic;
  color: var(--tally-deep);
}
.ph-desc {
  margin-top: var(--s-2);
  color: var(--ink-muted);
  font-size: 14px;
  max-width: 60ch;
}
.ph-actions {
  display: flex;
  gap: var(--s-2);
  align-items: center;
}
</style>
