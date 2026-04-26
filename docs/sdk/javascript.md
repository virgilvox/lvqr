# JavaScript SDK

Three npm packages for browser integration:

- `@lvqr/core` -- Low-level client library (WebTransport + WebSocket
  fMP4 fallback, admin API client, WebRTC DataChannel peer mesh)
- `@lvqr/player` -- Drop-in `<lvqr-player>` Web Component for the
  MoQ-Lite over WebTransport / WebSocket live path
- `@lvqr/dvr-player` -- Drop-in `<lvqr-dvr-player>` Web Component
  for HLS DVR scrub (custom seek bar, LIVE pill, Go Live button,
  client-side hover thumbnails, SCTE-35 ad-break markers). Wraps
  hls.js against the relay's live HLS endpoint with the
  `--hls-dvr-window` sliding-window DVR depth. See
  [`../dvr-scrub.md`](../dvr-scrub.md) for the operator embedding
  recipe.

`@lvqr/core` and `@lvqr/player` ship at `0.3.2`; `@lvqr/dvr-player`
ships at `0.3.3` (session 154 added the SCTE-35 marker layer; the
two other packages stayed at 0.3.2). Features added on `main`
after the last publish (listed below under **Timeouts + reconnect**
and **Admin API**) land for consumers at the next release cycle.

## Install

```bash
npm install @lvqr/core
# or for the live-only player component:
npm install @lvqr/player
# or for the HLS DVR scrub component:
npm install @lvqr/dvr-player
```

## Player component (simplest)

```html
<script type="module">
  import '@lvqr/player';
</script>

<lvqr-player
  src="https://relay.example.com:4443/live/my-stream"
  autoplay
  muted
></lvqr-player>
```

### Attributes

| Attribute | Description |
|-----------|-------------|
| `src` | Relay URL with stream path (required) |
| `autoplay` | Start playback on load |
| `muted` | Start muted |
| `fingerprint` | TLS cert fingerprint (development only) |

### CSS parts

```css
lvqr-player::part(video) { /* style the video element */ }
lvqr-player::part(status) { /* style the status overlay */ }
```

## DVR scrub component

For the HLS DVR scrub experience -- pause, scrub through a
sliding window of the broadcast, jump to live -- use
`@lvqr/dvr-player` against the relay's live HLS endpoint:

```html
<script type="module">
  import '@lvqr/dvr-player';
</script>

<lvqr-dvr-player
  src="https://relay.example.com:8080/hls/live/cam1/master.m3u8"
  token="<bearer-token-or-omit>"
  autoplay
  muted
></lvqr-dvr-player>
```

### Attributes

| Attribute | Description |
|-----------|-------------|
| `src` | Master playlist URL (required) |
| `autoplay` | Start playback when manifest is parsed |
| `muted` | Start muted (required for autoplay) |
| `token` | Bearer token forwarded as `Authorization: Bearer` |
| `live-edge-threshold-secs` | Live-edge detection threshold (default `max(6, 3 * #EXT-X-TARGETDURATION)`) |
| `thumbnails` | `enabled` (default) or `disabled` |
| `controls` | `custom` (default) or `native` |
| `markers` | `visible` (default) or `hidden` -- SCTE-35 ad-break markers on the seek bar (v0.3.3+) |

### Custom events

```typescript
player.addEventListener('lvqr-dvr-seek', (e) => {
  const { fromTime, toTime, isLiveEdge, source } = e.detail;
});
player.addEventListener('lvqr-dvr-live-edge-changed', (e) => {
  const { isAtLiveEdge, deltaSecs, thresholdSecs } = e.detail;
});
player.addEventListener('lvqr-dvr-error', (e) => {
  const { code, message, fatal, source } = e.detail;
});

// SCTE-35 ad-break markers (v0.3.3+).
player.addEventListener('lvqr-dvr-markers-changed', (e) => {
  const { markers, pairs } = e.detail;
});
player.addEventListener('lvqr-dvr-marker-crossed', (e) => {
  const { marker, direction, currentTime } = e.detail;
});
```

