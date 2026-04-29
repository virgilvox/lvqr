<script setup lang="ts">
import type { StreamInfo } from '@lvqr/core';
import Tally from '@/components/ui/Tally.vue';

defineProps<{
  /** Stream info from `/api/v1/streams`. */
  stream: StreamInfo;
}>();
</script>

<template>
  <RouterLink :to="`/streams/${encodeURIComponent(stream.name)}`" class="stream-row">
    <Tally status="on-air" />
    <div class="stream-name">{{ stream.name }}</div>
    <div class="stream-subs">{{ stream.subscribers }} <span>subs</span></div>
  </RouterLink>
</template>

<style scoped>
.stream-row {
  display: grid;
  grid-template-columns: auto 1fr auto;
  align-items: center;
  gap: var(--s-4);
  padding: var(--s-3) var(--s-4);
  border: 1px solid var(--chalk-hi);
  background: var(--paper);
  transition: all 0.1s;
}
.stream-row:hover {
  border-color: var(--tally);
  background: var(--paper-hi);
}
.stream-name {
  font-family: var(--font-mono);
  font-size: 13px;
  color: var(--ink);
  word-break: break-all;
}
.stream-subs {
  font-family: var(--font-mono);
  font-size: 13px;
  color: var(--tally-deep);
  font-weight: 600;
}
.stream-subs span {
  color: var(--ink-faint);
  font-weight: 400;
  font-size: 10px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  margin-left: 2px;
}
</style>
