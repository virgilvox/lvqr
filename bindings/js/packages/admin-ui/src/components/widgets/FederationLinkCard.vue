<script setup lang="ts">
import type { FederationLinkStatus } from '@lvqr/core';
import Tally from '@/components/ui/Tally.vue';
import Badge from '@/components/ui/Badge.vue';
import { formatRelativeTime } from '@/api/url';

const props = defineProps<{ link: FederationLinkStatus }>();

function statusKind(): 'on-air' | 'ready' | 'warn' | 'idle' {
  if (props.link.state === 'connected') return 'ready';
  if (props.link.state === 'connecting') return 'warn';
  if (props.link.state === 'failed') return 'on-air';
  return 'idle';
}
</script>

<template>
  <article class="fed">
    <header>
      <Tally :status="statusKind()" :label="link.state" />
      <span class="fed-url">{{ link.remote_url }}</span>
    </header>
    <div class="fed-grid">
      <div>
        <span class="num">{{ link.connect_attempts }}</span>
        <span class="label">connect attempts</span>
      </div>
      <div>
        <span class="num">{{ link.forwarded_broadcasts_seen }}</span>
        <span class="label">announces seen</span>
      </div>
      <div>
        <span class="num">{{ formatRelativeTime(link.last_connected_at_ms) }}</span>
        <span class="label">last connect</span>
      </div>
    </div>
    <div class="fed-bcs">
      <Badge v-for="b in link.forwarded_broadcasts" :key="b" variant="wire">{{ b }}</Badge>
      <span v-if="!link.forwarded_broadcasts.length" class="empty">no forwarded broadcasts configured</span>
    </div>
    <p v-if="link.last_error" class="fed-error">{{ link.last_error }}</p>
  </article>
</template>

<style scoped>
.fed {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-4) var(--s-5);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
header {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  border-bottom: 1px solid var(--chalk);
  padding-bottom: var(--s-2);
}
.fed-url {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink);
  word-break: break-all;
  margin-left: auto;
}
.fed-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: var(--s-3);
}
.fed-grid > div {
  display: flex;
  flex-direction: column;
}
.num {
  font-family: var(--font-display);
  font-size: 22px;
  letter-spacing: -0.01em;
}
.label {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.fed-bcs {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.empty {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-faint);
}
.fed-error {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--on-air);
  background: rgba(220, 38, 38, 0.06);
  padding: var(--s-2);
  border: 1px solid var(--on-air);
}
</style>
