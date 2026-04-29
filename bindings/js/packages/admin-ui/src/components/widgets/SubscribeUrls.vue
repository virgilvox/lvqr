<script setup lang="ts">
import { computed } from 'vue';
import { broadcastUrls } from '@/api/protocolUrls';
import { useConnectionStore } from '@/stores/connection';
import CopyableUrl from './CopyableUrl.vue';

const props = defineProps<{
  broadcast: string;
  bearerToken?: string;
}>();

const conn = useConnectionStore();
const urls = computed(() => {
  if (!conn.activeProfile) return null;
  return broadcastUrls(conn.activeProfile, props.broadcast, props.bearerToken);
});
</script>

<template>
  <div class="subs" v-if="urls">
    <p class="hint">
      <strong>LL-HLS</strong> + <strong>DASH</strong> open directly in a
      browser tab (LL-HLS via Safari natively, or VLC / hls.js for the
      others). <strong>MoQ</strong> + <strong>WHEP</strong> +
      <strong>WS fMP4</strong> are programmatic protocols -- pasting
      them into the address bar will fail; use the embed snippets below
      or the <code>@lvqr/core</code> / <code>@lvqr/player</code> SDKs.
    </p>
    <CopyableUrl label="MoQ" accent="wire" :value="urls.subscribe.moq" />
    <CopyableUrl label="WHEP" accent="wire" :value="urls.subscribe.whep" />
    <CopyableUrl label="LL-HLS" accent="wire" :value="urls.subscribe.hls" />
    <CopyableUrl label="DASH" accent="wire" :value="urls.subscribe.dash" />
    <CopyableUrl label="WS fMP4" accent="wire" :value="urls.subscribe.ws" />
    <CopyableUrl label="EMBED" :value="urls.embed.lvqrPlayer" />
    <CopyableUrl label="DVR EMBED" :value="urls.embed.lvqrDvrPlayer" />
  </div>
  <p v-else class="hint">No active connection -- pick a relay first.</p>
</template>

<style scoped>
.subs {
  display: flex;
  flex-direction: column;
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  padding: var(--s-3);
}
</style>
