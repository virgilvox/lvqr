import { onBeforeUnmount, onMounted, ref } from 'vue';

export interface PollingOptions {
  /** Interval in ms between polls. Default 5000. */
  intervalMs?: number;
  /** Whether to fire immediately on mount in addition to the interval. Default true. */
  immediate?: boolean;
}

export interface PollingHandle {
  /** True while a poll is in flight. */
  loading: ReturnType<typeof ref<boolean>>;
  /** Most-recent error from the polled task, or null. */
  error: ReturnType<typeof ref<Error | null>>;
  /** Force a refresh outside the interval. */
  refresh: () => Promise<void>;
  /** Stop the interval (call manually if not using lifecycle wiring). */
  stop: () => void;
}

/**
 * Poll an async task on an interval, auto-stopping on component unmount.
 * Errors are swallowed into the returned `error` ref so a transient relay
 * blip does not crash the view; the most-recent successful result remains
 * the source of truth for the consumer's display state.
 */
export function usePolling(task: () => Promise<void>, options: PollingOptions = {}): PollingHandle {
  const interval = options.intervalMs ?? 5_000;
  const immediate = options.immediate ?? true;
  const loading = ref(false);
  const error = ref<Error | null>(null);
  let timer: number | undefined;

  async function refresh(): Promise<void> {
    if (loading.value) return;
    loading.value = true;
    error.value = null;
    try {
      await task();
    } catch (e) {
      error.value = e instanceof Error ? e : new Error(String(e));
    } finally {
      loading.value = false;
    }
  }

  function start() {
    if (timer !== undefined) return;
    timer = window.setInterval(() => {
      void refresh();
    }, interval);
  }

  function stop() {
    if (timer !== undefined) {
      window.clearInterval(timer);
      timer = undefined;
    }
  }

  onMounted(() => {
    if (immediate) void refresh();
    start();
  });
  onBeforeUnmount(stop);

  return { loading, error, refresh, stop };
}
