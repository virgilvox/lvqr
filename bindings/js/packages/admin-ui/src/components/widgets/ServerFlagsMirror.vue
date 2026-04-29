<script setup lang="ts">
import { computed, ref } from 'vue';
import { useStatsStore } from '@/stores/stats';
import { useMeshStore } from '@/stores/mesh';
import { useClusterStore } from '@/stores/cluster';
import { useWasmFilterStore } from '@/stores/wasmFilter';
import { useStreamKeysStore } from '@/stores/streamkeys';
import { useConfigReloadStore } from '@/stores/configReload';
import { useConnectionStore } from '@/stores/connection';
import { useToast } from '@/composables/useToast';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';

/**
 * Derives the `lvqr serve` flag set the active relay would need to be
 * invoked with to reproduce its currently-observable runtime state. This
 * is an APPROXIMATION -- LVQR doesn't expose a full "what flags am I
 * running with" admin route today, so we infer from the data the admin
 * routes already serve:
 *
 *   * mesh.enabled  -> --mesh-enabled
 *   * cluster.available  -> --cluster-listen <addr>
 *   * wasmFilter.chain_length > 0  -> --wasm-filter <path>...
 *   * configReload.config_path set  -> --config <path>
 *   * stream-key store available + has keys  -> default; --no-streamkeys removes it
 *   * profile per-protocol port overrides -> the matching --<proto>-port flag
 *
 * Operators copy the rendered command into their service unit (or paste
 * into a Docker run, or use as a recipe in their k8s manifest). The flag
 * mirror is intentionally read-only; the live runtime state of the relay
 * is what's reported, not edits.
 */

const conn = useConnectionStore();
const stats = useStatsStore();
const mesh = useMeshStore();
const cluster = useClusterStore();
const wasm = useWasmFilterStore();
const sk = useStreamKeysStore();
const cfg = useConfigReloadStore();
const { push } = useToast();

const inferredFlags = computed<string[]>(() => {
  const profile = conn.activeProfile;
  if (!profile) return ['# no active connection'];
  const flags: string[] = [];

  // Admin port (always present; derive from the URL).
  try {
    const url = new URL(profile.baseUrl);
    if (url.port) flags.push(`--admin-port ${url.port}`);
  } catch {
    // ignore
  }

  if (profile.rtmpPort != null) flags.push(`--rtmp-port ${profile.rtmpPort}`);
  if (profile.whipPort != null) flags.push(`--whip-port ${profile.whipPort}`);
  if (profile.whepPort != null) flags.push(`--whep-port ${profile.whepPort}`);
  if (profile.hlsPort != null) flags.push(`--hls-port ${profile.hlsPort}`);
  if (profile.dashPort != null) flags.push(`--dash-port ${profile.dashPort}`);
  if (profile.srtPort != null) flags.push(`--srt-port ${profile.srtPort}`);
  if (profile.rtspPort != null) flags.push(`--rtsp-port ${profile.rtspPort}`);
  if (profile.moqPort != null) flags.push(`--port ${profile.moqPort}`);

  if (mesh.mesh?.enabled) flags.push('--mesh-enabled');
  if (cluster.available && cluster.nodes.length) {
    const me = cluster.nodes[0];
    flags.push(`--cluster-listen ${me.gossip_addr}`);
  }
  if (wasm.state?.enabled && (wasm.state?.chain_length ?? 0) > 0) {
    for (let i = 0; i < (wasm.state?.chain_length ?? 0); i++) {
      flags.push(`--wasm-filter <path-to-filter-${i + 1}.wasm>`);
    }
  }
  if (cfg.status?.config_path) flags.push(`--config ${cfg.status.config_path}`);
  if (sk.keys.length === 0 && cfg.status?.config_path == null) {
    // No way to tell from the wire whether stream-keys are enabled when
    // the list is empty + no config; emit a comment instead of a flag.
    flags.push('# (stream-key CRUD enabled by default; pass --no-streamkeys to disable)');
  }

  if (!flags.length) flags.push('# defaults; lvqr serve');
  return flags;
});

const command = computed(() => {
  return `lvqr serve \\\n  ${inferredFlags.value.join(' \\\n  ')}`;
});

const justCopied = ref(false);
async function copy() {
  try {
    await navigator.clipboard.writeText(command.value);
    justCopied.value = true;
    push('success', 'flags copied');
    window.setTimeout(() => {
      justCopied.value = false;
    }, 1500);
  } catch {
    push('error', 'clipboard not available; select the text manually');
  }
}

void stats; // silence unused; the store gets polled via Settings to keep the runtime fresh
</script>

<template>
  <div class="mirror">
    <p class="hint">
      Inferred from the relay's observable runtime state -- LVQR does not yet
      expose a "what flags am I running with" admin route. Treat this as a
      starting template; placeholders like
      <code>&lt;path-to-filter-N.wasm&gt;</code> need to be substituted with
      your actual paths.
    </p>
    <pre class="cmd">{{ command }}</pre>
    <div class="actions">
      <Button :variant="justCopied ? 'wire' : 'primary'" @click="copy">
        <Icon :name="justCopied ? 'check' : 'copy'" :size="12" />
        {{ justCopied ? 'copied' : 'Copy command' }}
      </Button>
    </div>
  </div>
</template>

<style scoped>
.mirror {
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
.cmd {
  background: var(--ink);
  color: var(--paper);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.7;
  padding: var(--s-3) var(--s-4);
  overflow-x: auto;
  white-space: pre;
  margin: 0;
}
.actions {
  display: flex;
  justify-content: flex-end;
}
</style>