See [`../dvr-scrub.md`](../dvr-scrub.md) for the full surface
including the programmatic API
(`play / pause / seek / goLive / getHlsInstance / getMarkers`),
CSS theming, the SCTE-35 marker recipe, and the importmap-based
CDN drop-in.

## Core client (low-level)

```typescript
import { LvqrClient } from '@lvqr/core';

const client = new LvqrClient('https://relay.example.com:4443', {
  fingerprint: 'aa:bb:cc:...', // optional, for self-signed certs
  connectTimeoutMs: 5_000,     // default 10_000; see below
});

client.on('connected', () => console.log('connected'));
client.on('frame', (data: Uint8Array, track: string) => {
  // Feed to MediaSource, WebCodecs, or canvas
});
client.on('error', (err) => console.error(err));

await client.connect();
await client.subscribe('live/my-stream');

// Later:
client.close();
```

## Admin client

`LvqrAdminClient` covers every `/api/v1/*` route the admin router
mounts today (9 methods; see below).

```typescript
import { LvqrAdminClient } from '@lvqr/core';

const admin = new LvqrAdminClient('http://localhost:8080', {
  fetchTimeoutMs: 5_000,       // default 10_000; see below
  bearerToken: process.env.LVQR_ADMIN_TOKEN, // optional
});

if (await admin.healthz()) {
  const stats = await admin.stats();
  console.log(`${stats.tracks} tracks, ${stats.subscribers} subscribers`);

  const streams = await admin.listStreams();
  for (const s of streams) {
    console.log(`${s.name}: ${s.subscribers} viewers`);
  }
}
```

### Method reference

| Method | Route | Returns | Notes |
|---|---|---|---|
| `healthz()` | `GET /healthz` | `Promise<boolean>` | `false` on any non-2xx or network error. |
| `stats()` | `GET /api/v1/stats` | `Promise<RelayStats>` | Aggregate counters. |
| `listStreams()` | `GET /api/v1/streams` | `Promise<StreamInfo[]>` | One entry per active broadcast. |
| `mesh()` | `GET /api/v1/mesh` | `Promise<MeshState>` | Peer-mesh state; `enabled === false` when the server ran without `--mesh-enabled`. |
| `slo()` | `GET /api/v1/slo` | `Promise<SloSnapshot>` | Wraps the per-broadcast-per-transport latency entries in `{ broadcasts: [...] }`. |
| `clusterNodes()` | `GET /api/v1/cluster/nodes` | `Promise<ClusterNodeView[]>` | Requires server built with `--features cluster` (on by default) + `--cluster-listen` set. Throws `HTTP 500` when the feature is on but no `Cluster` handle is wired. |
| `clusterBroadcasts()` | `GET /api/v1/cluster/broadcasts` | `Promise<BroadcastSummary[]>` | Active broadcast leases, non-expired, LWW winner per name. |
| `clusterConfig()` | `GET /api/v1/cluster/config` | `Promise<ConfigEntry[]>` | Cluster-wide LWW config KV. |
| `clusterFederation()` | `GET /api/v1/cluster/federation` | `Promise<FederationStatus>` | Wraps per-link status in `{ links: [...] }`. Empty list means "federation disabled OR no links configured" (the server collapses the distinction deliberately). |
| `wasmFilter()` | `GET /api/v1/wasm-filter` | `Promise<WasmFilterState>` | Configured WASM filter chain shape + per-`(broadcast, track)` seen/kept/dropped counters. Returns `{enabled: false, chain_length: 0, broadcasts: []}` when `--wasm-filter` is unset (200 OK, not 404); tooling can poll unconditionally. |

### Response type reference

