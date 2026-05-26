<script setup lang="ts">
import { computed, ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import Badge from '@/components/ui/Badge.vue';
import StreamRow from '@/components/widgets/StreamRow.vue';
import { useStreamsStore } from '@/stores/streams';
import { useServerInfoStore } from '@/stores/serverInfo';
import { useIngestStore } from '@/stores/ingest';
import { useBroadcastsStore } from '@/stores/broadcasts';
import { useConnectionStore } from '@/stores/connection';
import { usePolling } from '@/composables/usePolling';
import { useToast } from '@/composables/useToast';

const streams = useStreamsStore();
const server = useServerInfoStore();
const ingest = useIngestStore();
const broadcasts = useBroadcastsStore();
const conn = useConnectionStore();
const { push: pushToast } = useToast();

usePolling(() => streams.fetch(), { intervalMs: 10_000 });
usePolling(() => server.fetch(), { intervalMs: 30_000 });
// 5s poll: a stop click already refreshes, but the tick also catches
// operator-driven changes from another browser / curl.
usePolling(() => ingest.fetch(), { intervalMs: 5_000 });
// 3s poll for live publisher sessions: faster than the listener tick because
// publishers come and go on a per-session timescale.
usePolling(() => broadcasts.fetch(), { intervalMs: 3_000 });

// Stable display order across protocols (the registry is a hashmap so its
// iteration order is not deterministic).
const PROTOCOL_ORDER: Record<string, number> = { rtmp: 0, whip: 1, srt: 2, rtsp: 3 };
const labelFor = (p: string) => p.toUpperCase();

// Live listener inventory from `/api/v1/ingest` -- richer than server-info's
// static bound addresses because each row carries its current `enabled`
// state (flipped to false by `DELETE /api/v1/ingest/{protocol}`). The row
// stays in the list after stop so the UI shows "rtmp stopped" rather than
// the listener silently vanishing.
const listeners = computed(() => {
  const live = ingest.state?.listeners ?? [];
  return [...live].sort((a, b) => (PROTOCOL_ORDER[a.protocol] ?? 99) - (PROTOCOL_ORDER[b.protocol] ?? 99));
});
const serverVersion = computed(() => server.info?.version ?? null);

// Format an ms-epoch timestamp as a session uptime string ("2m 13s").
function uptimeFor(startedMs: number): string {
  const secs = Math.max(0, Math.floor((Date.now() - startedMs) / 1000));
  if (secs < 60) return `${secs}s`;
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}m ${s}s`;
}

const pendingKill = ref<string | null>(null);
async function killBroadcast(name: string, protocol: string): Promise<void> {
  if (pendingKill.value) return;
  // eslint-disable-next-line no-alert
  if (
    !window.confirm(
      `Kick the live ${protocol.toUpperCase()} publisher of "${name}"? The publisher's socket is closed; they can reconnect immediately.`,
    )
  ) {
    return;
  }
  pendingKill.value = name;
  try {
    await broadcasts.stopBroadcast(name);
    pushToast('info', `Publisher session for "${name}" killed.`, 4000);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    pushToast('error', `Kill "${name}" failed: ${msg}`, 6000);
  } finally {
    pendingKill.value = null;
  }
}

