import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useConnectionStore } from '../../src/stores/connection';
import { useStatsStore } from '../../src/stores/stats';
import { useStreamsStore } from '../../src/stores/streams';
import { useMeshStore } from '../../src/stores/mesh';
import { useSloStore } from '../../src/stores/slo';
import { useStreamKeysStore } from '../../src/stores/streamkeys';
import { useConfigReloadStore } from '../../src/stores/configReload';
import { useWasmFilterStore } from '../../src/stores/wasmFilter';
import { useHealthStore } from '../../src/stores/health';
import { useClusterStore } from '../../src/stores/cluster';

// Sanity-test every per-resource store by stubbing the active connection's
// LvqrAdminClient and asserting that a store fetch hits the right method.
// These guards lock the wiring that real views depend on; if a future
// refactor renames a method on @lvqr/core, a failing test here surfaces the
// drift before the UI silently displays empty data.

beforeEach(() => {
  setActivePinia(createPinia());
  localStorage.clear();
});

function stubClient(stub: Record<string, unknown>) {
  const conn = useConnectionStore();
  conn.addProfile({ label: 'stub', baseUrl: 'http://stub:8080' });
  // Override the Pinia computed `client` getter via Object.defineProperty so
  // every store resolving through `conn.client` sees the stub.
  Object.defineProperty(conn, 'client', { value: stub, configurable: true });
  return conn;
}

describe('per-resource stores', () => {
  it('stats.fetch calls client.stats and stamps lastFetchedAt', async () => {
    const spy = vi.fn().mockResolvedValue({
      publishers: 1,
      subscribers: 2,
      tracks: 3,
      bytes_received: 0,
      bytes_sent: 0,
      uptime_secs: 0,
    });
    stubClient({ stats: spy });
    const stats = useStatsStore();
    await stats.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(stats.stats?.subscribers).toBe(2);
    expect(stats.lastFetchedAt).not.toBeNull();
  });

  it('streams.fetch calls client.listStreams', async () => {
    const spy = vi.fn().mockResolvedValue([{ name: 'live/x', subscribers: 7 }]);
    stubClient({ listStreams: spy });
    const s = useStreamsStore();
    await s.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(s.streams[0]).toEqual({ name: 'live/x', subscribers: 7 });
  });

  it('mesh.fetch calls client.mesh', async () => {
    const spy = vi.fn().mockResolvedValue({
      enabled: true,
      peer_count: 3,
      offload_percentage: 50,
      peers: [],
    });
    stubClient({ mesh: spy });
    const s = useMeshStore();
    await s.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(s.mesh?.peer_count).toBe(3);
  });

  it('slo.fetch calls client.slo', async () => {
    const spy = vi.fn().mockResolvedValue({ broadcasts: [] });
    stubClient({ slo: spy });
    const s = useSloStore();
    await s.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(Array.isArray(s.slo?.broadcasts)).toBe(true);
  });

  it('streamkeys.mint then revoke each call the client + refresh the list', async () => {
    const listSpy = vi.fn().mockResolvedValue([]);
    const mintSpy = vi
      .fn()
      .mockResolvedValue({ id: 'k1', token: 'lvqr_sk_xyz', created_at: 0 });
    const revokeSpy = vi.fn().mockResolvedValue(undefined);
    stubClient({
      listStreamKeys: listSpy,
      mintStreamKey: mintSpy,
      revokeStreamKey: revokeSpy,
    });
    const s = useStreamKeysStore();
    const k = await s.mint({ label: 'a' });
    expect(k.id).toBe('k1');
    expect(mintSpy).toHaveBeenCalledTimes(1);
    expect(listSpy).toHaveBeenCalledTimes(1);
    await s.revoke('k1');
    expect(revokeSpy).toHaveBeenCalledWith('k1');
    expect(listSpy).toHaveBeenCalledTimes(2);
  });

  it('configReload.trigger calls client.triggerConfigReload', async () => {
    const spy = vi.fn().mockResolvedValue({
      config_path: '/etc/lvqr.toml',
      last_reload_at_ms: 1,
      last_reload_kind: 'admin_post',
      applied_keys: ['auth'],
      warnings: [],
    });
    stubClient({ triggerConfigReload: spy });
    const s = useConfigReloadStore();
    const next = await s.trigger();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(next.last_reload_kind).toBe('admin_post');
  });

  it('wasmFilter.fetch calls client.wasmFilter', async () => {
    const spy = vi.fn().mockResolvedValue({
      enabled: true,
      chain_length: 1,
      broadcasts: [],
      slots: [{ index: 0, seen: 0, kept: 0, dropped: 0 }],
    });
    stubClient({ wasmFilter: spy });
    const s = useWasmFilterStore();
    await s.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(s.state?.chain_length).toBe(1);
  });

  it('health.fetch calls client.healthz', async () => {
    const spy = vi.fn().mockResolvedValue(true);
    stubClient({ healthz: spy });
    const s = useHealthStore();
    await s.fetch();
    expect(spy).toHaveBeenCalledTimes(1);
    expect(s.healthy).toBe(true);
  });

  it('cluster.fetch flips available=false when the relay returns 500', async () => {
    const err500 = new Error('GET /api/v1/cluster/nodes: HTTP 500 Internal Server Error');
    stubClient({
      clusterNodes: vi.fn().mockRejectedValue(err500),
      clusterBroadcasts: vi.fn().mockRejectedValue(err500),
      clusterConfig: vi.fn().mockRejectedValue(err500),
      clusterFederation: vi.fn().mockRejectedValue(err500),
    });
    const s = useClusterStore();
    await s.fetch();
    expect(s.available).toBe(false);
    expect(s.nodes).toEqual([]);
    expect(s.broadcasts).toEqual([]);
    expect(s.config).toEqual([]);
    expect(s.federation?.links).toEqual([]);
  });

  it('cluster.fetch hits all four cluster routes when available', async () => {
    const nodesSpy = vi.fn().mockResolvedValue([]);
    const bcsSpy = vi.fn().mockResolvedValue([]);
    const cfgSpy = vi.fn().mockResolvedValue([]);
    const fedSpy = vi.fn().mockResolvedValue({ links: [] });
    stubClient({
      clusterNodes: nodesSpy,
      clusterBroadcasts: bcsSpy,
      clusterConfig: cfgSpy,
      clusterFederation: fedSpy,
    });
    const s = useClusterStore();
    await s.fetch();
    expect(nodesSpy).toHaveBeenCalledTimes(1);
    expect(bcsSpy).toHaveBeenCalledTimes(1);
    expect(cfgSpy).toHaveBeenCalledTimes(1);
    expect(fedSpy).toHaveBeenCalledTimes(1);
    expect(s.available).toBe(true);
  });
});
