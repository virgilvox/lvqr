<script setup lang="ts">
import PageHeader from '@/components/ui/PageHeader.vue';
import EmptyState from '@/components/ui/EmptyState.vue';
import Button from '@/components/ui/Button.vue';

// LVQR v1.x backlog: archive list API. Today the DVR scrub route at
// /playback/{broadcast}?from=&to= returns JSON for a single broadcast's
// segments but there is no admin "list every recording" route. Operators
// list recordings via the configured archive directory on disk
// (--archive-dir / ARCHIVE_DIR / object_store backend).
</script>

<template>
  <div class="page">
    <PageHeader crumb="CONSOLE / OPERATIONS / RECORDINGS">
      <template #title>Archive <em>vault.</em></template>
    </PageHeader>

    <EmptyState
      kicker="V1.X BACKLOG"
      title="No archive list API yet."
    >
      The LVQR DVR surface exposes per-broadcast scrub
      (<code>/playback/&lt;broadcast&gt;?from=&amp;to=</code>) and per-segment file delivery, but
      no admin route lists every recorded broadcast. List recordings via the configured archive
      directory on disk or your S3-compatible backend's bucket listing.
      <template #actions>
        <RouterLink to="/dvr">
          <Button variant="primary">Open DVR scrubber</Button>
        </RouterLink>
      </template>
    </EmptyState>
  </div>
</template>

<style scoped>
.page {
  padding: var(--s-6) var(--s-7);
  max-width: 1600px;
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
