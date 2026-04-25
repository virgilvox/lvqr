/**
 * LVQR Admin API client.
 * Works in both browser and Node.js environments.
 */

export interface RelayStats {
  publishers: number;
  subscribers: number;
  tracks: number;
  bytes_received: number;
  bytes_sent: number;
  uptime_secs: number;
}

export interface StreamInfo {
  name: string;
  subscribers: number;
}

/**
 * Per-peer offload stats surfaced by `GET /api/v1/mesh`. Mirrors
 * `lvqr_admin::MeshPeerStats`. `intended_children` reflects what the
 * topology planner assigned; `forwarded_frames` reflects the
 * cumulative count the peer reported via the `/signal` `ForwardReport`
 * message. PLAN Phase D session 141.
 */
export interface MeshPeerStats {
  /** Unique peer id as seen by the coordinator. */
  peer_id: string;
  /** Tree role: `"Root"`, `"Relay"`, or `"Leaf"`. */
  role: string;
  /** Parent peer id, or `null` for roots. */
  parent: string | null;
  /** Depth in the tree (0 = root). */
  depth: number;
  /** Children the planner assigned to this peer. */
  intended_children: number;
  /** Cumulative frames this peer has forwarded to its children. */
  forwarded_frames: number;
  /**
   * Per-peer self-reported relay capacity (max children this peer is
   * willing to serve), clamped to the operator's global max-peers.
   * `undefined` when the client did not advertise a value (the planner
   * falls back to the global ceiling for that peer). Session 144.
   */
  capacity?: number;
}

/**
 * Current peer-mesh state. Mirrors `lvqr_admin::MeshState`.
 */
export interface MeshState {
  /** Whether `--mesh-enabled` was configured on the server. */
  enabled: boolean;
  /** Number of peers currently registered with the coordinator. */
  peer_count: number;
  /**
   * Intended offload percentage, 0..=100. Reflects the topology
   * planner's projected fanout, not measured bandwidth savings.
   * Compare against the per-peer `forwarded_frames` values in `peers`
   * for the actual-vs-intended picture.
   */
  offload_percentage: number;
  /**
   * Per-peer intended-vs-actual offload stats. Empty when mesh is
   * disabled. Added in session 141; servers older than that omit the
   * field (TypeScript's structural typing is lenient on extra or
   * missing fields on read).
   */
  peers: MeshPeerStats[];
}

/**
 * One row from the `GET /api/v1/slo` response. Mirrors
 * `lvqr_admin::SloEntry`.
 */
export interface SloEntry {
  /** Broadcast name (e.g. `"live/demo"`). */
  broadcast: string;
  /** Egress surface: `"hls"`, `"ws"`, `"dash"`, `"whep"`, etc. */
  transport: string;
  /** 50th percentile latency in milliseconds across the retained window. */
  p50_ms: number;
  /** 95th percentile latency. */
  p95_ms: number;
  /** 99th percentile latency. */
  p99_ms: number;
  /** Peak observed latency. */
  max_ms: number;
  /** Samples retained in the ring buffer (bounded). */
  sample_count: number;
  /** Total samples ever observed (unbounded). */
  total_observed: number;
}

/**
 * Shape of `GET /api/v1/slo`. The outer object wraps the per-broadcast
 * entries so the response can gain sibling fields without a breaking
 * schema change (matches `lvqr-admin`'s emit: `{ "broadcasts": [...] }`).
 */
export interface SloSnapshot {
  broadcasts: SloEntry[];
}

/**
 * Resource capacity advertisement for one cluster node. Mirrors
 * `lvqr_cluster::NodeCapacity`. Optional on a `ClusterNodeView`
 * because newly-joined nodes may not yet have advertised.
 */
export interface NodeCapacity {
  /** CPU utilization (`0.0..=100.0`, per logical core aggregate). */
  cpu_pct: number;
  /** Resident set size in bytes. */
  rss_bytes: number;
  /** Outbound bandwidth (bytes per second). */
  bytes_out_per_sec: number;
}

/**
 * External-facing view of one cluster member. Mirrors
 * `lvqr_admin::ClusterNodeView`.
 */
export interface ClusterNodeView {
  /** Stringified node id (e.g. `"lvqr-ab12cd34ef56gh78"`). */
  id: string;
  /** Generation counter from chitchat. */
  generation: number;
  /** Stringified gossip socket address (`"10.0.0.1:10007"`). */
  gossip_addr: string;
  /** Most-recent capacity advert, or `null` until the first gossip round. */
  capacity: NodeCapacity | null;
}

