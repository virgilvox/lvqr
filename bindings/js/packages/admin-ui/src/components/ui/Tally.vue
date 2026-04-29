<script setup lang="ts">
defineProps<{
  /** Status the dot represents. */
  status: 'on-air' | 'ready' | 'warn' | 'idle';
  /** Optional label to render alongside the dot. */
  label?: string;
}>();
</script>

<template>
  <span class="tally" :class="`tally-${status}`">
    <span class="tally-dot" />
    <span v-if="label" class="tally-label">{{ label }}</span>
    <slot />
  </span>
</template>

<style scoped>
.tally {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  text-transform: uppercase;
}
.tally-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--idle);
}
.tally-on-air .tally-dot {
  background: var(--on-air);
  box-shadow: 0 0 10px var(--on-air-glow);
  animation: pulse 1.2s ease-in-out infinite;
}
.tally-ready .tally-dot {
  background: var(--ready);
  box-shadow: 0 0 8px rgba(22, 163, 74, 0.5);
}
.tally-warn .tally-dot {
  background: var(--warn);
  box-shadow: 0 0 8px rgba(217, 119, 6, 0.5);
}
.tally-idle .tally-dot {
  background: var(--idle);
}
</style>
