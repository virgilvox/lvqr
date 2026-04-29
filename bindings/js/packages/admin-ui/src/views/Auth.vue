<script setup lang="ts">
import { ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import Badge from '@/components/ui/Badge.vue';
import StreamKeyTable from '@/components/widgets/StreamKeyTable.vue';
import SignedUrlGenerator from '@/components/widgets/SignedUrlGenerator.vue';
import { useStreamKeysStore } from '@/stores/streamkeys';
import { useConfigReloadStore } from '@/stores/configReload';
import { useToast } from '@/composables/useToast';
import { usePolling } from '@/composables/usePolling';

const sk = useStreamKeysStore();
const cfg = useConfigReloadStore();
const { push } = useToast();

usePolling(() => sk.fetch(), { intervalMs: 30_000 });
usePolling(() => cfg.fetch(), { intervalMs: 30_000 });

const newLabel = ref('');
const newBroadcast = ref('');
const newTtl = ref<number | null>(null);

async function mint() {
  try {
    const k = await sk.mint({
      label: newLabel.value || undefined,
      broadcast: newBroadcast.value || undefined,
      ttl_seconds: newTtl.value ?? undefined,
    });
    push('success', `minted ${k.id}`);
    newLabel.value = '';
    newBroadcast.value = '';
    newTtl.value = null;
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}

async function rotate(id: string) {
  try {
    const k = await sk.rotate(id);
    push('success', `rotated ${k.id}; copy the new token`);
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}

async function revoke(id: string) {
  if (!confirm(`Revoke stream key ${id}?`)) return;
  try {
    await sk.revoke(id);
    push('success', `revoked ${id}`);
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / IDENTITY / AUTH">
      <template #title>Identity <em>plane.</em></template>
    </PageHeader>

    <Card kicker="STREAM KEYS" title="Runtime CRUD">
      <form class="mint-form" @submit.prevent="mint">
        <label>
          <span>Label</span>
          <input v-model="newLabel" placeholder="camera-a" />
        </label>
        <label>
          <span>Broadcast (optional)</span>
          <input v-model="newBroadcast" placeholder="live/cam-a" />
        </label>
        <label>
          <span>TTL seconds (optional)</span>
          <input v-model.number="newTtl" type="number" min="0" placeholder="3600" />
        </label>
        <Button variant="primary" type="submit"><Icon name="plus" :size="12" /> Mint</Button>
      </form>

      <StreamKeyTable
        :keys="sk.keys"
        @rotate="rotate"
        @revoke="revoke"
        @copy-token="push('success', 'token copied')"
      />
    </Card>

    <Card kicker="SIGNED URLS" title="HMAC playback / live URL generator" wire>
      <SignedUrlGenerator />
    </Card>

    <Card kicker="PROVIDERS" title="Authentication providers (read-only)">
      <p class="hint">
        JWT, JWKS, and webhook providers are configured at process startup with
        <code>--jwt-secret</code> / <code>--jwks-url</code> / <code>--webhook-auth-url</code>
        (or the equivalent <code>[auth]</code> section of <code>--config</code>). The block
        below is the live config-reload status.
      </p>
      <ul class="providers" v-if="cfg.status">
        <li>
          config path
          <Badge :variant="cfg.status.config_path ? 'wire' : 'neutral'">
            {{ cfg.status.config_path ?? 'no --config' }}
          </Badge>
        </li>
        <li>
          last reload
          <Badge :variant="cfg.status.last_reload_at_ms ? 'ready' : 'neutral'">
            {{ cfg.status.last_reload_kind ?? 'never' }}
          </Badge>
        </li>
        <li>
          applied keys
          <Badge v-for="k in (cfg.status.applied_keys ?? [])" :key="k" variant="tally">{{ k }}</Badge>
          <Badge v-if="!(cfg.status.applied_keys ?? []).length" variant="neutral">none</Badge>
        </li>
      </ul>
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
.mint-form {
  display: grid;
  grid-template-columns: repeat(3, 1fr) auto;
  gap: var(--s-3);
  align-items: end;
  margin-bottom: var(--s-4);
  padding: var(--s-3);
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
}
.mint-form label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.mint-form span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.mint-form input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 6px 10px;
  font-size: 13px;
  font-family: var(--font-mono);
}
.providers {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-muted);
}
.providers li {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex-wrap: wrap;
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
  .mint-form {
    grid-template-columns: 1fr;
  }
}
</style>
