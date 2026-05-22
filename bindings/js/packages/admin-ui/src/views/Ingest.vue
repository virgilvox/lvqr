<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import Badge from '@/components/ui/Badge.vue';
import StreamRow from '@/components/widgets/StreamRow.vue';
import { useStreamsStore } from '@/stores/streams';
import { useServerInfoStore } from '@/stores/serverInfo';
import { useConnectionStore } from '@/stores/connection';
import { usePolling } from '@/composables/usePolling';

const streams = useStreamsStore();
const server = useServerInfoStore();
const conn = useConnectionStore();

usePolling(() => streams.fetch(), { intervalMs: 10_000 });
usePolling(() => server.fetch(), { intervalMs: 30_000 });

// Real ingest-listener inventory from `/api/v1/server-info` bound addresses.
// `null` bound address = that listener is not enabled on this relay.
const INGEST_PROTOCOLS = [
  { key: 'rtmp', label: 'RTMP' },
  { key: 'srt', label: 'SRT' },
  { key: 'rtsp', label: 'RTSP' },
  { key: 'whip', label: 'WHIP' },
] as const;
const listeners = computed(() => {
  const bound = (server.info?.bound ?? {}) as Record<string, string | null | undefined>;
  return INGEST_PROTOCOLS.map((p) => ({ label: p.label, addr: bound[p.key] ?? null }));
});
const serverVersion = computed(() => server.info?.version ?? null);

const host = computed(() => {
  try {
    if (!conn.activeProfile) return '<relay-host>';
    return new URL(conn.activeProfile.baseUrl).hostname;
  } catch {
    return '<relay-host>';
  }
});

const recipes = computed(() => [
  { protocol: 'RTMP', port: 1935, example: `rtmp://${host.value}:1935/live/<key>` },
  { protocol: 'WHIP', port: 8443, example: `https://${host.value}:8443/whip/<broadcast>` },
  { protocol: 'SRT', port: 9000, example: `srt://${host.value}:9000?streamid=publish:<broadcast>` },
  { protocol: 'RTSP', port: 8554, example: `rtsp://${host.value}:8554/<broadcast>` },
]);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / INGEST">
      <template #title>Ingest <em>endpoints.</em></template>
      <template #actions>
        <RouterLink to="/stream-test">
          <Button variant="primary"><Icon name="rec" :size="12" /> Test stream from browser</Button>
        </RouterLink>
        <span class="hint">configured via <code>lvqr serve</code></span>
      </template>
    </PageHeader>

    <Card kicker="LISTENERS" title="Ingest listeners">
      <template #actions>
        <Badge v-if="serverVersion" variant="neutral">lvqr {{ serverVersion }}</Badge>
      </template>
      <div class="ltable" role="table" aria-label="Ingest listeners">
        <div class="tr th" role="row">
          <span role="columnheader">Protocol</span>
          <span role="columnheader">Bound address</span>
          <span role="columnheader">Status</span>
        </div>
        <div v-for="l in listeners" :key="l.label" class="tr" role="row">
          <span role="cell" class="proto">{{ l.label }}</span>
          <span role="cell" class="mono">{{ l.addr ?? '--' }}</span>
          <span role="cell"><Badge :variant="l.addr ? 'ready' : 'neutral'">{{ l.addr ? 'listening' : 'disabled' }}</Badge></span>
        </div>
      </div>
      <p v-if="server.error" class="hint" style="margin-top: var(--s-3)">
        server-info unavailable: {{ server.error }}
      </p>
    </Card>

    <Card kicker="ENDPOINTS" title="Publisher recipes">
      <div class="recipe-grid">
        <article v-for="r in recipes" :key="r.protocol" class="recipe">
          <header>{{ r.protocol }}<span>:{{ r.port }}</span></header>
          <code>{{ r.example }}</code>
        </article>
      </div>
      <p class="hint" style="margin-top: var(--s-3)">
        Authentication is uniform across protocols: a JWT carrier in the protocol-native field
        (RTMP stream key, WHIP <code>Authorization: Bearer</code>, SRT <code>streamid</code>,
        RTSP digest), enforced server-side via <code>--jwt-secret</code> /
        <code>--jwks-url</code>. See the <RouterLink to="/auth">Auth view</RouterLink>.
      </p>
    </Card>

    <Card kicker="LIVE" title="Active publishers">
      <div class="streams-list">
        <StreamRow v-for="s in streams.streams" :key="s.name" :stream="s" />
        <p v-if="!streams.streams.length" class="empty">
          No active publishers yet.
        </p>
      </div>
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
.recipe-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
  gap: var(--s-3);
}
.recipe {
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-3);
}
.recipe header {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--tally-deep);
  font-weight: 700;
  margin-bottom: 4px;
}
.recipe header span {
  color: var(--ink-faint);
  margin-left: 4px;
  font-weight: 400;
}
.recipe code {
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink);
  word-break: break-all;
}
.ltable {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.ltable .tr {
  display: grid;
  grid-template-columns: 1fr 2fr 1fr;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.ltable .tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.ltable .proto {
  font-family: var(--font-mono);
  font-weight: 700;
  color: var(--tally-deep);
}
.ltable .mono {
  font-family: var(--font-mono);
  color: var(--ink-muted);
}
.streams-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
  padding: var(--s-3);
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
@media (max-width: 1023px) {
  .page {
    padding: var(--s-5);
  }
}
</style>
