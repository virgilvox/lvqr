// @lvqr/core admin-client smoke tests.
//
// Every method on `LvqrAdminClient` is hit against a locally-running
// `lvqr serve` and the response body is asserted against the
// declared TypeScript shape. These are the tests the session-121
// audit surfaced as missing: before this suite, the admin client's
// `/api/v1/mesh`, `/api/v1/slo`, and `/api/v1/cluster/*` methods
// landed with zero runtime verification.
//
// Test setup assumes lvqr is running at `LVQR_TEST_ADMIN_URL`
// (default `http://127.0.0.1:18090`) with `--mesh-enabled` +
// `--cluster-listen` so both mesh and cluster routes answer 200.
// The CI workflow boots the binary with the right flags before
// invoking `vitest`. Locally, either boot lvqr yourself or run
// `npm run test:sdk:ci` which spawns + tears down the server
// automatically.

import { describe, expect, it, beforeAll } from 'vitest';
import { LvqrAdminClient } from '@lvqr/core';

const ADMIN_URL = process.env.LVQR_TEST_ADMIN_URL ?? 'http://127.0.0.1:18090';

describe('LvqrAdminClient against a running lvqr', () => {
  const admin = new LvqrAdminClient(ADMIN_URL, { fetchTimeoutMs: 5_000 });

  beforeAll(async () => {
    // Wait up to 10 s for the server to bind. The CI webServer step
    // starts the binary in the background; the health probe here
    // catches the "cargo build then spawn" race.
    const deadline = Date.now() + 10_000;
    while (Date.now() < deadline) {
      try {
        const resp = await fetch(`${ADMIN_URL}/healthz`);
        if (resp.ok) {
          return;
        }
      } catch {
        // TCP refused -- server still starting; retry.
      }
      await new Promise((resolve) => setTimeout(resolve, 200));
    }
    throw new Error(`lvqr admin at ${ADMIN_URL}/healthz never became reachable`);
  });

  it('healthz returns true', async () => {
    expect(await admin.healthz()).toBe(true);
  });

  it('stats returns a RelayStats shape', async () => {
    const stats = await admin.stats();
    expect(typeof stats.publishers).toBe('number');
    expect(typeof stats.subscribers).toBe('number');
    expect(typeof stats.tracks).toBe('number');
    expect(typeof stats.bytes_received).toBe('number');
    expect(typeof stats.bytes_sent).toBe('number');
    expect(typeof stats.uptime_secs).toBe('number');
  });

  it('listStreams returns an array', async () => {
    const streams = await admin.listStreams();
    expect(Array.isArray(streams)).toBe(true);
    // No publisher in the test harness; expect zero streams.
    for (const s of streams) {
      expect(typeof s.name).toBe('string');
      expect(typeof s.subscribers).toBe('number');
    }
  });

  it('mesh returns a MeshState shape', async () => {
    const mesh = await admin.mesh();
    expect(typeof mesh.enabled).toBe('boolean');
    expect(typeof mesh.peer_count).toBe('number');
    expect(typeof mesh.offload_percentage).toBe('number');
    // Test fixture boots with --mesh-enabled so `enabled` must be true.
    expect(mesh.enabled).toBe(true);
    // Session 141: the admin body carries per-peer intended-vs-actual
    // offload stats. The sdk-tests harness has no publisher or browser
    // peers so the vec is empty, but the shape must be array-valued.
    expect(Array.isArray(mesh.peers)).toBe(true);
    for (const peer of mesh.peers) {
      expect(typeof peer.peer_id).toBe('string');
      expect(typeof peer.role).toBe('string');
      expect(peer.parent === null || typeof peer.parent === 'string').toBe(true);
      expect(typeof peer.depth).toBe('number');
      expect(typeof peer.intended_children).toBe('number');
      expect(typeof peer.forwarded_frames).toBe('number');
    }
  });

  it('slo returns a { broadcasts: [] } snapshot', async () => {
    const snapshot = await admin.slo();
    expect(Array.isArray(snapshot.broadcasts)).toBe(true);
    // Shape-check any entries that happen to be present.
    for (const entry of snapshot.broadcasts) {
      expect(typeof entry.broadcast).toBe('string');
      expect(typeof entry.transport).toBe('string');
      expect(typeof entry.p50_ms).toBe('number');
      expect(typeof entry.p95_ms).toBe('number');
      expect(typeof entry.p99_ms).toBe('number');
      expect(typeof entry.max_ms).toBe('number');
    }
  });

  it('clusterNodes returns an array of ClusterNodeView', async () => {
    const nodes = await admin.clusterNodes();
    expect(Array.isArray(nodes)).toBe(true);
    // A single-node cluster shows exactly itself.
    for (const node of nodes) {
      expect(typeof node.id).toBe('string');
      expect(typeof node.generation).toBe('number');
      expect(typeof node.gossip_addr).toBe('string');
      // capacity may be null until the first gossip round lands.
      if (node.capacity !== null) {
        expect(typeof node.capacity.cpu_pct).toBe('number');
        expect(typeof node.capacity.rss_bytes).toBe('number');
        expect(typeof node.capacity.bytes_out_per_sec).toBe('number');
      }
    }
  });

  it('clusterBroadcasts returns an array', async () => {
    const broadcasts = await admin.clusterBroadcasts();
    expect(Array.isArray(broadcasts)).toBe(true);
    for (const b of broadcasts) {
      expect(typeof b.name).toBe('string');
      expect(typeof b.owner).toBe('string');
      expect(typeof b.expires_at_ms).toBe('number');
    }
  });

  it('clusterConfig returns an array', async () => {
    const entries = await admin.clusterConfig();
    expect(Array.isArray(entries)).toBe(true);
    for (const e of entries) {
      expect(typeof e.key).toBe('string');
      expect(typeof e.value).toBe('string');
      expect(typeof e.ts_ms).toBe('number');
    }
  });

  it('clusterFederation returns a { links: [] } shape', async () => {
    const status = await admin.clusterFederation();
    expect(Array.isArray(status.links)).toBe(true);
    // No federation links configured in the test harness; expect empty.
    for (const link of status.links) {
      expect(typeof link.remote_url).toBe('string');
      expect(Array.isArray(link.forwarded_broadcasts)).toBe(true);
      expect(['connecting', 'connected', 'failed']).toContain(link.state);
      expect(typeof link.connect_attempts).toBe('number');
      expect(typeof link.forwarded_broadcasts_seen).toBe('number');
    }
  });

  it('wasmFilter returns a WasmFilterState shape reflecting the configured chain', async () => {
    const state = await admin.wasmFilter();
    expect(typeof state.enabled).toBe('boolean');
    expect(typeof state.chain_length).toBe('number');
    expect(Array.isArray(state.broadcasts)).toBe(true);
    // Session 139 wired `--wasm-filter crates/lvqr-wasm/examples/frame-counter.wasm`
    // into sdk-tests.yml's lvqr serve spawn, so the admin route
    // reports a single-slot chain. No publisher is running, so
    // `broadcasts` stays empty; the chain is live and observable.
    expect(state.enabled).toBe(true);
    expect(state.chain_length).toBe(1);
    expect(state.broadcasts.length).toBe(0);
    for (const b of state.broadcasts) {
      expect(typeof b.broadcast).toBe('string');
      expect(typeof b.track).toBe('string');
      expect(typeof b.seen).toBe('number');
      expect(typeof b.kept).toBe('number');
      expect(typeof b.dropped).toBe('number');
    }
    // Session 140: per-slot counters mirror chain_length. With no
    // publisher the counters stay at zero but the shape + length
    // must match the configured chain so dashboards can render
    // per-slot panels even before any traffic lands.
    expect(Array.isArray(state.slots)).toBe(true);
    expect(state.slots.length).toBe(1);
    expect(state.slots[0].index).toBe(0);
    expect(state.slots[0].seen).toBe(0);
    expect(state.slots[0].kept).toBe(0);
    expect(state.slots[0].dropped).toBe(0);
  });

  it('fetchTimeoutMs aborts hung requests', async () => {
    // Point the client at an IP that accepts TCP but never responds.
    // `203.0.113.1` is TEST-NET-3 (RFC 5737) and should black-hole.
    // This exercises the AbortController timer rather than DNS
    // resolution; the test asserts the promise rejects fast, not
    // a specific error message (network stacks vary).
    const slow = new LvqrAdminClient('http://203.0.113.1:9/', { fetchTimeoutMs: 500 });
    const start = Date.now();
    await expect(slow.stats()).rejects.toBeInstanceOf(Error);
    const elapsed = Date.now() - start;
    expect(elapsed).toBeLessThan(5_000);
  });
});
