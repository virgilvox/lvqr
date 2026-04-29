<script setup lang="ts">
import { computed, ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import { useStreamsStore } from '@/stores/streams';
import { useConnectionStore } from '@/stores/connection';
import { usePolling } from '@/composables/usePolling';
import { joinUrl } from '@/api/url';
import '@lvqr/dvr-player';

const streams = useStreamsStore();
const conn = useConnectionStore();

usePolling(() => streams.fetch(), { intervalMs: 10_000 });

const selected = ref<string>('');
const baseUrl = computed(() => conn.activeProfile?.baseUrl ?? '');
const streamSrc = computed(() =>
  selected.value && baseUrl.value
    ? joinUrl(baseUrl.value, `/hls/${encodeURIComponent(selected.value)}/master.m3u8`)
    : '',
);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS / DVR">
      <template #title>DVR <em>scrubber.</em></template>
    </PageHeader>

    <Card kicker="CONTROLS" title="Pick a broadcast">
      <select v-model="selected" class="picker">
        <option value="">- select -</option>
        <option v-for="s in streams.streams" :key="s.name" :value="s.name">{{ s.name }}</option>
      </select>
    </Card>

    <Card v-if="streamSrc" kicker="PLAYBACK" :title="selected">
      <lvqr-dvr-player
        :src="streamSrc"
        :token="conn.activeProfile?.bearerToken ?? ''"
        controls="custom"
        thumbnails="enabled"
      />
    </Card>

    <EmptyState
      v-else
      kicker="EMPTY"
      title="Select a broadcast"
    >
      The DVR scrubber renders the live HLS endpoint with a window depth of
      <code>--hls-dvr-window-secs</code>. Authentication uses the active connection profile's
      bearer token via the <code>token</code> attribute.
    </EmptyState>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.picker {
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: 7px 10px;
  font-family: var(--font-mono);
  font-size: 13px;
  width: 100%;
  max-width: 480px;
}
lvqr-dvr-player {
  width: 100%;
  display: block;
  background: var(--ink);
}
code {
  font-family: var(--font-mono);
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
}
</style>
