import type { ConnectionProfile } from '@/stores/connection';

/**
 * Default ports each LVQR protocol binds to when no `--<proto>-port` flag is
 * passed. Mirrors `crates/lvqr-cli/src/lib.rs` defaults; matches the README
 * "Quickstart" recipes.
 */
export const DEFAULT_PROTOCOL_PORTS = {
  rtmp: 1935,
  whip: 8443,
  // WHEP gets its own axum listener (separate from WHIP) so it cannot
  // share 8443 with WHIP. 8444 is the adjacent-port convention and what
  // the lvqr-cli composition root uses in its examples + the unit-test
  // suite asserts whip != whep so this cannot silently regress.
  whep: 8444,
  hls: 8888,
  dash: 8889,
  srt: 9000,
  rtsp: 8554,
  moq: 4443,
  // WebSocket fMP4 + admin share the same listener as the admin port; no
  // dedicated default port.
  ws: undefined as number | undefined,
} as const;

/**
 * Per-protocol port overrides on a connection profile. Optional; undefined
 * fields fall back to the default ports above.
 */
export interface ProtocolPorts {
  rtmpPort?: number;
  whipPort?: number;
  whepPort?: number;
  hlsPort?: number;
  dashPort?: number;
  srtPort?: number;
  rtspPort?: number;
  moqPort?: number;
}

/**
 * Extract the host (no port) from a profile's admin baseUrl. URL-parsing
 * failures fall back to a literal `localhost` so the recipes still render
 * something usable in degenerate cases.
 */
export function profileHost(profile: Pick<ConnectionProfile, 'baseUrl'>): string {
  try {
    return new URL(profile.baseUrl).hostname || 'localhost';
  } catch {
    return 'localhost';
  }
}

/** Same idea but returns the URL's protocol scheme (`http:`, `https:`). */
export function profileScheme(profile: Pick<ConnectionProfile, 'baseUrl'>): string {
  try {
    return new URL(profile.baseUrl).protocol;
  } catch {
    return 'http:';
  }
}

/** Resolve the WebRTC scheme that pairs with the admin URL: https -> wss, http -> ws. */
export function wsScheme(profile: Pick<ConnectionProfile, 'baseUrl'>): 'ws:' | 'wss:' {
  return profileScheme(profile) === 'https:' ? 'wss:' : 'ws:';
}

/**
 * Build the publish + subscribe URLs for a broadcast against a connection
 * profile. Pure function; returns a fully populated record so view code can
 * splice fields into copy-buttons without nullable handling.
 */
export interface BroadcastUrls {
  publish: {
    rtmp: string;
    whip: string;
    srt: string;
    rtsp: string;
  };
  subscribe: {
    moq: string;
    whep: string;
    hls: string;
    dash: string;
    ws: string;
  };
  embed: {
    lvqrPlayer: string;
    lvqrDvrPlayer: string;
  };
}

export function broadcastUrls(
  profile: ConnectionProfile,
  broadcast: string,
  bearerToken?: string,
): BroadcastUrls {
  const host = profileHost(profile);
  const scheme = profileScheme(profile);
  const httpScheme = scheme === 'https:' ? 'https' : 'http';
  const wssOrWs = wsScheme(profile);
  const ports = profile as ConnectionProfile & ProtocolPorts;
  const enc = encodeURIComponent;

  const rtmpPort = ports.rtmpPort ?? DEFAULT_PROTOCOL_PORTS.rtmp;
  const whipPort = ports.whipPort ?? DEFAULT_PROTOCOL_PORTS.whip;
  const whepPort = ports.whepPort ?? DEFAULT_PROTOCOL_PORTS.whep;
  const hlsPort = ports.hlsPort ?? DEFAULT_PROTOCOL_PORTS.hls;
  const dashPort = ports.dashPort ?? DEFAULT_PROTOCOL_PORTS.dash;
  const srtPort = ports.srtPort ?? DEFAULT_PROTOCOL_PORTS.srt;
  const rtspPort = ports.rtspPort ?? DEFAULT_PROTOCOL_PORTS.rtsp;
  const moqPort = ports.moqPort ?? DEFAULT_PROTOCOL_PORTS.moq;

  // RTMP carries the publish key as the path tail; this matches LVQR's
  // ingest convention (`rtmp://<host>:<port>/live/<key>`). When a token is
  // present we substitute it in for the key segment.
  const rtmpKey = bearerToken ?? broadcast;
  const rtmpUrl = `rtmp://${host}:${rtmpPort}/live/${rtmpKey}`;

  // WHIP: bearer goes in the Authorization header at runtime, not the URL.
  const whipUrl = `${httpScheme}://${host}:${whipPort}/whip/${broadcast}`;
  const whepUrl = `${httpScheme}://${host}:${whepPort}/whep/${broadcast}`;

  // SRT carries auth in the streamid; document the shape via a token=...
  // placeholder when we have one to substitute.
  const tokenSegment = bearerToken ? `,t=${bearerToken}` : '';
  const srtUrl = `srt://${host}:${srtPort}?streamid=${enc(`m=publish,r=${broadcast}${tokenSegment}`)}`;
  const rtspUrl = `rtsp://${host}:${rtspPort}/${broadcast}`;

  // MoQ subscribers connect via QUIC/WebTransport; the URL shape is what the
  // moq-lite native client + browser WebTransport sample fetch.
  const moqUrl = `https://${host}:${moqPort}/${broadcast}`;
  const hlsUrl = `${httpScheme}://${host}:${hlsPort}/hls/${broadcast}/master.m3u8`;
  const dashUrl = `${httpScheme}://${host}:${dashPort}/dash/${broadcast}/manifest.mpd`;
  // WS fMP4 + admin share the listener; the path is /ws/<broadcast>.
  const wsUrl = `${wssOrWs}//${host}${profilePort(profile)}/ws/${broadcast}`;

  const lvqrPlayer = `<lvqr-player src="${moqUrl}"${bearerToken ? ` token="${bearerToken}"` : ''}></lvqr-player>`;
  const lvqrDvrPlayer = `<lvqr-dvr-player src="${hlsUrl}"${bearerToken ? ` token="${bearerToken}"` : ''}></lvqr-dvr-player>`;

  return {
    publish: { rtmp: rtmpUrl, whip: whipUrl, srt: srtUrl, rtsp: rtspUrl },
    subscribe: { moq: moqUrl, whep: whepUrl, hls: hlsUrl, dash: dashUrl, ws: wsUrl },
    embed: { lvqrPlayer, lvqrDvrPlayer },
  };
}

/**
 * Render the `:<port>` suffix from the admin URL when the port is non-default
 * (so localhost:18090 keeps its port; https://relay.example.com / 443 elides).
 */
function profilePort(profile: Pick<ConnectionProfile, 'baseUrl'>): string {
  try {
    const url = new URL(profile.baseUrl);
    if (!url.port) return '';
    return `:${url.port}`;
  } catch {
    return '';
  }
}