/**
 * One broadcast's current owner per LWW tiebreak. Mirrors
 * `lvqr_cluster::BroadcastSummary`.
 */
export interface BroadcastSummary {
  /** Broadcast name without the `broadcast.` prefix. */
  name: string;
  /** Current owner node id (LWW winner across the cluster). */
  owner: string;
  /** Unix ms at which the winning lease expires if not renewed. */
  expires_at_ms: number;
}

/**
 * One cluster-wide config entry. Mirrors `lvqr_cluster::ConfigEntry`.
 */
export interface ConfigEntry {
  /** Config key without the `config.` prefix. */
  key: string;
  /** Current value per cross-node LWW tiebreak. */
  value: string;
  /** Unix ms the winning entry was written with. */
  ts_ms: number;
}

/**
 * Phase of one federation link. Mirrors
 * `lvqr_cluster::FederationConnectState` (serde lowercase).
 */
export type FederationConnectState = 'connecting' | 'connected' | 'failed';

/**
 * External-facing status snapshot for one federation link. Mirrors
 * `lvqr_cluster::FederationLinkStatus`.
 */
export interface FederationLinkStatus {
  /** Remote relay URL as configured. Never carries the token. */
  remote_url: string;
  /** Broadcast names this link forwards (exact-match). */
  forwarded_broadcasts: string[];
  /** Current connection phase. */
  state: FederationConnectState;
  /** Unix ms of last successful connect, or `null` if never. */
  last_connected_at_ms: number | null;
  /** Most-recent error string, or `null` after a successful connect. */
  last_error: string | null;
  /** Total connect attempts since runner startup. */
  connect_attempts: number;
  /** Remote announcements matched since runner startup. */
  forwarded_broadcasts_seen: number;
}

/**
 * Shape of `GET /api/v1/cluster/federation`. Mirrors
 * `lvqr_admin::FederationStatusView`. Empty `links` is returned
 * both when federation is disabled and when no links are
 * configured; the server collapses the distinction deliberately.
 */
export interface FederationStatus {
  links: FederationLinkStatus[];
}

/**
 * Per-`(broadcast, track)` WASM filter counters. Mirrors
 * `lvqr_admin::WasmFilterBroadcastStats`.
 */
export interface WasmFilterBroadcastStats {
  /** Broadcast name (e.g. `"live/cam1"`). */
  broadcast: string;
  /** Track name within the broadcast (e.g. `"0.mp4"`). */
  track: string;
  /** Total fragments observed through the chain (kept + dropped). */
  seen: number;
  /** Fragments the chain returned `Some` for (survived every slot). */
  kept: number;
  /** Fragments a slot in the chain returned `None` for (short-circuit). */
  dropped: number;
}

/**
 * Per-slot WASM filter counters. Mirrors
 * `lvqr_admin::WasmFilterSlotStats`. `index` is the filter's
 * position in the chain (0-based). `seen` / `kept` / `dropped`
 * describe what THAT slot observed -- later slots in a chain show
 * smaller `seen` counts when an earlier slot drops, because the
 * chain short-circuits on the first `None`. PLAN Phase D session
 * 140.
 */
export interface WasmFilterSlotStats {
  /** 0-based position in the configured chain. */
  index: number;
  /** Fragments this slot observed (kept + dropped for this slot). */
  seen: number;
  /** Fragments this slot returned `Some` for. */
  kept: number;
  /** Fragments this slot returned `None` for (short-circuit drop). */
  dropped: number;
}

/**
 * Shape of `GET /api/v1/wasm-filter`. Mirrors
 * `lvqr_admin::WasmFilterState`. When `--wasm-filter` is unset the
 * server returns `{ enabled: false, chain_length: 0, broadcasts: [],
 * slots: [] }` rather than 404 so dashboards can pre-bake the shape.
 */
export interface WasmFilterState {
  /** Whether `--wasm-filter` was configured on the server. */
  enabled: boolean;
  /**
   * Number of filters composed into the chain installed at
   * `lvqr serve` time. Constant for the server's lifetime.
   */
  chain_length: number;
  /** Every `(broadcast, track)` pair the filter tap has observed. */
  broadcasts: WasmFilterBroadcastStats[];
  /**
   * Per-slot counters in insertion order. Contains `chain_length`
   * entries when `enabled` is true; empty otherwise. Added in PLAN
   * Phase D session 140; servers older than that version omit the
   * field, so the type is optional-safe if you are polling a
   * pre-session-140 deployment (TypeScript treats missing fields
   * leniently on reads).
   */
  slots: WasmFilterSlotStats[];
}

