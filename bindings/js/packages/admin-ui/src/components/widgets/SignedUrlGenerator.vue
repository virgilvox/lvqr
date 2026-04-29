<script setup lang="ts">
import { computed, ref } from 'vue';
import { useConnectionStore } from '@/stores/connection';
import { signLiveUrl, signPlaybackUrl } from '@/composables/useHmacSign';
import { profileHost, profileScheme, DEFAULT_PROTOCOL_PORTS } from '@/api/protocolUrls';
import Button from '@/components/ui/Button.vue';
import CopyableUrl from './CopyableUrl.vue';
import { useToast } from '@/composables/useToast';

const conn = useConnectionStore();
const { push } = useToast();

const scheme = ref<'playback' | 'hls' | 'dash'>('hls');
const broadcast = ref('live/demo');
const path = ref('/playback/live/demo/master.m3u8');
const expirySeconds = ref(3600);
const secret = ref('');
const generated = ref<string | null>(null);

const expUnix = computed(() => Math.floor(Date.now() / 1000) + Math.max(60, expirySeconds.value));

async function generate() {
  if (!secret.value.trim()) {
    push('error', 'paste the relay\'s --hmac-playback-secret to sign');
    return;
  }
  if (!conn.activeProfile) {
    push('error', 'no active connection');
    return;
  }
  try {
    if (scheme.value === 'playback') {
      const base = conn.activeProfile.baseUrl;
      generated.value = await signPlaybackUrl(base, path.value, expUnix.value, secret.value);
    } else {
      const host = profileHost(conn.activeProfile);
      const httpScheme = profileScheme(conn.activeProfile) === 'https:' ? 'https' : 'http';
      const profileWithPorts = conn.activeProfile as { hlsPort?: number; dashPort?: number };
      const port = scheme.value === 'hls'
        ? profileWithPorts.hlsPort ?? DEFAULT_PROTOCOL_PORTS.hls
        : profileWithPorts.dashPort ?? DEFAULT_PROTOCOL_PORTS.dash;
      const base = `${httpScheme}://${host}:${port}`;
      generated.value = await signLiveUrl(base, scheme.value, broadcast.value, expUnix.value, secret.value);
    }
    push('success', 'signed URL generated');
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}
</script>

<template>
  <div class="sig">
    <p class="hint">
      Pure browser-side HMAC-SHA256 signing. Mirrors
      <code>lvqr_cli::sign_playback_url</code> /
      <code>sign_live_url</code> exactly. The secret never leaves this
      browser.
    </p>
    <div class="grid">
      <label>
        <span>Scheme</span>
        <select v-model="scheme">
          <option value="hls">live HLS</option>
          <option value="dash">live DASH</option>
          <option value="playback">playback / DVR</option>
        </select>
      </label>
      <label v-if="scheme !== 'playback'">
        <span>Broadcast</span>
        <input v-model="broadcast" placeholder="live/demo" />
      </label>
      <label v-else>
        <span>Path</span>
        <input v-model="path" placeholder="/playback/live/demo/master.m3u8" />
      </label>
      <label>
        <span>Expires in (seconds)</span>
        <input v-model.number="expirySeconds" type="number" min="60" />
      </label>
      <label>
        <span>HMAC secret</span>
        <input v-model="secret" type="password" placeholder="--hmac-playback-secret" />
      </label>
    </div>
    <div class="actions">
      <Button variant="primary" @click="generate">Generate signed URL</Button>
    </div>
    <div v-if="generated" class="result">
      <CopyableUrl label="SIGNED" accent="tally" :value="generated" />
    </div>
  </div>
</template>

<style scoped>
.sig {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  line-height: 1.65;
}
.hint code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--s-3);
}
.grid label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.grid span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.grid input,
.grid select {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 7px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.actions {
  display: flex;
  justify-content: flex-end;
}
.result {
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-2) var(--s-3);
}
</style>