```typescript
interface RelayStats {
  publishers: number;
  subscribers: number;
  tracks: number;
  bytes_received: number;
  bytes_sent: number;
  uptime_secs: number;
}

interface StreamInfo {
  name: string;
  subscribers: number;
}

interface MeshState {
  enabled: boolean;
  peer_count: number;
  offload_percentage: number; // intended offload, not measured
  peers: MeshPeerStats[];     // per-peer intended-vs-actual, session 141
}

interface MeshPeerStats {
  peer_id: string;
  role: string;               // "Root" | "Relay" | "Leaf"
  parent: string | null;
  depth: number;
  intended_children: number;  // from the topology planner
  forwarded_frames: number;   // from the peer's ForwardReport
  capacity?: number;          // self-reported relay cap (session 144);
                              // undefined when the client did not advertise
}

interface SloEntry {
  broadcast: string;
  transport: string; // "hls" | "dash" | "ws" | "whep" ...
  p50_ms: number;
  p95_ms: number;
  p99_ms: number;
  max_ms: number;
  sample_count: number;   // bounded to MAX_SAMPLES_PER_KEY
  total_observed: number; // unbounded
}

interface SloSnapshot {
  broadcasts: SloEntry[];
}

interface NodeCapacity {
  cpu_pct: number;              // 0.0..=100.0 per logical core
  rss_bytes: number;
  bytes_out_per_sec: number;
}

interface ClusterNodeView {
  id: string;
  generation: number;
  gossip_addr: string;          // "10.0.0.1:10007"
  capacity: NodeCapacity | null;
}

interface BroadcastSummary {
  name: string;
  owner: string;                // node id of the LWW winner
  expires_at_ms: number;
}

interface ConfigEntry {
  key: string;
  value: string;
  ts_ms: number;
}

type FederationConnectState = 'connecting' | 'connected' | 'failed';

interface FederationLinkStatus {
  remote_url: string;
  forwarded_broadcasts: string[];
  state: FederationConnectState;
  last_connected_at_ms: number | null;
  last_error: string | null;
  connect_attempts: number;
  forwarded_broadcasts_seen: number;
}

interface FederationStatus {
  links: FederationLinkStatus[];
}

interface WasmFilterBroadcastStats {
  broadcast: string;      // "live/cam1"
  track: string;          // "0.mp4"
  seen: number;           // kept + dropped
  kept: number;           // survived every slot in the chain
  dropped: number;        // a slot returned None (short-circuit)
}

interface WasmFilterSlotStats {
  index: number;          // 0-based position in the chain
  seen: number;           // fragments THIS slot observed
  kept: number;           // fragments this slot returned Some for
  dropped: number;        // fragments this slot returned None for
}

interface WasmFilterState {
  enabled: boolean;       // mirrors whether --wasm-filter was configured
  chain_length: number;   // constant for the server's lifetime
  broadcasts: WasmFilterBroadcastStats[];
  slots: WasmFilterSlotStats[];  // per-slot counters, added in session 140
}
```

## Timeouts + reconnect

Both `LvqrClient` and `LvqrAdminClient` enforce a per-operation
deadline so a misbehaving server (TCP accepts but never responds)
cannot hang a `Promise` forever.

### `LvqrClient.connectTimeoutMs`

Applied to the WebTransport + WebSocket + WebSocket-broadcast
connect paths via a shared `withConnectTimeout` helper that
closes the in-flight handshake on timeout. Defaults to `10_000`
(10 s). Set to `0` or omit the option to use the default; there
is no way to disable the timeout entirely, because the
connect path has no useful "never time out" semantics in
practice.

```typescript
const client = new LvqrClient('https://relay.example.com:4443', {
  connectTimeoutMs: 5_000,
});

try {
  await client.connect();
} catch (err) {
  // err.name === 'AbortError' when the timeout fired;
  // err.name === 'NetworkError' on a real network-layer refusal.
}
```

### `LvqrAdminClient.fetchTimeoutMs`

Applied to every admin HTTP call via an `AbortController`.
Defaults to `10_000`. Set to `0` to disable the timer (not
recommended for production -- a wedged admin endpoint becomes
a wedged application).

