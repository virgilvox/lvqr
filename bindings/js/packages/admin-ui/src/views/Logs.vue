<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch, nextTick } from 'vue';
import type { LogLine } from '@lvqr/core';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import { useConnectionStore } from '@/stores/connection';
import { LOG_LEVELS, type LogLevel, appendCapped, parseLogLine } from '@/api/logTail';

const MAX_LINES = 1000;

const conn = useConnectionStore();

const lines = ref<LogLine[]>([]);
const connected = ref(false);
const paused = ref(false);
const error = ref<string | null>(null);
const activeLevels = ref<Set<LogLevel>>(new Set(LOG_LEVELS));
const logEl = ref<HTMLElement | null>(null);
const autoscroll = ref(true);

let source: EventSource | null = null;

const filtered = computed(() => lines.value.filter((l) => activeLevels.value.has(l.level as LogLevel)));

function levelClass(level: string): string {
  return `lvl lvl-${level.toLowerCase()}`;
}
function fmtTime(ts: number): string {
  const d = new Date(ts);
  return d.toLocaleTimeString(undefined, { hour12: false }) + '.' + String(d.getMilliseconds()).padStart(3, '0');
}

function toggleLevel(level: LogLevel) {
  const next = new Set(activeLevels.value);
  if (next.has(level)) next.delete(level);
  else next.add(level);
  activeLevels.value = next;
}

function connect() {
  disconnect();
  if (!conn.client) return;
  error.value = null;
  try {
    source = new EventSource(conn.client.logsStreamUrl());
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
    return;
  }
  source.onopen = () => {
    connected.value = true;
    error.value = null;
  };
  source.onmessage = (ev: MessageEvent<string>) => {
    if (paused.value) return;
    const line = parseLogLine(ev.data);
    if (!line) return;
    lines.value = appendCapped(lines.value, line, MAX_LINES);
    if (autoscroll.value) void scrollToBottom();
  };
  source.onerror = () => {
    // EventSource auto-reconnects; surface the dropped state meanwhile.
    connected.value = false;
    error.value = 'stream disconnected, retrying...';
  };
}

function disconnect() {
  if (source) {
    source.close();
    source = null;
  }
  connected.value = false;
}

async function scrollToBottom() {
  await nextTick();
  const el = logEl.value;
  if (el) el.scrollTop = el.scrollHeight;
}

function onScroll() {
  const el = logEl.value;
  if (!el) return;
  // Re-enable autoscroll only when the operator is back at the bottom.
  autoscroll.value = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
}

function clear() {
  lines.value = [];
}

// (Re)connect whenever the active connection changes.
watch(
  () => conn.client,
  () => connect(),
  { immediate: true },
);

onBeforeUnmount(disconnect);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / SYSTEM / LOGS">
      <template #title>Live <em>tail.</em></template>
      <template #actions>
        <span class="status" :class="{ on: connected }">{{ connected ? 'LIVE' : 'OFFLINE' }}</span>
        <Button variant="ghost" @click="paused = !paused">
          <Icon name="rec" :size="12" /> {{ paused ? 'Resume' : 'Pause' }}
        </Button>
        <Button variant="ghost" @click="clear"><Icon name="x" :size="12" /> Clear</Button>
      </template>
    </PageHeader>

    <EmptyState v-if="!conn.client" kicker="NO RELAY" title="No connection selected.">
      Add or select a connection profile to tail this relay's logs.
    </EmptyState>

    <template v-else>
      <div class="bar">
        <div class="filters" role="group" aria-label="Level filters">
          <button
            v-for="lvl in LOG_LEVELS"
            :key="lvl"
            type="button"
            class="chip"
            :class="[`chip-${lvl.toLowerCase()}`, { off: !activeLevels.has(lvl) }]"
            :aria-pressed="activeLevels.has(lvl)"
            @click="toggleLevel(lvl)"
          >
            {{ lvl }}
          </button>
        </div>
        <span class="count">{{ filtered.length }} / {{ lines.length }} lines</span>
      </div>

      <p v-if="error" class="err">{{ error }}</p>

      <div ref="logEl" class="log" @scroll="onScroll" role="log" aria-live="polite">
        <div v-for="(l, i) in filtered" :key="i" class="row">
          <span class="ts">{{ fmtTime(l.ts_ms) }}</span>
          <span :class="levelClass(l.level)">{{ l.level }}</span>
          <span class="target">{{ l.target }}</span>
          <span class="msg">{{ l.message }}</span>
        </div>
        <p v-if="!filtered.length" class="placeholder">
          {{ lines.length ? 'No lines match the active level filter.' : 'Waiting for log lines...' }}
        </p>
      </div>

      <p class="hint">
        Lines are tailed live over Server-Sent Events from <code>GET /api/v1/logs</code>; the most
        recent {{ MAX_LINES }} are kept in the buffer. Adjust verbosity at the relay with
        <code>RUST_LOG</code>.
      </p>
    </template>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.status {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.12em;
  color: var(--ink-faint);
}
.status.on {
  color: var(--on-air);
}
.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.filters {
  display: flex;
  gap: var(--s-2);
}
.chip {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.1em;
  padding: 3px 9px;
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  cursor: pointer;
  color: var(--ink-light);
}
.chip.off {
  opacity: 0.4;
}
.chip-error {
  border-color: var(--on-air);
  color: var(--on-air);
}
.chip-warn {
  border-color: var(--warn);
  color: var(--warn);
}
.count {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-faint);
}
.err {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--warn);
}
.log {
  background: var(--ink);
  color: var(--bone);
  font-family: var(--font-mono);
  font-size: 12px;
  line-height: 1.5;
  padding: var(--s-3);
  height: 60vh;
  overflow-y: auto;
  border: 1px solid var(--chalk-hi);
}
.row {
  display: grid;
  grid-template-columns: max-content max-content max-content 1fr;
  gap: var(--s-3);
  white-space: pre-wrap;
  word-break: break-word;
}
.ts {
  color: var(--ink-ghost);
}
.target {
  color: var(--wire);
}
.lvl {
  font-weight: 700;
}
.lvl-error {
  color: #ff6b6b;
}
.lvl-warn {
  color: #ffd166;
}
.lvl-info {
  color: #8ad6cc;
}
.lvl-debug {
  color: var(--ink-ghost);
}
.lvl-trace {
  color: var(--ink-faint);
}
.placeholder {
  color: var(--ink-faint);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
code {
  background: var(--chalk-lo);
  color: var(--ink);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .row {
    grid-template-columns: max-content max-content 1fr;
  }
  .row .target {
    display: none;
  }
}
</style>
