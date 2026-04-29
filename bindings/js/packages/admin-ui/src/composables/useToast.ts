import { ref } from 'vue';

export type ToastKind = 'info' | 'success' | 'warn' | 'error';

export interface Toast {
  id: string;
  kind: ToastKind;
  message: string;
  ts: number;
}

const toasts = ref<Toast[]>([]);

function newId(): string {
  return `t-${Math.random().toString(36).slice(2, 10)}`;
}

export function useToast() {
  function push(kind: ToastKind, message: string, autoDismissMs = 4000): Toast {
    const t: Toast = { id: newId(), kind, message, ts: Date.now() };
    toasts.value.push(t);
    if (autoDismissMs > 0) {
      window.setTimeout(() => dismiss(t.id), autoDismissMs);
    }
    return t;
  }

  function dismiss(id: string): void {
    toasts.value = toasts.value.filter((t) => t.id !== id);
  }

  return { toasts, push, dismiss };
}
