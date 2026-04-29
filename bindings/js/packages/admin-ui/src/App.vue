<script setup lang="ts">
import { onMounted, ref, watch } from 'vue';
import { RouterView, useRoute } from 'vue-router';
import Topbar from '@/components/ui/Topbar.vue';
import Rail from '@/components/ui/Rail.vue';
import StatusBar from '@/components/ui/StatusBar.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Button from '@/components/ui/Button.vue';
import ConnectionDrawer from '@/components/widgets/ConnectionDrawer.vue';
import ToastStack from '@/components/widgets/ToastStack.vue';
import { useConnectionStore } from '@/stores/connection';
import { useHealthStore } from '@/stores/health';
import { useStatsStore } from '@/stores/stats';
import { loadAppConfig } from '@/config/defaults';
import { useToast } from '@/composables/useToast';

const conn = useConnectionStore();
const health = useHealthStore();
const stats = useStatsStore();
const route = useRoute();
const { push: pushToast } = useToast();

const railOpen = ref(false);
const drawerOpen = ref(false);

// Bootstrap: load app-config; seed a connection profile from defaults if none
// exists so first-time operators land on a working dashboard against
// localhost:8080.
onMounted(async () => {
  const cfg = await loadAppConfig();
  if (!conn.profiles.length && cfg.defaultRelayUrl) {
    conn.addProfile({
      label: 'localhost',
      baseUrl: cfg.defaultRelayUrl,
      bearerToken: cfg.defaultBearerToken,
    });
  }
  void health.fetch();
  // Initial credentialed probe -- surfaces a toast on 401 so a wrong bearer
  // token does not silently render an empty dashboard. /healthz is open and
  // does not exercise the auth gate, so we need this separate hit on a
  // gated route to validate the token end-to-end.
  try {
    await stats.fetch();
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (/HTTP 401/.test(msg) || /HTTP 403/.test(msg)) {
      pushToast(
        'error',
        'Authentication failed -- check the bearer token on the active connection profile.',
        8000,
      );
    } else {
      pushToast('warn', `Could not reach the relay: ${msg}`, 6000);
    }
  }
});

// Background polling for the data the topbar + status bar need everywhere.
const POLL_MS = 10_000;
let timer: number | undefined;
function startBackgroundPolling() {
  if (timer !== undefined) return;
  timer = window.setInterval(() => {
    if (!conn.client) return;
    void health.fetch();
    void stats.fetch();
  }, POLL_MS);
}
startBackgroundPolling();

// Route change closes the mobile rail drawer.
watch(
  () => route.fullPath,
  () => {
    railOpen.value = false;
  },
);
</script>

<template>
  <div class="app">
    <Topbar @toggle-rail="railOpen = !railOpen" @pick-connection="drawerOpen = true" />
    <Rail :open="railOpen" @close="railOpen = false" />
    <main class="main">
      <template v-if="!conn.client">
        <div class="page-pad">
          <EmptyState kicker="WELCOME" title="Connect to a relay">
            Add your first LVQR relay to start administrating. The connection profile (URL +
            optional bearer token) lives in this browser's localStorage; nothing is persisted
            server-side.
            <template #actions>
              <Button variant="primary" @click="drawerOpen = true">Add a relay</Button>
            </template>
          </EmptyState>
        </div>
      </template>
      <RouterView v-else />
    </main>
    <StatusBar />
    <ConnectionDrawer :open="drawerOpen" @close="drawerOpen = false" />
    <ToastStack />
  </div>
</template>

<style scoped>
.app {
  display: grid;
  grid-template-columns: var(--rail-w) 1fr;
  grid-template-rows: var(--topbar-h) 1fr var(--statusbar-h);
  grid-template-areas:
    'rail topbar'
    'rail main'
    'rail statusbar';
  height: 100vh;
  width: 100vw;
}
.main {
  grid-area: main;
  overflow-y: auto;
  background: var(--bone);
  position: relative;
}
.main::before {
  content: '';
  position: absolute;
  inset: 0;
  background-image:
    radial-gradient(circle at 8% 0%, rgba(232, 117, 26, 0.04) 0%, transparent 35%),
    radial-gradient(circle at 92% 100%, rgba(14, 116, 144, 0.04) 0%, transparent 35%);
  pointer-events: none;
}
.page-pad {
  padding: var(--s-6) var(--s-7);
  position: relative;
  z-index: 1;
}

@media (max-width: 1023px) {
  .app {
    grid-template-columns: 1fr;
    grid-template-areas:
      'topbar'
      'main'
      'statusbar';
  }
}
</style>
