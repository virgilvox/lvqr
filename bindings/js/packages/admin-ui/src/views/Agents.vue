<script setup lang="ts">
import { computed, reactive, ref } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import Card from '@/components/ui/Card.vue';
import Badge from '@/components/ui/Badge.vue';
import Button from '@/components/ui/Button.vue';
import { useAgentsStore } from '@/stores/agents';
import { usePolling } from '@/composables/usePolling';
import { useToast } from '@/composables/useToast';

// Introspection + runtime CRUD of in-process agents via /api/v1/agents.
// Today this drives the Whisper captions agent: start one by giving a model
// path, stop it by name. New and already-live broadcasts both pick up a
// started agent.
const agents = useAgentsStore();
const { push } = useToast();
usePolling(() => agents.fetch(), { intervalMs: 10_000 });

const state = computed(() => agents.state);
const enabled = computed(() => agents.state?.enabled ?? false);
const configured = computed(() => agents.state?.agents ?? []);
const active = computed(() => agents.state?.active ?? []);

const form = reactive({ model: '', window_ms: 5000 });
const submitting = ref(false);
const removing = ref<string | null>(null);

async function submitAdd() {
  if (!form.model.trim()) {
    push('error', 'a model path is required');
    return;
  }
  submitting.value = true;
  try {
    await agents.addAgent({ model: form.model.trim(), window_ms: form.window_ms });
    push('success', 'started captions agent');
    form.model = '';
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.includes('409')) push('error', 'an agent is already running', 6000);
    else if (msg.includes('503')) push('error', 'agent mutation unavailable (build with --features whisper)', 7000);
    else push('error', msg, 6000);
  } finally {
    submitting.value = false;
  }
}

async function removeAgent(name: string) {
  removing.value = name;
  try {
    await agents.removeAgent(name);
    push('success', `stopped agent ${name}`);
  } catch (e) {
    push('error', e instanceof Error ? e.message : String(e), 6000);
  } finally {
    removing.value = null;
  }
}
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / AGENTS">
      <template #title>In-process <em>agents.</em></template>
    </PageHeader>

    <p v-if="agents.error" class="state state-error">{{ agents.error }}</p>

    <div v-else-if="!state" class="state state-loading"><span class="spinner" /> Loading agents...</div>

    <template v-else>
      <template v-if="enabled">
        <section class="kpis">
          <KpiTile label="Running agents" :value="configured.length" />
          <KpiTile label="Live attachments" :value="active.length" accent="wire" />
        </section>

        <Card v-for="a in configured" :key="a.name" :kicker="a.kind.toUpperCase()" :title="a.name">
          <template #actions>
            <Button variant="ghost" :loading="removing === a.name" :aria-label="`Stop ${a.name}`" @click="removeAgent(a.name)">
              Stop
            </Button>
          </template>
          <dl class="meta">
            <template v-if="a.model">
              <dt>Model</dt>
              <dd class="mono">{{ a.model }}</dd>
            </template>
            <template v-if="a.window_ms != null">
              <dt>Window</dt>
              <dd class="mono">{{ a.window_ms }} ms</dd>
            </template>
          </dl>
        </Card>

        <Card kicker="LIVE" title="Attachments" wire>
          <div v-if="active.length" class="atable" role="table" aria-label="Live agent attachments">
            <div class="tr th" role="row">
              <span role="columnheader">Agent</span>
              <span role="columnheader">Source</span>
              <span role="columnheader">Track</span>
              <span role="columnheader" class="num">Fragments</span>
              <span role="columnheader" class="num">Panics</span>
            </div>
            <div v-for="x in active" :key="`${x.agent}|${x.broadcast}|${x.track}`" class="tr" role="row">
              <span role="cell"><Badge variant="tally">{{ x.agent }}</Badge></span>
              <span role="cell" class="mono">{{ x.broadcast }}</span>
              <span role="cell" class="mono">{{ x.track }}</span>
              <span role="cell" class="num">{{ x.fragments_seen.toLocaleString() }}</span>
              <span role="cell" class="num" :class="{ warn: x.panics > 0 }">{{ x.panics }}</span>
            </div>
          </div>
          <p v-else class="empty">
            No agents are attached to a live broadcast yet. Agents attach when a matching source
            track starts publishing.
          </p>
        </Card>
      </template>

      <EmptyState v-else kicker="NO AGENTS" title="No agents running.">
        Start the Whisper captions agent below by giving it a model path, or configure one at
        startup with <code>--whisper-model &lt;path/to/ggml-tiny.en.bin&gt;</code>. The relay must be
        built with <code>--features whisper</code>; the agent emits a sibling captions track that
        <code>@lvqr/dvr-player</code> renders through the standard HLS subtitle rendition.
      </EmptyState>

      <Card kicker="START AGENT" title="Whisper captions">
        <form class="addform" @submit.prevent="submitAdd">
          <div class="fields">
            <label class="grow"><span>Model path</span><input v-model="form.model" placeholder="/models/ggml-tiny.en.bin" required /></label>
            <label><span>Window ms</span><input v-model.number="form.window_ms" type="number" min="100" /></label>
            <Button variant="primary" type="submit" :loading="submitting">Start</Button>
          </div>
          <p class="note">
            The model path is read on the relay host. Starting attaches the captions agent to new and
            already-live broadcasts; build with <code>--features whisper</code> for this to be available.
          </p>
        </form>
      </Card>
    </template>
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
.kpis {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
}
.meta {
  display: grid;
  grid-template-columns: max-content 1fr;
  gap: 4px var(--s-4);
  font-size: 13px;
}
.meta dt {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  align-self: center;
}
.atable {
  display: flex;
  flex-direction: column;
  font-size: 13px;
}
.tr {
  display: grid;
  grid-template-columns: 1fr 1.6fr 0.8fr 1fr 0.6fr;
  gap: var(--s-3);
  align-items: center;
  padding: 8px 4px;
  border-bottom: 1px solid var(--chalk-lo);
}
.tr.th {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  color: var(--ink-faint);
  border-bottom: 1px solid var(--chalk-hi);
}
.mono {
  font-family: var(--font-mono);
}
.num {
  text-align: right;
  font-variant-numeric: tabular-nums;
  font-family: var(--font-mono);
}
.num.warn {
  color: var(--warn);
}
.state {
  font-family: var(--font-mono);
  font-size: 12px;
  display: flex;
  align-items: center;
  gap: 6px;
  padding: var(--s-3) 0;
}
.state-error {
  color: var(--on-air);
}
.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--chalk-hi);
  border-top-color: var(--tally-deep);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
.empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
}
.addform .fields {
  display: flex;
  flex-wrap: wrap;
  align-items: flex-end;
  gap: var(--s-3);
}
.addform label {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.addform label.grow {
  flex: 1;
  min-width: 240px;
}
.addform label span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.1em;
  text-transform: uppercase;
  color: var(--ink-faint);
}
.addform input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: 6px 9px;
  font-family: var(--font-mono);
  font-size: 13px;
}
.addform label:not(.grow) input {
  width: 110px;
}
.note {
  margin-top: var(--s-2);
  font-family: var(--font-mono);
  font-size: 11px;
  color: var(--ink-muted);
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
  .tr {
    grid-template-columns: 1fr 1.4fr 0.6fr 0.6fr;
  }
  .tr span:nth-child(3) {
    display: none;
  }
}
</style>
