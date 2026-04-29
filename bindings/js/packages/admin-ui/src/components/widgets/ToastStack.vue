<script setup lang="ts">
import { useToast } from '@/composables/useToast';

const { toasts, dismiss } = useToast();
</script>

<template>
  <div class="toast-stack" aria-live="polite">
    <TransitionGroup name="toast">
      <div
        v-for="t in toasts"
        :key="t.id"
        class="toast"
        :class="`toast-${t.kind}`"
        @click="dismiss(t.id)"
      >
        <span class="toast-msg">{{ t.message }}</span>
        <span class="toast-dismiss" aria-label="Dismiss">&times;</span>
      </div>
    </TransitionGroup>
  </div>
</template>

<style scoped>
.toast-stack {
  position: fixed;
  right: var(--s-5);
  bottom: calc(var(--statusbar-h) + var(--s-4));
  z-index: 50;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.toast {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: 8px 12px;
  font-size: 13px;
  cursor: pointer;
  min-width: 240px;
  max-width: 360px;
  border-left-width: 3px;
}
.toast-info {
  border-left-color: var(--ink-muted);
}
.toast-success {
  border-left-color: var(--ready);
}
.toast-warn {
  border-left-color: var(--warn);
}
.toast-error {
  border-left-color: var(--on-air);
}
.toast-msg {
  flex: 1;
}
.toast-dismiss {
  color: var(--ink-faint);
  font-size: 18px;
  line-height: 1;
}

.toast-enter-active,
.toast-leave-active {
  transition: all 0.15s ease;
}
.toast-enter-from {
  transform: translateX(20px);
  opacity: 0;
}
.toast-leave-to {
  transform: translateX(20px);
  opacity: 0;
}
</style>