/**
 * One stream-key as the admin API returns it. Mirrors
 * `lvqr_auth::StreamKey` (session 146). The token carries the
 * literal bearer credential a publisher uses; operators copy it
 * out of the mint response (or `listStreamKeys()`) and hand it
 * to whatever ingest tool will publish (ffmpeg, OBS, GStreamer).
 *
 * Tokens are formatted as `lvqr_sk_<43-char base64url-no-pad>`
 * (32 bytes of CSPRNG output prefixed for secret-scanning
 * recognisability).
 */
export interface StreamKey {
  /** Stable handle for revoke + rotate. Never changes. */
  id: string;
  /** Bearer credential the publisher sends as the stream key. */
  token: string;
  /** Operator-friendly name, or `null` / absent. */
  label?: string | null;
  /**
   * When set, the key only authorises publishes for this exact
   * broadcast name (e.g. `"live/cam-a"`). Unset means the key
   * accepts any broadcast under the configured ingest surfaces.
   */
  broadcast?: string | null;
  /** Unix seconds at mint or rotate time. */
  created_at: number;
  /**
   * Unix seconds after which the key stops authenticating publishes.
   * Unset means no expiry. Lazy: the auth path filters expired
   * tokens; the list endpoint still surfaces them so operators can
   * revoke stale entries.
   */
  expires_at?: number | null;
}

/**
 * Mint / rotate request body. Mirrors `lvqr_auth::StreamKeySpec`
 * (session 146). Every field is optional; operators only declare
 * the scoping they actually want.
 */
export interface StreamKeySpec {
  label?: string | null;
  broadcast?: string | null;
  /**
   * Time-to-live in seconds. Server converts to
   * `expires_at = now + ttl_seconds`. `0` or absent means no
   * expiry.
   */
  ttl_seconds?: number | null;
}

/**
 * Outer shape of `GET /api/v1/streamkeys`. Mirrors
 * `lvqr_admin::StreamKeyList`. The wrapper exists so the response
 * can grow sibling fields (counts, pagination cursors) without a
 * breaking schema change.
 */
export interface StreamKeyList {
  keys: StreamKey[];
}

/**
 * Wire shape for `GET /api/v1/config-reload` and the success body
 * of `POST /api/v1/config-reload`. Mirrors
 * `lvqr_admin::ConfigReloadStatus` (session 147). Every Optional
 * field carries an explicit nullable shape so SDK clients older
 * than the server keep parsing forward when later sessions add
 * sibling fields.
 */
export interface ConfigReloadStatus {
  /** Resolved path of the configured `--config` file, or `null` when the server booted without `--config`. */
  config_path?: string | null;
  /** Unix milliseconds at the most recent successful reload, or `null` until the first reload succeeds. */
  last_reload_at_ms?: number | null;
  /** `"sighup"`, `"admin_post"`, `"boot"`, or `null` when no reload has occurred yet. */
  last_reload_kind?: string | null;
  /** Keys the most recent reload effectively re-applied. Currently always `["auth"]` on success. */
  applied_keys?: string[];
  /** Operator-facing warnings -- e.g. structural-key diffs that require a server restart. */
  warnings?: string[];
}

export interface LvqrAdminClientOptions {
  /**
   * Per-request deadline in milliseconds. Applied to every admin
   * HTTP call so a misbehaving server that accepts the TCP
   * connection but never responds does not hang the Promise
   * forever. Defaults to 10_000 (10 s). Set to 0 to disable (not
   * recommended for production).
   */
  fetchTimeoutMs?: number;
  /**
   * Optional bearer token. When set, every admin fetch sends
   * `Authorization: Bearer <token>`. Required when the server
   * was booted with `--admin-token` or a JWT provider.
   */
  bearerToken?: string;
}

const DEFAULT_FETCH_TIMEOUT_MS = 10_000;

/**
 * Client for the LVQR admin HTTP API.
 *
 * Covers every route the admin router mounts today:
 * `/healthz`, `/api/v1/{stats,streams,mesh,slo,wasm-filter}`, and the
 * cluster-gated `/api/v1/cluster/{nodes,broadcasts,config,federation}`.
 *
 * @example
 * ```ts
 * const admin = new LvqrAdminClient('http://localhost:8080', {
 *   bearerToken: 'secret',
 *   fetchTimeoutMs: 5_000,
 * });
 * const streams = await admin.listStreams();
 * const slo = await admin.slo();
 * const nodes = await admin.clusterNodes();
 * ```
 */
