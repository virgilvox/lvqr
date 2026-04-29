<script setup lang="ts">
import { computed } from 'vue';
import { broadcastUrls } from '@/api/protocolUrls';
import { useConnectionStore } from '@/stores/connection';
import CopyableUrl from './CopyableUrl.vue';

const props = defineProps<{
  /** Broadcast name (e.g. `live/cam-a`). */
  broadcast: string;
  /** Optional bearer / stream-key token to substitute into auth-bearing URLs. */
  bearerToken?: string;
}>();

const conn = useConnectionStore();
const urls = computed(() => {
  if (!conn.activeProfile) return null;
  return broadcastUrls(conn.activeProfile, props.broadcast, props.bearerToken);
});
</script>

<template>
  <div class="recipes" v-if="urls">
    <p class="hint">
      Auth-bearing URLs substitute the bearer token where the protocol expects
      it (RTMP key segment, SRT <code>streamid</code>). WHIP / WHEP carry the
      token in the <code>Authorization: Bearer</code> header at request time.
    </p>
    <CopyableUrl label="RTMP" accent="tally" :value="urls.publish.rtmp" />
    <CopyableUrl label="WHIP" accent="tally" :value="urls.publish.whip" />
    <CopyableUrl label="SRT" accent="tally" :value="urls.publish.srt" />
    <CopyableUrl label="RTSP" accent="tally" :value="urls.publish.rtsp" />
  </div>
  <p v-else class="hint">No active connection -- pick a relay first.</p>
</template>

<style scoped>
.recipes {
  display: flex;
  flex-direction: column;
  gap: 0;
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-bottom: var(--s-2);
  line-height: 1.65;
}
.hint code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
</style>
