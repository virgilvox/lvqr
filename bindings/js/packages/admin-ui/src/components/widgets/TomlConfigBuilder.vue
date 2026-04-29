<script setup lang="ts">
import { computed, ref } from 'vue';
import Button from '@/components/ui/Button.vue';
import Icon from '@/components/ui/Icon.vue';
import { useToast } from '@/composables/useToast';

/**
 * TOML config-template generator. The shape mirrors LVQR's `--config`
 * acceptance: `[auth]` / `[mesh]` / `[hmac]` / `[jwks]` / `[webhook]` keys.
 * Only the runtime-mutable subset is offered here -- those are the keys the
 * relay's session 147-149 hot-reload pipeline actually re-applies on
 * `POST /api/v1/config-reload` / SIGHUP.
 *
 * Process-startup-only fields (port bindings, archive dir, transcode
 * ladder, whisper model path, mesh `--mesh-enabled`, cluster gossip addr,
 * wasm-filter chain) are NOT in the TOML emitted here -- they live on the
 * `lvqr serve` flag line and a TOML edit cannot toggle them. The
 * "Server flags" card above documents those.
 */

const { push } = useToast();

// State.
const adminToken = ref('');
const subscribeToken = ref('');
const publishToken = ref('');
const jwtSecret = ref('');
const jwtIssuer = ref('');
const jwtAudience = ref('');
const jwksUrl = ref('');
const jwksRefresh = ref(300);
const webhookUrl = ref('');
const webhookCacheTtl = ref(60);
const webhookDenyTtl = ref(10);
const meshIceServers = ref<string[]>(['stun:stun.l.google.com:19302']);
const newIceServer = ref('');
const hmacSecret = ref('');

function addIceServer() {
  const v = newIceServer.value.trim();
  if (!v) return;
  meshIceServers.value.push(v);
  newIceServer.value = '';
}

function removeIceServer(i: number) {
  meshIceServers.value = meshIceServers.value.filter((_, idx) => idx !== i);
}

const toml = computed<string>(() => {
  const lines: string[] = [
    '# LVQR runtime-reloadable config',
    '# Pass to `lvqr serve --config <this-file.toml>` and trigger a reload',
    '# via SIGHUP or `POST /api/v1/config-reload`.',
    '#',
    '# Hot-reload-eligible keys: every section below. Process-startup-only',
    '# settings (port bindings, archive dir, transcode ladder, etc.) live',
    '# on the lvqr-serve command line, not in this file.',
    '',
  ];

  if (adminToken.value || subscribeToken.value || publishToken.value || jwtSecret.value || jwtIssuer.value || jwtAudience.value) {
    lines.push('[auth]');
    if (adminToken.value) lines.push(`admin_token = "${adminToken.value}"`);
    if (subscribeToken.value) lines.push(`subscribe_token = "${subscribeToken.value}"`);
    if (publishToken.value) lines.push(`publish_token = "${publishToken.value}"`);
    if (jwtSecret.value) lines.push(`jwt_secret = "${jwtSecret.value}"`);
    if (jwtIssuer.value) lines.push(`jwt_issuer = "${jwtIssuer.value}"`);
    if (jwtAudience.value) lines.push(`jwt_audience = "${jwtAudience.value}"`);
    lines.push('');
  }

  if (jwksUrl.value) {
    lines.push('# JWKS dynamic key discovery (mutually exclusive with jwt_secret).');
    lines.push('[auth.jwks]');
    lines.push(`url = "${jwksUrl.value}"`);
    lines.push(`refresh_interval_seconds = ${jwksRefresh.value}`);
    lines.push('');
  }

  if (webhookUrl.value) {
    lines.push('# Webhook auth provider (mutually exclusive with jwt_secret + jwks).');
    lines.push('[auth.webhook]');
    lines.push(`url = "${webhookUrl.value}"`);
    lines.push(`allow_cache_ttl_seconds = ${webhookCacheTtl.value}`);
    lines.push(`deny_cache_ttl_seconds = ${webhookDenyTtl.value}`);
    lines.push('');
  }

  if (meshIceServers.value.length) {
    lines.push('# Mesh WebRTC ICE server list, hot-reloadable.');
    lines.push('[mesh]');
    lines.push(`ice_servers = [${meshIceServers.value.map((s) => `"${s}"`).join(', ')}]`);
    lines.push('');
  }

  if (hmacSecret.value) {
    lines.push('# HMAC-signed-URL secret (single shared secret for /playback/* + live HLS/DASH).');
    lines.push('[hmac]');
    lines.push(`playback_secret = "${hmacSecret.value}"`);
    lines.push('');
  }

  if (lines.length <= 8) {
    lines.push('# (no fields filled in; this template is a no-op against the running server)');
  }

  return lines.join('\n');
});

