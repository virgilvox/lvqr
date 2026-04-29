<script setup lang="ts">
import { ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Badge from '@/components/ui/Badge.vue';
import { useConnectionStore } from '@/stores/connection';
import { joinUrl } from '@/api/url';

const conn = useConnectionStore();
const broadcast = ref('');
const result = ref<{ valid: boolean; state?: string; signer?: string; raw?: unknown } | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);

async function verify() {
  if (!conn.activeProfile) return;
  loading.value = true;
  error.value = null;
  result.value = null;
  try {
    const url = joinUrl(conn.activeProfile.baseUrl, `/playback/verify/${encodeURIComponent(broadcast.value)}`);
    const headers: Record<string, string> = {};
    if (conn.activeProfile.bearerToken) {
      headers['Authorization'] = `Bearer ${conn.activeProfile.bearerToken}`;
    }
    const resp = await fetch(url, { headers });
    if (!resp.ok) {
      error.value = `HTTP ${resp.status} ${resp.statusText}`;
      return;
    }
    const body = (await resp.json()) as { valid: boolean; state?: string; signer?: string };
    result.value = { ...body, raw: body };
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e);
  } finally {
    loading.value = false;
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / IDENTITY / PROVENANCE">
      <template #title>Content <em>provenance.</em></template>
    </PageHeader>

    <Card kicker="C2PA" title="Verify a recorded broadcast">
      <p class="hint">
        Calls <code>GET /playback/verify/&lt;broadcast&gt;</code>; the relay re-validates the
        archived asset's C2PA manifest using its configured trust anchor and reports
        <strong>valid</strong> + <strong>state</strong> + <strong>signer</strong>.
      </p>
      <form @submit.prevent="verify" class="verify-form">
        <label>
          <span>Broadcast</span>
          <input v-model="broadcast" placeholder="live/demo" required />
        </label>
        <Button variant="primary" type="submit" :loading="loading">Verify</Button>
      </form>

      <div v-if="result" class="result">
        <Badge :variant="result.valid ? 'ready' : 'on-air'">
          {{ result.valid ? 'valid' : 'invalid' }}
        </Badge>
        <span v-if="result.state">state: <strong>{{ result.state }}</strong></span>
        <span v-if="result.signer">signer: <strong>{{ result.signer }}</strong></span>
      </div>

      <p v-if="error" class="err">{{ error }}</p>
    </Card>

    <Card kicker="SIGNING" title="Configuration">
      <p class="hint">
        C2PA signing is configured at process startup with
        <code>--c2pa-signing-cert</code> + <code>--c2pa-signing-key</code> +
        <code>--c2pa-signing-alg</code> + <code>--c2pa-trust-anchor</code> +
        <code>--c2pa-timestamp-authority</code> (or the <code>[c2pa]</code> block in
        <code>--config</code>). The relay signs at fragment finalization; verification is
        re-runnable at any time via the verify route above.
      </p>
    </Card>
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
.verify-form {
  display: flex;
  gap: var(--s-3);
  align-items: end;
  flex-wrap: wrap;
  margin-bottom: var(--s-3);
}
.verify-form label {
  display: flex;
  flex-direction: column;
  gap: 4px;
  flex: 1;
  min-width: 240px;
}
.verify-form span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.verify-form input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 7px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.result {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-muted);
  padding: var(--s-3);
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
}
.result strong {
  color: var(--ink);
}
.err {
  color: var(--on-air);
  font-family: var(--font-mono);
  font-size: 12px;
  padding: var(--s-2);
  border: 1px solid var(--on-air);
  background: rgba(220, 38, 38, 0.06);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-bottom: var(--s-3);
}
.hint code {
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