```typescript
const admin = new LvqrAdminClient('http://localhost:8080', {
  fetchTimeoutMs: 3_000, // stricter for a health dashboard
});

try {
  const stats = await admin.stats();
} catch (err) {
  if ((err as Error).name === 'AbortError') {
    // Timeout fired. Backoff or fall back.
  }
}
```

### `LvqrAdminClientOptions.bearerToken`

When the server runs with `--admin-token <T>` or
`--jwt-secret <S>`, every `/api/v1/*` route is auth-gated. Set
`bearerToken` on the client so every fetch carries
`Authorization: Bearer <T>`. Omitting the option is fine for
open-access deployments (no token configured or
`NoopAuthProvider`).

```typescript
const admin = new LvqrAdminClient('http://localhost:8080', {
  bearerToken: process.env.LVQR_ADMIN_TOKEN,
});
```

### Reconnect recipe

`LvqrClient.connect()` + `LvqrClient.subscribe()` do **NOT**
automatically reconnect. The SDK leaves the reconnect policy
to the caller so operator code can choose the right backoff
for its environment (jittered exponential backoff for public
deployments; fixed-interval polling for lab setups; bounded
retry count for CI drivers). A canonical recipe:

```typescript
import { LvqrClient } from '@lvqr/core';

async function runWithReconnect(
  url: string,
  broadcast: string,
  onFrame: (data: Uint8Array, track: string) => void,
): Promise<void> {
  let attempt = 0;
  for (;;) {
    const client = new LvqrClient(url, { connectTimeoutMs: 5_000 });
    client.on('frame', onFrame);

    try {
      await client.connect();
      attempt = 0; // reset backoff on successful connect
      await client.subscribe(broadcast);
      // Wait on a close signal. Adapt to your app's
      // cancellation mechanism; this example blocks forever.
      await new Promise<void>((resolve, reject) => {
        client.on('error', reject);
      });
    } catch (err) {
      console.warn('lvqr client dropped:', err);
    } finally {
      client.close();
    }

    attempt++;
    const delayMs = Math.min(30_000, 500 * 2 ** Math.min(attempt, 6));
    const jitter = Math.floor(Math.random() * (delayMs / 4));
    await new Promise((r) => setTimeout(r, delayMs + jitter));
  }
}
```

Tune the floor (`500` ms) and the ceiling (`30_000` ms) to
match your deployment. A single transient network blip should
not push a page to a 30 s backoff; a sustained outage should
not send reconnect storms at the server.

### Admin-side retries

`LvqrAdminClient` calls are idempotent GETs, so retrying a
failed call is always safe. When `fetchTimeoutMs` fires or a
transient network error surfaces, retry with a capped
exponential backoff:

```typescript
async function withRetry<T>(
  fn: () => Promise<T>,
  maxAttempts = 4,
): Promise<T> {
  let attempt = 0;
  for (;;) {
    try {
      return await fn();
    } catch (err) {
      attempt++;
      if (attempt >= maxAttempts) throw err;
      const delayMs = Math.min(5_000, 200 * 2 ** attempt);
      await new Promise((r) => setTimeout(r, delayMs));
    }
  }
}

const stats = await withRetry(() => admin.stats());
```

## Transport detection

```typescript
import { detectTransport } from '@lvqr/core';

const transport = detectTransport();
// 'webtransport' | 'websocket' | 'none'
```

## Peer mesh

`MeshPeer` connects to the server's `/signal` WebSocket, opens
an `RTCPeerConnection` to an assigned parent peer, and relays
incoming DataChannel frames to its own children. The mesh data
plane is fully implemented as of session 144: two-browser E2E
(115), actual-vs-intended offload reporting (141), three-peer
Playwright matrix (142), TURN recipe with server-driven ICE
config (143), and per-peer capacity advertisement (144) all
ship.

