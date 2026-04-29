<script setup lang="ts">
/**
 * Inline SVG icon set lifted from the storybook + admin mockup. Keep the
 * stroke / fill conventions consistent across glyphs. New icons land here
 * rather than as separate files so the shipped bundle stays compact.
 */
defineProps<{
  /** Icon identifier. Tolerates unknown names by rendering an empty box. */
  name: string;
  /** Pixel size of the rendered glyph. Default 16. */
  size?: number;
}>();
</script>

<template>
  <svg
    :width="size ?? 16"
    :height="size ?? 16"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-width="1.6"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
  >
    <template v-if="name === 'grid'">
      <rect x="3" y="3" width="7" height="9" />
      <rect x="14" y="3" width="7" height="5" />
      <rect x="14" y="12" width="7" height="9" />
      <rect x="3" y="16" width="7" height="5" />
    </template>
    <template v-else-if="name === 'streams'">
      <circle cx="12" cy="12" r="3" />
      <path d="M5.07 19A9 9 0 1 0 19 19" />
      <path d="M3 12h2M19 12h2M12 3v2M12 19v2" />
    </template>
    <template v-else-if="name === 'rec'">
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3" fill="currentColor" />
    </template>
    <template v-else-if="name === 'play'">
      <polygon points="5 3 19 12 5 21 5 3" />
    </template>
    <template v-else-if="name === 'ingest'">
      <path d="M12 2v14M5 9l7-7 7 7" />
      <rect x="3" y="18" width="18" height="4" />
    </template>
    <template v-else-if="name === 'filters'">
      <circle cx="6" cy="6" r="3" />
      <circle cx="18" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="18" cy="18" r="3" />
      <path d="M9 6h6M9 18h6M6 9v6M18 9v6" />
    </template>
    <template v-else-if="name === 'transcode'">
      <rect x="3" y="6" width="18" height="12" rx="1" />
      <path d="M3 10h18M9 14h6" />
    </template>
    <template v-else-if="name === 'agents'">
      <rect x="4" y="6" width="16" height="14" rx="2" />
      <circle cx="9" cy="13" r="1.5" />
      <circle cx="15" cy="13" r="1.5" />
      <path d="M12 2v4M8 18h8" />
    </template>
    <template v-else-if="name === 'egress'">
      <path d="M12 22V8M5 15l7 7 7-7" />
      <rect x="3" y="2" width="18" height="4" />
    </template>
    <template v-else-if="name === 'cluster'">
      <circle cx="12" cy="6" r="3" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="18" cy="18" r="3" />
      <path d="M12 9v3l-6 3M12 12l6 3" />
    </template>
    <template v-else-if="name === 'mesh'">
      <circle cx="12" cy="4" r="2" />
      <circle cx="6" cy="12" r="2" />
      <circle cx="18" cy="12" r="2" />
      <circle cx="4" cy="20" r="1.5" />
      <circle cx="9" cy="20" r="1.5" />
      <circle cx="15" cy="20" r="1.5" />
      <circle cx="20" cy="20" r="1.5" />
      <path d="M12 6v4M10 11l-3 1M14 11l3 1M5 14l-1 4M7 14l2 4M17 14l1 4M19 14l1 4" />
    </template>
    <template v-else-if="name === 'federation'">
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="12" r="3" />
      <path d="M9 12h6M11 8l-2 4 2 4M13 8l2 4-2 4" />
    </template>
    <template v-else-if="name === 'lock'">
      <rect x="5" y="11" width="14" height="10" rx="1" />
      <path d="M8 11V7a4 4 0 0 1 8 0v4" />
    </template>
    <template v-else-if="name === 'shield'">
      <path d="M12 2 4 6v6c0 5 4 9 8 10 4-1 8-5 8-10V6l-8-4z" />
      <path d="m9 12 2 2 4-4" />
    </template>
    <template v-else-if="name === 'chart'">
      <path d="M3 12h3l3-9 6 18 3-9h3" />
    </template>
    <template v-else-if="name === 'logs'">
      <rect x="4" y="4" width="16" height="16" rx="1" />
      <path d="M8 9h8M8 13h8M8 17h5" />
    </template>
    <template v-else-if="name === 'gear'">
      <circle cx="12" cy="12" r="3" />
      <path
        d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33h0a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
      />
    </template>
    <template v-else-if="name === 'search'">
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.5-3.5" />
    </template>
    <template v-else-if="name === 'plus'">
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </template>
    <template v-else-if="name === 'reload'">
      <polyline points="23 4 23 10 17 10" />
      <path d="M20.49 15A9 9 0 1 1 5.64 5.64L23 10" />
    </template>
    <template v-else-if="name === 'plugin'">
      <path d="M9 4v6M15 4v6M5 12h14M7 12v4a4 4 0 0 0 4 4h2a4 4 0 0 0 4-4v-4" />
    </template>
    <template v-else-if="name === 'menu'">
      <path d="M3 6h18M3 12h18M3 18h18" />
    </template>
    <template v-else-if="name === 'close'">
      <path d="M6 6l12 12M18 6L6 18" />
    </template>
    <template v-else-if="name === 'check'">
      <path d="M5 12l4 4L19 6" />
    </template>
    <template v-else-if="name === 'copy'">
      <rect x="9" y="9" width="11" height="11" rx="1" />
      <path d="M5 15V5a1 1 0 0 1 1-1h10" />
    </template>
    <template v-else-if="name === 'trash'">
      <path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M6 6l1 14a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-14" />
    </template>
    <template v-else-if="name === 'refresh'">
      <path d="M4 4v6h6M20 20v-6h-6" />
      <path d="M4 10a8 8 0 0 1 14-3l2 3M20 14a8 8 0 0 1-14 3l-2-3" />
    </template>
    <!-- unknown -- render an empty box so layout does not collapse -->
  </svg>
</template>
