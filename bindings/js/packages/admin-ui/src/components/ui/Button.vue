<script setup lang="ts">
defineProps<{
  /** Visual variant. Default 'default'. */
  variant?: 'default' | 'primary' | 'wire' | 'ghost' | 'danger';
  /** Smaller compact variant. */
  small?: boolean;
  /** Disable the button. */
  disabled?: boolean;
  /** Set when the button kicks off a long action. */
  loading?: boolean;
  /** Pass-through type. */
  type?: 'button' | 'submit' | 'reset';
}>();
</script>

<template>
  <button
    :class="[
      'btn',
      `btn-${variant ?? 'default'}`,
      { 'btn-sm': small, 'is-loading': loading },
    ]"
    :disabled="disabled || loading"
    :type="type ?? 'button'"
  >
    <slot />
  </button>
</template>

<style scoped>
.btn {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  padding: 7px 14px;
  font-family: var(--font-mono);
  font-size: 11px;
  font-weight: 600;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  border: 1px solid var(--ink);
  background: var(--paper);
  color: var(--ink);
  transition: all 0.1s;
  cursor: pointer;
}
.btn:hover:not(:disabled) {
  background: var(--ink);
  color: var(--paper);
}
.btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}
.btn-primary {
  background: var(--tally);
  border-color: var(--tally-deep);
  color: var(--paper);
}
.btn-primary:hover:not(:disabled) {
  background: var(--tally-deep);
  border-color: var(--ink);
}
.btn-wire {
  background: var(--wire);
  border-color: var(--wire-deep);
  color: var(--paper);
}
.btn-wire:hover:not(:disabled) {
  background: var(--wire-deep);
}
.btn-ghost {
  background: transparent;
  border-color: var(--ink-faint);
  color: var(--ink-muted);
}
.btn-ghost:hover:not(:disabled) {
  background: var(--ink);
  color: var(--paper);
  border-color: var(--ink);
}
.btn-danger {
  background: transparent;
  border-color: var(--on-air);
  color: var(--on-air);
}
.btn-danger:hover:not(:disabled) {
  background: var(--on-air);
  color: var(--paper);
}
.btn-sm {
  padding: 4px 10px;
  font-size: 10px;
}
.btn :deep(svg) {
  width: 12px;
  height: 12px;
}
.is-loading {
  position: relative;
  cursor: wait;
}
</style>
