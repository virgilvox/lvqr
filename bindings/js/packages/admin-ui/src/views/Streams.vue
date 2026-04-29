<script setup lang="ts">
import { computed, ref } from 'vue';
import { useRouter } from 'vue-router';
import PageHeader from '@/components/ui/PageHeader.vue';
import StreamRow from '@/components/widgets/StreamRow.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import { useStreamsStore } from '@/stores/streams';
import { useStreamKeysStore } from '@/stores/streamkeys';
import { useToast } from '@/composables/useToast';
import { usePolling } from '@/composables/usePolling';

const streams = useStreamsStore();
const sk = useStreamKeysStore();
const router = useRouter();
const { push } = useToast();

usePolling(() => streams.fetch(), { intervalMs: 5_000 });

const query = ref('');
const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return streams.streams;
  return streams.streams.filter((s) => s.name.toLowerCase().includes(q));
});

// New-broadcast modal state.
const newOpen = ref(false);
const newName = ref('live/demo');
const mintKey = ref(true);
const newSubmitting = ref(false);

async function submitNew() {
  if (!newName.value.trim()) {
    push('error', 'broadcast name required');
    return;
  }
  newSubmitting.value = true;
  let token: string | undefined;
  try {
    if (mintKey.value) {
      const k = await sk.mint({ label: `streams-${newName.value}`, broadcast: newName.value });
      token = k.token;
      push('success', `minted stream key for ${newName.value}`);
    }
    void router.push({
      path: '/stream-test',
      query: {
        broadcast: newName.value,
        ...(token ? { token } : {}),
      },
    });
    newOpen.value = false;
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  } finally {
    newSubmitting.value = false;
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS / LIVE STREAMS">
      <template #title>Streams <em>on the wire.</em></template>
      <template #actions>
        <div class="search">
          <Icon name="search" :size="12" />
          <input v-model="query" placeholder="filter by name..." />
        </div>
        <Button variant="ghost" @click="streams.fetch()"><Icon name="reload" :size="12" /> Reload</Button>
        <Button variant="primary" @click="newOpen = true"><Icon name="plus" :size="12" /> New broadcast</Button>
      </template>
    </PageHeader>

    <div class="rows">
      <StreamRow v-for="s in filtered" :key="s.name" :stream="s" />
      <p v-if="!filtered.length" class="empty">
        {{ streams.streams.length ? 'No streams match the filter.' : 'No active streams.' }}
      </p>
    </div>

    <Teleport to="body">
      <div v-if="newOpen" class="modal-overlay" @click.self="newOpen = false">
        <div class="modal" role="dialog" aria-label="New broadcast">
          <header>
            <div class="kicker">NEW BROADCAST</div>
            <h2>Test or publish</h2>
          </header>
          <p class="hint">
            Reserve a broadcast name and (optionally) mint a stream key. Then jump to the
            in-browser publisher to test it -- or hand the publish URL to OBS / ffmpeg.
          </p>
          <form @submit.prevent="submitNew" class="form">
            <label>
              <span>Broadcast name</span>
              <input v-model="newName" placeholder="live/demo" required autofocus />
            </label>
            <label class="check">
              <input type="checkbox" v-model="mintKey" />
              <span>mint a stream key for this broadcast (recommended)</span>
            </label>
            <div class="actions">
              <Button variant="ghost" type="button" @click="newOpen = false">Cancel</Button>
              <Button variant="primary" type="submit" :loading="newSubmitting">
                Continue to publisher
              </Button>
            </div>
          </form>
        </div>
      </div>
    </Teleport>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
}
.search {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: 4px 10px;
  font-family: var(--font-mono);
  font-size: 12px;
}
.search input {
  border: none;
  outline: none;
  background: transparent;
  width: 240px;
}
.rows {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.empty {
  padding: var(--s-5);
  text-align: center;
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(20, 32, 46, 0.45);
  z-index: 50;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-5);
}
.modal {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  width: min(480px, 100%);
  padding: var(--s-5);
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.modal header h2 {
  font-family: var(--font-display);
  font-size: 28px;
  line-height: 1;
  letter-spacing: -0.01em;
}
.kicker {
  font-family: var(--font-mono);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--ink-faint);
  margin-bottom: 4px;
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  line-height: 1.6;
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.form label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.form > label > span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.form input[type='text'],
.form input:not([type='checkbox']) {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 7px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.form .check {
  flex-direction: row;
  align-items: center;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-light);
}
.actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
  .search input {
    width: 160px;
  }
}
</style>
