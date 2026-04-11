/**
 * Transport detection and negotiation.
 */

export type TransportType = 'webtransport' | 'websocket' | 'none';

/**
 * Detect the best available transport in the current environment.
 */
export function detectTransport(): TransportType {
  if (typeof globalThis !== 'undefined' && 'WebTransport' in globalThis) {
    return 'webtransport';
  }
  if (typeof globalThis !== 'undefined' && 'WebSocket' in globalThis) {
    return 'websocket';
  }
  return 'none';
}
