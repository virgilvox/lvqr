<script setup lang="ts">
import { computed } from 'vue';
import { useConnectionStore } from '@/stores/connection';
import { useHealthStore } from '@/stores/health';
import { useStatsStore } from '@/stores/stats';
import { formatRelativeTime } from '@/api/url';

const conn = useConnectionStore();
const health = useHealthStore();
const stats = useStatsStore();

const activeUrl = computed(() => conn.activeProfile?.baseUrl ?? '-');
const lastHealth = computed(() => formatRelativeTime(health.lastFetchedAt ?? null));
const subs = computed(() => stats.stats?.subscribers ?? 0);
const tracks = computed(() => stats.stats?.tracks ?? 0);
const publishers = computed(() => stats.stats?.publishers ?? 0);
</script>

<template>
  <footer class="statusbar">
    <span class="sb-section">
      <span class="sb-dot" :class="{ warn: !health.healthy }" />
      <span>{{ health.healthy ? 'OK' : 'down' }}</span>
    </span>
    <span class="sb-section">
      <span>relay</span><span>{{ activeUrl }}</span>
    </span>
    <span class="sb-section">
      <span>subs</span><span>{{ subs }}</span>
    </span>
    <span class="sb-section">
      <span>tracks</span><span>{{ tracks }}</span>
    </span>
    <span class="sb-section">
      <span>pubs</span><span>{{ publishers }}</span>
    </span>
    <span class="sb-spacer" />
    <span class="sb-section">
      <span>last health</span><span>{{ lastHealth }}</span>
    </span>
  </footer>
</template>

<style scoped>
.statusbar {
  grid-area: statusbar;
  background: var(--ink);
  color: var(--ink-ghost);
  display: flex;
  align-items: center;
  padding: 0 var(--s-5);
  gap: var(--s-5);
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.05em;
  border-top: 1px solid #000;
}
.sb-section {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}
.sb-section + .sb-section {
  padding-left: var(--s-5);
  border-left: 1px solid var(--ink-light);
}
.sb-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--ready);
  box-shadow: 0 0 6px rgba(22, 163, 74, 0.5);
}
.sb-dot.warn {
  background: var(--warn);
  box-shadow: 0 0 6px rgba(217, 119, 6, 0.5);
}
.sb-spacer {
  flex: 1;
}
@media (max-width: 767px) {
  .statusbar {
    overflow-x: auto;
  }
}
</style>