export class LvqrAdminClient {
  private baseUrl: string;
  private options: LvqrAdminClientOptions;

  constructor(baseUrl: string, options: LvqrAdminClientOptions = {}) {
    this.baseUrl = baseUrl.replace(/\/$/, '');
    this.options = options;
  }

  /** Check if the relay is healthy. Returns `false` on any non-2xx or network error. */
  async healthz(): Promise<boolean> {
    try {
      const resp = await this.fetchWithTimeout(`${this.baseUrl}/healthz`);
      return resp.ok;
    } catch {
      return false;
    }
  }

  /** `GET /api/v1/stats` -- aggregate relay statistics. */
  async stats(): Promise<RelayStats> {
    return this.getJson<RelayStats>('/api/v1/stats');
  }

  /** `GET /api/v1/streams` -- list of active broadcasts. */
  async listStreams(): Promise<StreamInfo[]> {
    return this.getJson<StreamInfo[]>('/api/v1/streams');
  }

  /** `GET /api/v1/mesh` -- current peer-mesh state. */
  async mesh(): Promise<MeshState> {
    return this.getJson<MeshState>('/api/v1/mesh');
  }

  /**
   * `GET /api/v1/slo` -- per-broadcast + per-transport latency
   * snapshot. The response wraps the entries in an object so
   * callers can distinguish "no tracker wired" (`broadcasts: []`)
   * from "tracker configured but no samples" (also `[]`, but the
   * route still returns 200).
   */
  async slo(): Promise<SloSnapshot> {
    return this.getJson<SloSnapshot>('/api/v1/slo');
  }

  /**
   * `GET /api/v1/cluster/nodes` -- live cluster members.
   * Requires the server to be built with `--features cluster`
   * (on by default) and `--cluster-listen` to be set. Throws
   * `HTTP 500` if the route is reachable but no `Cluster` handle
   * is wired.
   */
  async clusterNodes(): Promise<ClusterNodeView[]> {
    return this.getJson<ClusterNodeView[]>('/api/v1/cluster/nodes');
  }

  /** `GET /api/v1/cluster/broadcasts` -- active broadcast leases. */
  async clusterBroadcasts(): Promise<BroadcastSummary[]> {
    return this.getJson<BroadcastSummary[]>('/api/v1/cluster/broadcasts');
  }

  /** `GET /api/v1/cluster/config` -- cluster-wide LWW config entries. */
  async clusterConfig(): Promise<ConfigEntry[]> {
    return this.getJson<ConfigEntry[]>('/api/v1/cluster/config');
  }

  /**
   * `GET /api/v1/cluster/federation` -- status of every configured
   * federation link. Returns `{ links: [] }` both when federation is
   * disabled and when no links are configured; the server collapses
   * the distinction deliberately so tooling can poll
   * unconditionally.
   */
  async clusterFederation(): Promise<FederationStatus> {
    return this.getJson<FederationStatus>('/api/v1/cluster/federation');
  }

  /**
   * `GET /api/v1/wasm-filter` -- configured WASM filter chain shape +
   * per-(broadcast, track) seen/kept/dropped counters. Returns
   * `{ enabled: false, chain_length: 0, broadcasts: [] }` when
   * `--wasm-filter` is unset; tooling can poll unconditionally.
   */
  async wasmFilter(): Promise<WasmFilterState> {
    return this.getJson<WasmFilterState>('/api/v1/wasm-filter');
  }

  /**
   * `GET /api/v1/streamkeys` -- every stream-key currently in the
   * runtime store, including expired entries. Returns
   * `{ keys: [] }` when the server booted with `--no-streamkeys`
   * (so polling tooling can run unconditionally). Session 146.
   */
  async listStreamKeys(): Promise<StreamKey[]> {
    const wrapper = await this.getJson<StreamKeyList>('/api/v1/streamkeys');
    return wrapper.keys ?? [];
  }

  /**
   * `POST /api/v1/streamkeys` -- mint a new stream-key. Server
   * fills `id`, `token`, `created_at`, and `expires_at` (from
   * `ttl_seconds`). Returns the full minted key including the
   * bearer token literal. Session 146.
   */
  async mintStreamKey(spec: StreamKeySpec = {}): Promise<StreamKey> {
    return this.sendJson<StreamKey>('POST', '/api/v1/streamkeys', spec);
  }

