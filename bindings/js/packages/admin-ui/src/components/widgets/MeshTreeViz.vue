<script setup lang="ts">
import { computed } from 'vue';
import type { MeshPeerStats } from '@lvqr/core';

const props = defineProps<{
  peers: MeshPeerStats[];
}>();

interface Node extends MeshPeerStats {
  children: Node[];
  x?: number;
  y?: number;
}

interface Layout {
  nodes: Node[];
  width: number;
  height: number;
}

const layout = computed<Layout>(() => {
  const byId = new Map<string, Node>();
  for (const p of props.peers) {
    byId.set(p.peer_id, { ...p, children: [] });
  }
  const roots: Node[] = [];
  for (const node of byId.values()) {
    if (node.parent && byId.has(node.parent)) {
      byId.get(node.parent)!.children.push(node);
    } else {
      roots.push(node);
    }
  }

  // Compute level widths (count nodes at each depth across roots).
  const levels = new Map<number, Node[]>();
  function collect(node: Node) {
    const list = levels.get(node.depth) ?? [];
    list.push(node);
    levels.set(node.depth, list);
    for (const c of node.children) collect(c);
  }
  roots.forEach(collect);

  const widthPerNode = 150;
  const verticalGap = 90;
  const horizontalPad = 30;
  const verticalPad = 40;
  const maxLevelCount = Math.max(1, ...Array.from(levels.values()).map((l) => l.length));
  const width = maxLevelCount * widthPerNode + horizontalPad * 2;
  const depth = Math.max(1, ...Array.from(levels.keys())) + 1;
  const height = depth * verticalGap + verticalPad * 2;

  // Position nodes per level.
  for (const [d, list] of levels.entries()) {
    list.forEach((n, idx) => {
      n.x = horizontalPad + (idx + 0.5) * ((width - horizontalPad * 2) / list.length);
      n.y = verticalPad + d * verticalGap;
    });
  }

  return {
    nodes: Array.from(byId.values()),
    width,
    height,
  };
});

const edges = computed(() =>
  layout.value.nodes
    .filter((n) => n.parent)
    .map((n) => {
      const parent = layout.value.nodes.find((p) => p.peer_id === n.parent);
      if (!parent) return null;
      return { from: parent, to: n };
    })
    .filter((e): e is { from: Node; to: Node } => e !== null),
);

function dotColor(role: string): string {
  if (role === 'Root') return 'var(--tally)';
  if (role === 'Relay') return 'var(--wire)';
  return 'var(--ink-muted)';
}
</script>

<template>
  <div class="viz">
    <svg
      v-if="peers.length"
      :viewBox="`0 0 ${layout.width} ${layout.height}`"
      :style="{ minHeight: `${layout.height}px` }"
      role="img"
      aria-label="Peer mesh topology"
    >
      <line
        v-for="(edge, i) in edges"
        :key="`e-${i}`"
        :x1="edge.from.x"
        :y1="edge.from.y"
        :x2="edge.to.x"
        :y2="edge.to.y"
        stroke="var(--chalk-hi)"
        stroke-width="1.5"
      />
      <g v-for="node in layout.nodes" :key="node.peer_id">
        <circle :cx="node.x" :cy="node.y" r="14" :fill="dotColor(node.role)" stroke="var(--paper)" stroke-width="2" />
        <text
          :x="node.x"
          :y="node.y! + 32"
          text-anchor="middle"
          font-family="var(--font-mono)"
          font-size="10"
          fill="var(--ink)"
        >
          {{ node.peer_id }}
        </text>
        <text
          :x="node.x"
          :y="node.y! + 46"
          text-anchor="middle"
          font-family="var(--font-mono)"
          font-size="9"
          fill="var(--ink-faint)"
        >
          {{ node.role }} - {{ node.forwarded_frames }} fwd
        </text>
      </g>
    </svg>
    <div v-else class="viz-empty">No peers registered.</div>
  </div>
</template>

<style scoped>
.viz {
  background: var(--paper);
  border: 1px solid var(--chalk-hi);
  padding: var(--s-4);
  overflow-x: auto;
}
.viz svg {
  width: 100%;
  height: auto;
  display: block;
}
.viz-empty {
  font-family: var(--font-mono);
  color: var(--ink-faint);
  font-size: 12px;
  padding: var(--s-5);
  text-align: center;
}
</style>
