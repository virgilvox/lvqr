<script setup lang="ts">
import { computed } from 'vue';
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import FederationLinkCard from '@/components/widgets/FederationLinkCard.vue';
import { useClusterStore } from '@/stores/cluster';
import { usePolling } from '@/composables/usePolling';

const cluster = useClusterStore();
usePolling(() => cluster.fetch(), { intervalMs: 15_000 });

const links = computed(() => cluster.federation?.links ?? []);
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / INFRASTRUCTURE / FEDERATION">
      <template #title>Cross-cluster <em>federation.</em></template>
    </PageHeader>

    <EmptyState
      v-if="!links.length"
      kicker="NO LINKS"
      title="No federation links configured."
    >
      Federation forwards announced broadcasts between clusters over a single QUIC link.
      Configure with <code>--federation-link &lt;remote-url&gt;:&lt;token&gt;:&lt;broadcast,broadcast,...&gt;</code>
      flags (or the equivalent block in the TOML config).
    </EmptyState>

    <section v-else class="link-grid">
      <FederationLinkCard v-for="(link, i) in links" :key="i" :link="link" />
    </section>
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
.link-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(420px, 1fr));
  gap: var(--s-4);
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
