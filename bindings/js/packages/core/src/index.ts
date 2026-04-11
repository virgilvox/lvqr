/**
 * @lvqr/core - LVQR client library
 *
 * Connects to an LVQR relay and subscribes to live video streams.
 * Uses WebTransport when available, falls back to WebSocket.
 *
 * @example
 * ```ts
 * import { LvqrClient } from '@lvqr/core';
 *
 * const client = new LvqrClient('https://relay.example.com:4443');
 * await client.connect();
 *
 * client.on('frame', (data: Uint8Array) => {
 *   // Feed to MediaSource, WebCodecs, or canvas
 * });
 *
 * await client.subscribe('live/my-stream');
 * ```
 */

export { LvqrClient, type LvqrClientOptions, type LvqrEvents } from './client';
export { LvqrAdminClient, type StreamInfo, type RelayStats } from './admin';
export { detectTransport, type TransportType } from './transport';
