# JavaScript SDK

Two npm packages for browser integration:

- `@lvqr/core` - Low-level client library (WebTransport + WebSocket)
- `@lvqr/player` - Drop-in `<lvqr-player>` Web Component

## Install

```bash
npm install @lvqr/core
# or for the player component:
npm install @lvqr/player
```

## Player Component (Simplest)

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
| `fingerprint` | TLS cert fingerprint (development) |

### CSS Parts

```css
lvqr-player::part(video) { /* style the video element */ }
lvqr-player::part(status) { /* style the status overlay */ }
```

## Core Client (Low-Level)

```typescript
import { LvqrClient } from '@lvqr/core';

const client = new LvqrClient('https://relay.example.com:4443', {
  fingerprint: 'aa:bb:cc:...', // optional, for self-signed certs
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

## Admin Client

```typescript
import { LvqrAdminClient } from '@lvqr/core';

const admin = new LvqrAdminClient('http://localhost:8080');

if (await admin.healthz()) {
  const stats = await admin.stats();
  console.log(`${stats.tracks} tracks, ${stats.subscribers} subscribers`);

  const streams = await admin.listStreams();
  streams.forEach(s => console.log(`${s.name}: ${s.subscribers} viewers`));
}
```

## Transport Detection

```typescript
import { detectTransport } from '@lvqr/core';

const transport = detectTransport();
// 'webtransport' | 'websocket' | 'none'
```

## WASM Module

The `@lvqr/core` package includes a WASM module compiled from Rust for performance-critical operations. You can access it directly:

```typescript
import init, { LvqrSubscriber, isWebTransportSupported } from '@lvqr/core/wasm';

await init();
console.log(isWebTransportSupported());

const sub = new LvqrSubscriber('https://relay.example.com:4443');
await sub.connect();
```
