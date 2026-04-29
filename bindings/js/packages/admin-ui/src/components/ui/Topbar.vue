<script setup lang="ts">
import { computed } from 'vue';
import Icon from './Icon.vue';
import Tally from './Tally.vue';
import { useConnectionStore } from '@/stores/connection';
import { useHealthStore } from '@/stores/health';

const conn = useConnectionStore();
const health = useHealthStore();

defineEmits<{
  toggleRail: [];
  pickConnection: [];
}>();

const activeLabel = computed(() => conn.activeProfile?.label ?? 'no connection');
const initials = computed(() => {
  const lbl = conn.activeProfile?.label ?? '?';
  const parts = lbl.trim().split(/\s+/);
  return ((parts[0]?.[0] ?? '?') + (parts[1]?.[0] ?? '')).toUpperCase();
});
</script>

<template>
  <div class="topbar">
    <div class="tb-bg" aria-hidden="true" />
    <button class="tb-menu" aria-label="Toggle navigation" @click="$emit('toggleRail')">
      <Icon name="menu" :size="18" />
    </button>
    <div class="tb-brand">
      <span class="tb-logo">LVQR<span class="tb-logo-dot" /></span>
      <span class="tb-version">v1.0.0</span>
    </div>
    <span class="tb-divider" />
    <div class="tb-cluster">
      <Tally :status="health.healthy ? 'ready' : 'idle'" :label="activeLabel" />
    </div>
    <div class="tb-spacer" />
    <button class="tb-icon-btn" @click="$emit('pickConnection')" :aria-label="'Switch connection'">
      <Icon name="cluster" :size="16" />
    </button>
    <button class="tb-icon-btn" aria-label="Refresh" @click="health.fetch()">
      <Icon name="reload" :size="16" />
    </button>
    <button class="tb-user" @click="$emit('pickConnection')">
      <span class="tb-avatar">{{ initials }}</span>
      <span class="tb-user-name">{{ activeLabel }}</span>
    </button>
  </div>
</template>

<style scoped>
.topbar {
  grid-area: topbar;
  background: var(--ink);
  color: var(--paper);
  border-bottom: 1px solid #000;
  display: flex;
  align-items: center;
  padding: 0 var(--s-5);
  gap: var(--s-5);
  position: relative;
}
.tb-bg {
  position: absolute;
  inset: 0;
  background: repeating-linear-gradient(
    0deg,
    transparent 0,
    transparent 23px,
    rgba(255, 255, 255, 0.025) 23px,
    rgba(255, 255, 255, 0.025) 24px
  );
  pointer-events: none;
}
.tb-menu {
  position: relative;
  z-index: 1;
  width: 32px;
  height: 32px;
  display: none;
  align-items: center;
  justify-content: center;
  color: var(--ink-ghost);
  border: 1px solid var(--ink-light);
}
.tb-brand {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  position: relative;
  z-index: 1;
}
.tb-logo {
  font-family: var(--font-display);
  font-size: 24px;
  letter-spacing: -0.02em;
  color: var(--paper);
}
.tb-logo-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  background: var(--tally);
  margin-left: 2px;
  vertical-align: 1px;
  box-shadow: 0 0 12px var(--tally-glow);
}
.tb-version {
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--ink-faint);
  padding: 2px 6px;
  border: 1px solid var(--ink-light);
  letter-spacing: 0.05em;
}
.tb-divider {
  width: 1px;
  height: 24px;
  background: var(--ink-light);
  position: relative;
  z-index: 1;
}
.tb-cluster {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-ghost);
  position: relative;
  z-index: 1;
}
.tb-cluster :deep(.tally-label) {
  color: var(--ink-ghost);
}
.tb-spacer {
  flex: 1;
  position: relative;
  z-index: 1;
}
.tb-icon-btn {
  position: relative;
  z-index: 1;
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid transparent;
  color: var(--ink-ghost);
  transition: all 0.15s;
}
.tb-icon-btn:hover {
  border-color: var(--ink-light);
  color: var(--paper);
}
.tb-user {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  gap: var(--s-2);
  padding: 4px 10px 4px 4px;
  border: 1px solid var(--ink-light);
  background: transparent;
  color: var(--paper);
}
.tb-avatar {
  width: 24px;
  height: 24px;
  background: var(--tally);
  color: var(--ink);
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
}
.tb-user-name {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--paper);
}
@media (max-width: 1023px) {
  .tb-menu {
    display: inline-flex;
  }
  .tb-divider,
  .tb-cluster {
    display: none;
  }
}
</style>
