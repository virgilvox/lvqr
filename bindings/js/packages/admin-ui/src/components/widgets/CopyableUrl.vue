<script setup lang="ts">
import { ref } from 'vue';
import Icon from '@/components/ui/Icon.vue';

defineProps<{
  /** Pretty label rendered to the left of the value. */
  label: string;
  /** The URL or string to display + copy. */
  value: string;
  /** Optional accent variant for the label pill (defaults to `ink`). */
  accent?: 'ink' | 'tally' | 'wire';
  /** Treat the value as a code block (preserves whitespace). Default true. */
  monospace?: boolean;
}>();

const justCopied = ref(false);
const emit = defineEmits<{ copied: [value: string] }>();

async function copy(value: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(value);
    justCopied.value = true;
    emit('copied', value);
    window.setTimeout(() => {
      justCopied.value = false;
    }, 1500);
  } catch {
    // ignore -- some browsers gate clipboard on a user gesture; the value
    // is still selectable in the rendered code block.
  }
}
</script>

<template>
  <div class="copy-row">
    <span class="copy-label" :class="`copy-label-${accent ?? 'ink'}`">{{ label }}</span>
    <code v-if="(monospace ?? true)" class="copy-value mono">{{ value }}</code>
    <span v-else class="copy-value">{{ value }}</span>
    <button class="copy-btn" :class="{ copied: justCopied }" @click="copy(value)" :title="`Copy ${label}`">
      <Icon :name="justCopied ? 'check' : 'copy'" :size="12" />
      <span>{{ justCopied ? 'copied' : 'copy' }}</span>
    </button>
  </div>
</template>

<style scoped>
.copy-row {
  display: grid;
  grid-template-columns: 90px 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 7px 0;
  border-bottom: 1px solid var(--chalk);
}
.copy-row:last-child {
  border-bottom: none;
}
.copy-label {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  text-align: center;
  padding: 2px 6px;
  border: 1px solid;
}
.copy-label-ink {
  border-color: var(--chalk-hi);
  color: var(--ink-muted);
  background: var(--chalk-lo);
}
.copy-label-tally {
  border-color: var(--tally-deep);
  color: var(--tally-deep);
  background: var(--tally-wash);
}
.copy-label-wire {
  border-color: var(--wire-deep);
  color: var(--wire-deep);
  background: var(--wire-wash);
}
.copy-value {
  font-size: 12px;
  color: var(--ink);
  word-break: break-all;
  overflow-x: auto;
}
.copy-value.mono {
  font-family: var(--font-mono);
  font-size: 11px;
}
.copy-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  padding: 4px 8px;
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  color: var(--ink-muted);
  cursor: pointer;
  transition: all 0.1s;
}
.copy-btn:hover {
  background: var(--ink);
  color: var(--paper);
  border-color: var(--ink);
}
.copy-btn.copied {
  background: var(--ready);
  color: var(--paper);
  border-color: var(--ready);
}
</style>
