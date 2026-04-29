<script setup lang="ts">
import type { StreamKey } from '@lvqr/core';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';

defineProps<{
  keys: StreamKey[];
}>();

const emit = defineEmits<{
  rotate: [id: string];
  revoke: [id: string];
  copyToken: [token: string];
}>();

function fmtUnix(secs?: number | null): string {
  if (!secs) return '-';
  return new Date(secs * 1000).toISOString().replace('T', ' ').slice(0, 19);
}

function isExpired(k: StreamKey): boolean {
  return !!k.expires_at && k.expires_at * 1000 < Date.now();
}

async function copy(token: string) {
  try {
    await navigator.clipboard.writeText(token);
    emit('copyToken', token);
  } catch {
    /* no clipboard permission; the row's reveal action remains */
  }
}
</script>

<template>
  <div class="sk-table">
    <header class="sk-row sk-head">
      <span>Label</span>
      <span>Broadcast</span>
      <span>Token</span>
      <span>Created</span>
      <span>Expires</span>
      <span></span>
    </header>
    <article v-for="k in keys" :key="k.id" class="sk-row" :class="{ expired: isExpired(k) }">
      <span>{{ k.label ?? '-' }}</span>
      <span>{{ k.broadcast ?? 'any' }}</span>
      <span class="token-cell">
        <code>{{ k.token.slice(0, 18) }}...</code>
        <button class="copy" @click="copy(k.token)" :title="'Copy ' + k.token">
          <Icon name="copy" :size="12" />
        </button>
      </span>
      <span>{{ fmtUnix(k.created_at) }}</span>
      <span>{{ fmtUnix(k.expires_at) }}</span>
      <span class="actions">
        <Button small variant="ghost" @click="emit('rotate', k.id)">rotate</Button>
        <Button small variant="danger" @click="emit('revoke', k.id)">revoke</Button>
      </span>
    </article>
    <p v-if="!keys.length" class="empty">No stream keys minted yet.</p>
  </div>
</template>

<style scoped>
.sk-table {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  overflow-x: auto;
}
.sk-row {
  display: grid;
  grid-template-columns: 1fr 1.4fr 1.6fr 1fr 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--chalk);
  font-family: var(--font-mono);
  font-size: 12px;
}
.sk-row:last-child {
  border-bottom: none;
}
.sk-head {
  background: var(--chalk-lo);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  font-weight: 700;
}
.expired {
  background: rgba(217, 119, 6, 0.06);
}
.token-cell {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}
.token-cell code {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.copy {
  border: 1px solid var(--chalk-hi);
  padding: 3px 5px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.copy:hover {
  background: var(--ink);
  color: var(--paper);
  border-color: var(--ink);
}
.actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
}
.empty {
  padding: var(--s-5);
  text-align: center;
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}

@media (max-width: 767px) {
  .sk-row {
    grid-template-columns: 1fr 1fr;
    gap: var(--s-2);
    padding: var(--s-3);
  }
  .sk-head {
    display: none;
  }
}
</style>
