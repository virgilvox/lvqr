import { describe, expect, it } from 'vitest';
import {
  DEFAULT_PROTOCOL_PORTS,
  broadcastUrls,
  profileHost,
  profileScheme,
  wsScheme,
} from '../../src/api/protocolUrls';
import type { ConnectionProfile } from '../../src/stores/connection';

function profile(over: Partial<ConnectionProfile> = {}): ConnectionProfile {
  return {
    id: 'cp-stub',
    label: 'stub',
    baseUrl: 'http://relay.example.com:18090',
    ...over,
  };
}

describe('profileHost / profileScheme / wsScheme', () => {
  it('extracts the host and scheme', () => {
    const p = profile();
    expect(profileHost(p)).toBe('relay.example.com');
    expect(profileScheme(p)).toBe('http:');
    expect(wsScheme(p)).toBe('ws:');
  });

  it('flips the ws scheme when admin is https', () => {
    const p = profile({ baseUrl: 'https://relay.example.com' });
    expect(wsScheme(p)).toBe('wss:');
  });

  it('falls back to localhost on a malformed baseUrl', () => {
    const p = profile({ baseUrl: 'not-a-url' });
    expect(profileHost(p)).toBe('localhost');
  });
});

describe('broadcastUrls', () => {
  it('uses default ports when none are overridden', () => {
    const urls = broadcastUrls(profile(), 'live/demo');
    expect(urls.publish.rtmp).toBe(`rtmp://relay.example.com:${DEFAULT_PROTOCOL_PORTS.rtmp}/live/live/demo`);
    expect(urls.publish.whip).toBe(`http://relay.example.com:${DEFAULT_PROTOCOL_PORTS.whip}/whip/live/demo`);
    expect(urls.subscribe.hls).toBe(`http://relay.example.com:${DEFAULT_PROTOCOL_PORTS.hls}/hls/live/demo/master.m3u8`);
    expect(urls.subscribe.dash).toBe(`http://relay.example.com:${DEFAULT_PROTOCOL_PORTS.dash}/dash/live/demo/manifest.mpd`);
  });

  it('honors per-protocol port overrides on the profile', () => {
    const urls = broadcastUrls(
      profile({ rtmpPort: 11935, whipPort: 18443, hlsPort: 18888 }),
      'live/demo',
    );
    expect(urls.publish.rtmp).toContain(':11935');
    expect(urls.publish.whip).toContain(':18443');
    expect(urls.subscribe.hls).toContain(':18888');
  });

  it('substitutes a bearer token into the RTMP key segment when provided', () => {
    const urls = broadcastUrls(profile(), 'live/demo', 'lvqr_sk_secret');
    expect(urls.publish.rtmp).toBe('rtmp://relay.example.com:1935/live/lvqr_sk_secret');
  });

  it('renders the SRT streamid with the token + broadcast', () => {
    const urls = broadcastUrls(profile(), 'live/demo', 'tok123');
    expect(urls.publish.srt).toContain('streamid=');
    expect(decodeURIComponent(urls.publish.srt)).toContain('m=publish,r=live/demo,t=tok123');
  });

  it('renders the embed snippets with the token in the attribute', () => {
    const urls = broadcastUrls(profile(), 'live/demo', 'tok-x');
    expect(urls.embed.lvqrPlayer).toContain('token="tok-x"');
    expect(urls.embed.lvqrDvrPlayer).toContain('token="tok-x"');
    expect(urls.embed.lvqrPlayer).toContain('<lvqr-player ');
    expect(urls.embed.lvqrDvrPlayer).toContain('<lvqr-dvr-player ');
  });

  it('uses https for subscribe URLs when admin URL is https', () => {
    const urls = broadcastUrls(profile({ baseUrl: 'https://relay.example.com' }), 'live/demo');
    expect(urls.subscribe.hls.startsWith('https://')).toBe(true);
    expect(urls.subscribe.dash.startsWith('https://')).toBe(true);
  });

  it('omits the port from the WS subscribe URL when admin is on a default port', () => {
    const urls = broadcastUrls(profile({ baseUrl: 'http://relay.example.com' }), 'live/demo');
    expect(urls.subscribe.ws).toBe('ws://relay.example.com/ws/live/demo');
  });
});
