<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Card from '@/components/ui/Card.vue';

// LVQR v1.x backlog: per-agent admin route. Today agents (e.g. whisper
// captions via lvqr-agent-whisper) are configured at process startup with
// --whisper-model flags + the lvqr-agent runner registers them
// programmatically. No runtime list / start / stop admin surface.
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / AGENTS">
      <template #title>In-process <em>agents.</em></template>
    </PageHeader>

    <EmptyState
      kicker="V1.X BACKLOG"
      title="No agent admin route yet."
    >
      In-process agents (Whisper captions, AI overlays) attach to broadcasts via the
      <code>lvqr_agent</code> trait. They are configured with startup flags + registered
      programmatically. A runtime list / start / stop admin route is on the v1.x backlog.
    </EmptyState>

    <Card kicker="REFERENCE" title="Whisper captions recipe">
      <p>
        Add <code>--whisper-model &lt;path/to/ggml-tiny.en.bin&gt;</code> to <code>lvqr serve</code>.
        The agent attaches to every broadcast and emits a sibling captions track that
        <code>@lvqr/dvr-player</code> renders through the standard HLS subtitle rendition.
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
p {
  font-size: 13px;
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
