/**
 * @lvqr/core - LVQR client library
 *
 * Connects to an LVQR relay and subscribes to live video streams
 * using the MoQ-Lite protocol over WebTransport.
 *
 * @example
 * ```ts
 * import { LvqrClient } from '@lvqr/core';
 *
 * const client = new LvqrClient('https://relay.example.com:4443');
 * await client.connect();
 *
 * client.on('frame', (data: Uint8Array, track: string) => {
 *   // data is fMP4: feed to MSE SourceBuffer
 *   sourceBuffer.appendBuffer(data);
 * });
 *
 * await client.subscribe('live/my-stream');
 * ```
 */

export { LvqrClient, type LvqrClientOptions, type LvqrEvents } from './client';
export {
  LvqrAdminClient,
  type LvqrAdminClientOptions,
  type RelayStats,
  type StreamInfo,
  type MeshState,
  type SloEntry,
  type SloSnapshot,
  type NodeCapacity,
  type ClusterNodeView,
  type BroadcastSummary,
  type ConfigEntry,
  type FederationConnectState,
  type FederationLinkStatus,
  type FederationStatus,
} from './admin';
export { detectTransport, type TransportType } from './transport';
export { MoqSubscriber } from './moq';
export { MeshPeer, type MeshConfig } from './mesh';