const justCopied = ref(false);
async function copy() {
  try {
    await navigator.clipboard.writeText(toml.value);
    justCopied.value = true;
    push('success', 'TOML copied');
    window.setTimeout(() => {
      justCopied.value = false;
    }, 1500);
  } catch {
    push('error', 'clipboard not available; select the text manually');
  }
}
</script>

<template>
  <div class="builder">
    <p class="hint">
      Build a runtime-reloadable config. Only fields you fill in get emitted.
      Save the result to a file the relay reads via <code>--config</code>,
      then trigger a reload above. Anything not on this form is
      process-startup-only and won't take effect via reload.
    </p>

    <div class="cols">
      <fieldset>
        <legend>[auth] static + JWT</legend>
        <label><span>admin_token</span><input v-model="adminToken" type="password" placeholder="optional" /></label>
        <label><span>subscribe_token</span><input v-model="subscribeToken" type="password" /></label>
        <label><span>publish_token</span><input v-model="publishToken" type="password" /></label>
        <label><span>jwt_secret (HS256)</span><input v-model="jwtSecret" type="password" /></label>
        <label><span>jwt_issuer</span><input v-model="jwtIssuer" placeholder="lvqr.example.com" /></label>
        <label><span>jwt_audience</span><input v-model="jwtAudience" placeholder="lvqr-clients" /></label>
      </fieldset>

      <fieldset>
        <legend>[auth.jwks] dynamic keys</legend>
        <label><span>url</span><input v-model="jwksUrl" placeholder="https://issuer/.well-known/jwks.json" /></label>
        <label><span>refresh seconds</span><input v-model.number="jwksRefresh" type="number" min="30" /></label>
      </fieldset>

      <fieldset>
        <legend>[auth.webhook]</legend>
        <label><span>url</span><input v-model="webhookUrl" placeholder="https://my-auth/lvqr/check" /></label>
        <label><span>allow cache ttl (s)</span><input v-model.number="webhookCacheTtl" type="number" min="0" /></label>
        <label><span>deny cache ttl (s)</span><input v-model.number="webhookDenyTtl" type="number" min="0" /></label>
      </fieldset>

      <fieldset>
        <legend>[mesh] ICE servers</legend>
        <ul class="ice-list">
          <li v-for="(s, i) in meshIceServers" :key="i">
            <code>{{ s }}</code>
            <button type="button" @click="removeIceServer(i)" :title="`remove ${s}`"><Icon name="trash" :size="11" /></button>
          </li>
        </ul>
        <div class="add-ice">
          <input v-model="newIceServer" placeholder="stun:stun.l.google.com:19302" />
          <Button variant="ghost" small type="button" @click="addIceServer">add</Button>
        </div>
      </fieldset>

      <fieldset>
        <legend>[hmac] signed-URL secret</legend>
        <label><span>playback_secret</span><input v-model="hmacSecret" type="password" placeholder="--hmac-playback-secret" /></label>
      </fieldset>
    </div>

    <pre class="output">{{ toml }}</pre>
    <div class="actions">
      <Button :variant="justCopied ? 'wire' : 'primary'" @click="copy">
        <Icon :name="justCopied ? 'check' : 'copy'" :size="12" />
        {{ justCopied ? 'copied' : 'Copy TOML' }}
      </Button>
    </div>
  </div>
</template>

<style scoped>
.builder {
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
.cols {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
  gap: var(--s-3);
}
fieldset {
  border: 1px solid var(--chalk-hi);
  background: var(--paper-hi);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
legend {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.18em;
  text-transform: uppercase;
  color: var(--tally-deep);
  font-weight: 700;
  padding: 0 6px;
}
fieldset label {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
fieldset span {
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.1em;
  color: var(--ink-faint);
}
fieldset input {
  border: 1px solid var(--chalk-hi);
  background: var(--paper);
  padding: 5px 8px;
  font-size: 12px;
  font-family: var(--font-mono);
}
.ice-list {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-family: var(--font-mono);
  font-size: 11px;
  margin-bottom: var(--s-2);
}
.ice-list li {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 3px 6px;
  background: var(--chalk-lo);
  border: 1px solid var(--chalk-hi);
}
.ice-list button {
  background: transparent;
  border: 1px solid var(--chalk-hi);
  padding: 3px 5px;
  cursor: pointer;
}
.ice-list button:hover {
  background: var(--on-air);
  color: var(--paper);
  border-color: var(--on-air);
}
.add-ice {
  display: flex;
  gap: 6px;
}
.add-ice input {
  flex: 1;
}
.output {
  background: var(--ink);
  color: var(--paper);
  font-family: var(--font-mono);
  font-size: 11px;
  line-height: 1.65;
  padding: var(--s-3) var(--s-4);
  overflow-x: auto;
  white-space: pre;
  margin: 0;
  max-height: 320px;
}
.actions {
  display: flex;
  justify-content: flex-end;
}
</style>
