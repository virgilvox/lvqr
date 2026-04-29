<script setup lang="ts">
import { computed } from 'vue';
import { useRouter, type RouteRecordRaw } from 'vue-router';
import { RAIL_SECTIONS } from '@/router';
import { listPlugins } from '@/plugins';
import Icon from './Icon.vue';

const router = useRouter();

interface RailEntry {
  name: string;
  path: string;
  label: string;
  icon: string;
  rail: string;
}

const entries = computed<RailEntry[]>(() => {
  const out: RailEntry[] = [];
  // built-in routes whose meta.rail is a known section
  for (const route of router.getRoutes() as RouteRecordRaw[]) {
    const meta = (route.meta ?? {}) as Record<string, unknown>;
    if (typeof meta.rail !== 'string' || !meta.rail) continue;
    if (!route.name || typeof route.name !== 'string') continue;
    if (typeof meta.label !== 'string') continue;
    out.push({
      name: route.name,
      path: typeof route.path === 'string' ? route.path : '/',
      label: meta.label,
      icon: typeof meta.icon === 'string' ? meta.icon : 'plugin',
      rail: meta.rail,
    });
  }
  // plugin-registered entries
  for (const plugin of listPlugins()) {
    out.push({
      name: plugin.id,
      path: plugin.path,
      label: plugin.label,
      icon: plugin.icon ?? 'plugin',
      rail: plugin.rail ?? 'system',
    });
  }
  return out;
});

const grouped = computed(() => {
  const map = new Map<string, RailEntry[]>();
  for (const e of entries.value) {
    const list = map.get(e.rail) ?? [];
    list.push(e);
    map.set(e.rail, list);
  }
  return RAIL_SECTIONS.filter((s) => map.has(s.id)).map((s) => ({
    ...s,
    entries: map.get(s.id)!,
  }));
});

defineProps<{ open: boolean }>();
defineEmits<{ close: [] }>();
</script>

<template>
  <aside class="rail" :class="{ 'rail-open': open }">
    <nav class="rail-scroll" aria-label="Primary">
      <section v-for="section in grouped" :key="section.id" class="rail-section">
        <div class="rail-label">{{ section.label }}</div>
        <RouterLink
          v-for="entry in section.entries"
          :key="entry.name"
          :to="entry.path"
          class="rail-item"
          :exact-active-class="'is-active'"
          @click="$emit('close')"
        >
          <Icon :name="entry.icon" :size="14" />
          <span>{{ entry.label }}</span>
        </RouterLink>
      </section>
    </nav>
    <div class="rail-footer">
      Tallyboard <span style="color: var(--tally-deep)">console</span><br />
      <a href="https://github.com/virgilvox/lvqr" target="_blank" rel="noopener">github.com/virgilvox/lvqr</a>
    </div>
  </aside>
</template>

<style scoped>
.rail {
  grid-area: rail;
  background: var(--paper);
  border-right: 1px solid var(--chalk-hi);
  display: flex;
  flex-direction: column;
  overflow-y: auto;
  height: 100%;
}
.rail-scroll {
  flex: 1;
  overflow-y: auto;
}
.rail-section {
  padding: var(--s-4) 0 var(--s-2);
}
.rail-section:not(:first-child) {
  border-top: 1px solid var(--chalk);
}
.rail-label {
  font-family: var(--font-mono);
  font-size: 9px;
  font-weight: 700;
  letter-spacing: 0.2em;
  color: var(--ink-faint);
  padding: 0 var(--s-5) var(--s-2);
  text-transform: uppercase;
}
.rail-item {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  padding: 7px var(--s-5);
  font-size: 13px;
  color: var(--ink-light);
  border-left: 2px solid transparent;
  cursor: pointer;
  transition: all 0.1s;
  font-weight: 500;
}
.rail-item:hover {
  background: var(--chalk-lo);
  color: var(--ink);
}
.rail-item.is-active {
  background: var(--tally-wash);
  color: var(--ink);
  border-left-color: var(--tally);
  font-weight: 600;
}
.rail-item :deep(svg) {
  color: var(--ink-muted);
  flex-shrink: 0;
}
.rail-item.is-active :deep(svg) {
  color: var(--tally-deep);
}
.rail-footer {
  padding: var(--s-4) var(--s-5);
  border-top: 1px solid var(--chalk);
  font-family: var(--font-mono);
  font-size: 10px;
  color: var(--ink-faint);
  line-height: 1.6;
}
.rail-footer a {
  color: var(--wire);
  text-decoration: underline;
  text-underline-offset: 2px;
}

@media (max-width: 1023px) {
  .rail {
    position: fixed;
    inset: var(--topbar-h) auto var(--statusbar-h) 0;
    width: var(--rail-w);
    z-index: 10;
    transform: translateX(-100%);
    transition: transform 0.18s ease;
    box-shadow: 0 0 24px rgba(20, 32, 46, 0.15);
  }
  .rail.rail-open {
    transform: translateX(0);
  }
}
</style>