**Actual-vs-intended offload reporting** shipped in session 141.
`MeshPeer` maintains a private cumulative forwarded-frame
counter and emits a `ForwardReport` signal message every
second (skip-on-unchanged, so idle peers stay silent). Read the
count locally via `peer.forwardedFrameCount`; read the
server-aggregated values via `adminClient.mesh()` which surfaces
a `peers: MeshPeerStats[]` array with `intended_children`
(topology planner) and `forwarded_frames` (client report) per
peer.

Read the assigned parent via `peer.parentPeerId` (added in
session 142 alongside the three-peer Playwright matrix). Returns
`null` for Root peers and for peers that have not yet received an
`AssignParent` message; once the assignment lands, the value is
the parent's peer_id.

**Per-peer capacity advertisement** shipped in session 144. Pass
`MeshConfig.capacity?: number` to declare the maximum children
this peer is willing to relay to. The server clamps the claim to
its operator-configured global `--max-peers` ceiling, so a client
cannot exceed the operator's limit even by mis-claiming. Omit the
field to fall back to the global default. Common values:
`capacity: 0` for known-mobile peers (do not relay), `capacity: 5`
for known-laptop peers, omitted for "let the server decide".

```typescript
import { MeshPeer } from '@lvqr/core';

const peer = new MeshPeer({
  signalUrl: 'wss://relay.example.com:8080/signal',
  peerId: 'peer-one',
  iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
  // Session 144: serve up to 3 children. Omit for the
  // operator's global default.
  capacity: 3,
  // Fires once per child when its DataChannel opens on the
  // parent side. Use for deterministic one-shot push (e.g.
  // init segment for a late-joining subscriber).
  onChildOpen: (childId, dc) => {
    console.log(`mesh child opened: ${childId}`);
    // dc.send(initSegmentBytes); // optional
  },
  onFrame: (data, fromPeer) => {
    // Received a MoQ frame over the peer mesh.
  },
});

await peer.connect();

// Root peer: inject media received from the server into the
// mesh tree.
peer.pushFrame(new Uint8Array([0x01, 0x02, 0x03]));

// Local cumulative forwarded-frame count (also reported to the
// server every second for /api/v1/mesh surfacing).
console.log(`forwarded so far: ${peer.forwardedFrameCount}`);

// Later:
peer.close();
```

## Stream-key CRUD (session 146, `main`)

`LvqrAdminClient` exposes runtime CRUD over the server's
`/api/v1/streamkeys` admin routes. Operators mint, list, revoke,
and rotate stream keys without bouncing the relay.

```typescript
import { LvqrAdminClient, type StreamKey } from '@lvqr/core';

const admin = new LvqrAdminClient('http://localhost:8080', {
  bearerToken: 'admin-secret',
});

// Mint -- server fills id, token, created_at.
const key: StreamKey = await admin.mintStreamKey({
  label: 'camera-a',
  broadcast: 'live/cam-a',
  ttl_seconds: 3600, // optional; omit for no expiry
});
console.log(key.token); // "lvqr_sk_<43-char base64url-no-pad>"

// List -- includes expired entries so operators can clean up.
const keys = await admin.listStreamKeys();

// Rotate -- preserves id, swaps token. Empty argument keeps scope;
// passing an override re-scopes while rotating.
const rotated = await admin.rotateStreamKey(key.id);

// Revoke -- 204 on success. Idempotent callers catch on rejection
// to mean "already gone".
await admin.revokeStreamKey(key.id);
```

`StreamKey` and `StreamKeySpec` are exported from `@lvqr/core` and
mirror the `lvqr_auth::StreamKey` / `StreamKeySpec` Rust types
byte-for-byte. Tokens carry the typed prefix `lvqr_sk_` (industry
convention -- secret-scanners can recognise leaked LVQR keys in
public commits).

## WASM module

The `@lvqr/core` package includes a WASM module compiled from
Rust for performance-critical operations. You can access it
directly:

```typescript
import init, { LvqrSubscriber, isWebTransportSupported } from '@lvqr/core/wasm';

await init();
console.log(isWebTransportSupported());

const sub = new LvqrSubscriber('https://relay.example.com:4443');
await sub.connect();
```