const pendingStop = ref<string | null>(null);
async function stopListener(protocol: string): Promise<void> {
  if (pendingStop.value) return;
  // eslint-disable-next-line no-alert
  if (!window.confirm(`Stop the ${labelFor(protocol)} listener? This is one-way until the relay restarts.`)) {
    return;
  }
  pendingStop.value = protocol;
  try {
    await ingest.stopListener(protocol);
    pushToast('info', `${labelFor(protocol)} listener stopped.`, 4000);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    pushToast('error', `Stop ${labelFor(protocol)} failed: ${msg}`, 6000);
  } finally {
    pendingStop.value = null;
  }
}

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
          <span role="columnheader"><span class="vh">Actions</span></span>
        </div>
        <div v-if="!listeners.length" class="tr empty-row" role="row">
          <span role="cell" colspan="4" class="hint">
            No ingest listeners bound on this relay.
          </span>
        </div>
        <div v-for="l in listeners" :key="l.protocol" class="tr" role="row">
          <span role="cell" class="proto">{{ labelFor(l.protocol) }}</span>
          <span role="cell" class="mono">{{ l.addr }}</span>
          <span role="cell">
            <Badge :variant="l.enabled ? 'ready' : 'neutral'">
              {{ l.enabled ? 'listening' : 'stopped' }}
            </Badge>
          </span>
          <span role="cell" class="action">
            <Button
              v-if="l.enabled"
              variant="danger"
              :disabled="pendingStop === l.protocol"
              :aria-label="`Stop ${labelFor(l.protocol)} listener`"
              @click="stopListener(l.protocol)"
            >
              {{ pendingStop === l.protocol ? 'Stopping...' : 'Stop' }}
            </Button>
            <span v-else class="hint mono">requires relay restart</span>
          </span>
        </div>
      </div>
      <p v-if="ingest.error" class="hint" style="margin-top: var(--s-3)">
        ingest registry unavailable: {{ ingest.error }}
      </p>
      <p v-else-if="server.error" class="hint" style="margin-top: var(--s-3)">
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

    <Card kicker="LIVE" title="Live publisher sessions">
      <p class="hint" style="margin-bottom: var(--s-2)">
        Per-publisher view from <code>/api/v1/broadcasts</code>. "Kick" closes the publisher's
        socket (subscribers see end-of-stream); the publisher can reconnect immediately. Today
        wired for RTMP; WHIP / SRT / RTSP follow.
      </p>
      <div class="btable" role="table" aria-label="Live publisher sessions">
        <div class="tr th" role="row">
          <span role="columnheader">Broadcast</span>
          <span role="columnheader">Protocol</span>
          <span role="columnheader">Peer</span>
          <span role="columnheader">Uptime</span>
          <span role="columnheader"><span class="vh">Actions</span></span>
        </div>
        <div v-if="!broadcasts.state?.sessions?.length" class="tr empty-row" role="row">
          <span role="cell" class="hint">No live publisher sessions.</span>
        </div>
        <div v-for="b in broadcasts.state?.sessions ?? []" :key="b.broadcast" class="tr" role="row">
          <span role="cell" class="mono">{{ b.broadcast }}</span>
          <span role="cell" class="proto">{{ b.protocol.toUpperCase() }}</span>
          <span role="cell" class="mono">{{ b.peer ?? '--' }}</span>
          <span role="cell" class="mono">{{ uptimeFor(b.started_ms) }}</span>
          <span role="cell" class="action">
            <Button
              variant="danger"
              :disabled="pendingKill === b.broadcast"
              :aria-label="`Kick publisher of ${b.broadcast}`"
              @click="killBroadcast(b.broadcast, b.protocol)"
            >
              {{ pendingKill === b.broadcast ? 'Kicking...' : 'Kick' }}
            </Button>
          </span>
        </div>
      </div>
      <p v-if="broadcasts.error" class="hint" style="margin-top: var(--s-3)">
        broadcasts unavailable: {{ broadcasts.error }}
      </p>
    </Card>

    <Card kicker="REGISTRY" title="Broadcasts with fragment data">
      <div class="streams-list">
        <StreamRow v-for="s in streams.streams" :key="s.name" :stream="s" />
        <p v-if="!streams.streams.length" class="empty">
          No broadcasts in the registry yet.
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
  grid-template-columns: 1fr 2fr 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.ltable .tr.empty-row {
  grid-template-columns: 1fr;
}
.ltable .action {
  display: flex;
  justify-content: flex-end;
}
/* Visually-hidden label for the action column header so axe-core does not
   flag the otherwise-empty <span role="columnheader">. */
.vh {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
.ltable .tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.ltable .proto,
.btable .proto {
  font-family: var(--font-mono);
  font-weight: 700;
  color: var(--tally-deep);
}
.btable {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.btable .tr {
  display: grid;
  grid-template-columns: minmax(0, 2fr) 1fr minmax(0, 2fr) 1fr auto;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.btable .tr.empty-row {
  grid-template-columns: 1fr;
}
.btable .tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.btable .action {
  display: flex;
  justify-content: flex-end;
}
.btable .mono {
  font-family: var(--font-mono);
  color: var(--ink-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
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
