<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import Badge from '@/components/ui/Badge.vue';
import { useConfigReloadStore } from '@/stores/configReload';
import { useConnectionStore } from '@/stores/connection';
import { useToast } from '@/composables/useToast';
import { usePolling } from '@/composables/usePolling';
import { formatRelativeTime } from '@/api/url';

const cfg = useConfigReloadStore();
const conn = useConnectionStore();
const { push } = useToast();

usePolling(() => cfg.fetch(), { intervalMs: 30_000 });

async function trigger() {
  try {
    await cfg.trigger();
    push('success', 'config reload accepted');
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / SYSTEM / SETTINGS">
      <template #title>Server <em>config.</em></template>
    </PageHeader>

    <Card kicker="CONFIG RELOAD" title="Hot reload">
      <p class="hint">
        When the relay was started with <code>--config &lt;path.toml&gt;</code>, it watches the
        file via SIGHUP / inotify. The button below triggers
        <code>POST /api/v1/config-reload</code>; the response carries the keys the reload
        re-applied (auth providers, mesh ICE, HMAC secret, JWKS URL, webhook URL).
      </p>
      <div class="status">
        <span>config path</span>
        <Badge :variant="cfg.status?.config_path ? 'wire' : 'neutral'">
          {{ cfg.status?.config_path ?? 'no --config' }}
        </Badge>
        <span>last reload</span>
        <Badge :variant="cfg.status?.last_reload_at_ms ? 'ready' : 'neutral'">
          {{ cfg.status?.last_reload_kind ?? 'never' }}
          ({{ formatRelativeTime(cfg.status?.last_reload_at_ms ?? null) }})
        </Badge>
      </div>
      <div class="applied">
        <span>applied keys</span>
        <Badge v-for="k in (cfg.status?.applied_keys ?? [])" :key="k" variant="tally">{{ k }}</Badge>
        <Badge v-if="!(cfg.status?.applied_keys ?? []).length" variant="neutral">none</Badge>
      </div>
      <div v-if="cfg.status?.warnings?.length" class="warnings">
        <strong>Warnings:</strong>
        <ul>
          <li v-for="w in cfg.status.warnings" :key="w">{{ w }}</li>
        </ul>
      </div>
      <Button variant="primary" :disabled="!cfg.status?.config_path" @click="trigger">
        <Icon name="refresh" :size="12" /> Trigger reload
      </Button>
    </Card>

    <Card kicker="CONNECTIONS" title="Active connection">
      <p v-if="conn.activeProfile">
        <strong>{{ conn.activeProfile.label }}</strong>
        <span class="addr">{{ conn.activeProfile.baseUrl }}</span>
      </p>
      <p v-else>No active relay. Use the connection drawer in the topbar to add one.</p>
      <p class="hint">
        Profiles are stored in this browser's localStorage. Use the connection drawer in the
        topbar (cluster icon) to add, edit, or remove relays.
      </p>
    </Card>

    <!-- LVQR v1.x backlog: full GET / PUT of the parsed --config file shape. -->
    <Card kicker="V1.X BACKLOG" title="Full config GET/PUT">
      <p class="hint">
        Reading the resolved <code>--config</code> file via the admin API, or pushing a
        full-file replacement, is on the v1.x backlog. Today operators edit the file directly
        + trigger a reload.
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
.status,
.applied {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--ink-faint);
  margin-bottom: var(--s-3);
}
.warnings {
  background: rgba(217, 119, 6, 0.08);
  border: 1px solid var(--warn);
  padding: var(--s-3);
  margin-bottom: var(--s-3);
  font-size: 13px;
  color: var(--ink-light);
}
.warnings ul {
  list-style: disc;
  padding-left: var(--s-4);
  margin-top: var(--s-2);
}
p {
  font-size: 13px;
  color: var(--ink-muted);
  margin-bottom: var(--s-2);
}
.addr {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
  margin-left: var(--s-2);
}
.hint {
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
}
.hint code {
  background: var(--chalk-lo);
  padding: 1px 5px;
  border: 1px solid var(--chalk-hi);
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