  /**
   * `DELETE /api/v1/streamkeys/{id}` -- hard-delete by id.
   * Resolves on 204 (success) and rejects on 404 (unknown id) or
   * any other non-2xx status. Idempotent callers should treat
   * 404 as "already gone" by catching the rejection. Session 146.
   */
  async revokeStreamKey(id: string): Promise<void> {
    const resp = await this.fetchWithTimeout(
      `${this.baseUrl}/api/v1/streamkeys/${encodeURIComponent(id)}`,
      { method: 'DELETE' },
    );
    if (!resp.ok) {
      throw new Error(`DELETE /api/v1/streamkeys/${id}: HTTP ${resp.status} ${resp.statusText}`);
    }
  }

  /**
   * `POST /api/v1/streamkeys/{id}/rotate` -- swap the token while
   * preserving `id`. With no `override` argument the existing
   * `label` / `broadcast` / `expires_at` are preserved; passing
   * an override `StreamKeySpec` re-scopes the key while rotating
   * (a `null` field on the override CLEARS the existing field).
   * Returns the rotated key with the new token. Session 146.
   */
  async rotateStreamKey(id: string, override?: StreamKeySpec): Promise<StreamKey> {
    return this.sendJson<StreamKey>(
      'POST',
      `/api/v1/streamkeys/${encodeURIComponent(id)}/rotate`,
      override,
    );
  }

  /**
   * `GET /api/v1/config-reload` -- status of the configured
   * `--config` file (path, last reload timestamp + kind, applied
   * keys, warnings). Returns a default-shaped body when the
   * server booted without `--config`. Session 147.
   */
  async configReload(): Promise<ConfigReloadStatus> {
    return this.getJson<ConfigReloadStatus>('/api/v1/config-reload');
  }

  /**
   * `POST /api/v1/config-reload` -- trigger a reload of the
   * configured `--config` file. Resolves with the new
   * `ConfigReloadStatus` on success. Rejects with a 503 error
   * when the server booted without `--config`, or 500 when the
   * file is malformed / the rebuild failed (in which case the
   * prior provider stays live). Session 147.
   */
  async triggerConfigReload(): Promise<ConfigReloadStatus> {
    const resp = await this.fetchWithTimeout(`${this.baseUrl}/api/v1/config-reload`, {
      method: 'POST',
    });
    if (!resp.ok) {
      throw new Error(`POST /api/v1/config-reload: HTTP ${resp.status} ${resp.statusText}`);
    }
    return (await resp.json()) as ConfigReloadStatus;
  }

  /**
   * Shared JSON-body helper for POST + PATCH-like routes. Sends
   * `JSON.stringify(body)` when `body` is provided, or a
   * zero-length body otherwise (the rotate route specifically
   * uses the empty-body branch as "no override -- preserve scope").
   */
  private async sendJson<T>(method: string, path: string, body?: unknown): Promise<T> {
    const init: RequestInit = { method };
    if (body !== undefined) {
      init.body = JSON.stringify(body);
      init.headers = { 'Content-Type': 'application/json' };
    }
    const resp = await this.fetchWithTimeout(`${this.baseUrl}${path}`, init);
    if (!resp.ok) {
      throw new Error(`${method} ${path}: HTTP ${resp.status} ${resp.statusText}`);
    }
    return (await resp.json()) as T;
  }

  /**
   * Shared JSON GET helper. Applies the configured bearer token +
   * fetch timeout, throws a descriptive `Error` on non-2xx, and
   * returns the parsed body cast to `T`.
   */
  private async getJson<T>(path: string): Promise<T> {
    const resp = await this.fetchWithTimeout(`${this.baseUrl}${path}`);
    if (!resp.ok) {
      throw new Error(`GET ${path}: HTTP ${resp.status} ${resp.statusText}`);
    }
    return (await resp.json()) as T;
  }

  /**
   * `fetch` wrapper that enforces the configured fetch timeout via
   * an AbortController + injects the bearer header when configured.
   * A timeout cancels the in-flight request and rejects with an
   * AbortError so callers can distinguish timeout from network
   * failure via `e.name === 'AbortError'`.
   */
  private async fetchWithTimeout(url: string, init: RequestInit = {}): Promise<Response> {
    const timeoutMs = this.options.fetchTimeoutMs ?? DEFAULT_FETCH_TIMEOUT_MS;
    const headers = new Headers(init.headers);
    if (this.options.bearerToken) {
      headers.set('Authorization', `Bearer ${this.options.bearerToken}`);
    }
    const base: RequestInit = { ...init, headers };
    if (timeoutMs <= 0) {
      return fetch(url, base);
    }
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      return await fetch(url, { ...base, signal: controller.signal });
    } finally {
      clearTimeout(timer);
    }
  }
}
