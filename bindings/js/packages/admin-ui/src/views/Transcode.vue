<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Card from '@/components/ui/Card.vue';

// LVQR v1.x backlog: per-broadcast transcode ladder + encoder choice via the
// admin API. Currently configured with --transcode-rendition + --transcode-encoder
// flags at startup; no runtime mutation surface.
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / PIPELINE / TRANSCODE">
      <template #title>Transcode <em>ladders.</em></template>
    </PageHeader>

    <EmptyState
      kicker="V1.X BACKLOG"
      title="No runtime transcode API yet."
    >
      Transcode ladders are configured at process startup with
      <code>--transcode-rendition &lt;name&gt;</code>
      (e.g. <code>--transcode-rendition 720p --transcode-rendition 480p</code>). Encoder
      backend is selected with <code>--transcode-encoder
      software|videotoolbox|nvenc|vaapi|qsv</code>.
    </EmptyState>

    <Card kicker="REFERENCE" title="Encoder backends">
      <ul class="ref">
        <li><strong>software</strong> -- x264; portable; default.</li>
        <li><strong>videotoolbox</strong> -- macOS Apple Silicon HW; build with <code>--features hw-videotoolbox</code>.</li>
        <li><strong>nvenc</strong> -- Linux + Nvidia GPU; build with <code>--features hw-nvenc</code>.</li>
        <li><strong>vaapi</strong> -- Linux + Intel iGPU + AMD; build with <code>--features hw-vaapi</code>.</li>
        <li><strong>qsv</strong> -- Linux + Intel Quick Sync; build with <code>--features hw-qsv</code>.</li>
      </ul>
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
.ref {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  font-family: var(--font-mono);
  font-size: 12px;
  color: var(--ink-muted);
}
.ref li strong {
  color: var(--tally-deep);
  margin-right: 8px;
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
