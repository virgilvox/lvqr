<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import Card from '@/components/ui/Card.vue';
import KpiTile from '@/components/ui/KpiTile.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import MeshTreeViz from '@/components/widgets/MeshTreeViz.vue';
import { useMeshStore } from '@/stores/mesh';
import { usePolling } from '@/composables/usePolling';

const mesh = useMeshStore();
usePolling(() => mesh.fetch(), { intervalMs: 10_000 });

const peers = computed(() => mesh.mesh?.peers ?? []);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / INFRASTRUCTURE / MESH">
      <template #title>WebRTC <em>peer tree.</em></template>
    </PageHeader>

    <EmptyState
      v-if="!mesh.mesh?.enabled"
      kicker="DISABLED"
      title="Peer mesh not enabled."
    >
      Boot the relay with <code>--mesh-enabled</code> to opt subscribers into the
      DataChannel-based peer relay. The mesh saves origin bandwidth at the cost of WebRTC ICE
      complexity.
    </EmptyState>

    <template v-else>
      <section class="kpis">
        <KpiTile label="Peers" :value="mesh.mesh?.peer_count ?? 0" />
        <KpiTile
          label="Intended offload"
          :value="(mesh.mesh?.offload_percentage ?? 0).toFixed(1) + '%'"
          accent="wire"
        />
        <KpiTile
          label="Forwarded frames"
          :value="peers.reduce((acc, p) => acc + p.forwarded_frames, 0)"
          accent="none"
        />
      </section>

      <Card kicker="TOPOLOGY" title="Tree">
        <MeshTreeViz :peers="peers" />
      </Card>

      <Card kicker="PEERS" title="Per-peer detail">
        <div class="peer-list">
          <article v-for="p in peers" :key="p.peer_id">
            <header>
              <strong>{{ p.peer_id }}</strong>
              <span class="role" :class="`role-${p.role.toLowerCase()}`">{{ p.role }}</span>
            </header>
            <div class="peer-grid">
              <span>parent <strong>{{ p.parent ?? '-' }}</strong></span>
              <span>depth <strong>{{ p.depth }}</strong></span>
              <span>intended children <strong>{{ p.intended_children }}</strong></span>
              <span>forwarded <strong>{{ p.forwarded_frames }}</strong></span>
              <span v-if="p.capacity != null">capacity <strong>{{ p.capacity }}</strong></span>
            </div>
          </article>
        </div>
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
.peer-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.peer-list article {
  background: var(--paper-hi);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.peer-list header {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  font-family: var(--font-mono);
  font-size: 12px;
}
.peer-list strong {
  color: var(--ink);
}
.role {
  font-size: 10px;
  letter-spacing: 0.15em;
  text-transform: uppercase;
  padding: 2px 6px;
  border: 1px solid;
  font-weight: 700;
}
.role-root {
  color: var(--tally-deep);
  border-color: var(--tally);
  background: var(--tally-wash);
}
.role-relay {
  color: var(--wire-deep);
  border-color: var(--wire);
  background: var(--wire-wash);
}
.role-leaf {
  color: var(--ink-muted);
  border-color: var(--chalk-hi);
}
.peer-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: var(--s-2);
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
}
</style>
