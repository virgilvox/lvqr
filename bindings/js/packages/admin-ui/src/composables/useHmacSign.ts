/**
 * HMAC-SHA256 signing in the browser via the Web Crypto API. Mirrors the
 * pure helpers `lvqr_cli::sign_playback_url` + `lvqr_cli::sign_live_url` in
 * the LVQR Rust crate so an operator can mint a signed URL from this UI
 * without going back to the CLI.
 *
 * The signed-URL contract LVQR enforces:
 *
 *   * Playback (`/playback/*`): signature input is `<path>?exp=<ts>` and the
 *     signature URL becomes `<path>?exp=<ts>&sig=<base64url>`.
 *   * Live HLS / DASH (`/hls/<broadcast>/master.m3u8`,
 *     `/dash/<broadcast>/manifest.mpd`): signature input is
 *     `<scheme>:<broadcast>?exp=<ts>` (scheme = "hls" or "dash"), and the
 *     same `?exp=&sig=` query suffix appends to the master URL. A single
 *     signature grants access to every numbered / partial segment under
 *     `/hls/<broadcast>/*` (or `/dash/<broadcast>/*`) so LL-HLS partials
 *     that roll over every 200 ms still resolve.
 *
 * Both schemes use the same shared `--hmac-playback-secret` on the relay.
 */

const enc = new TextEncoder();

/** Base64URL (no `=` padding) -- matches Rust's base64 URL_SAFE_NO_PAD. */
export function bytesToBase64Url(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  const b64 = typeof btoa === 'function' ? btoa(s) : Buffer.from(s, 'binary').toString('base64');
  return b64.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

async function importHmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    'raw',
    enc.encode(secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
}

/**
 * Sign an arbitrary input string under a shared secret. Useful when the
 * caller wants to drive the URL composition themselves; both
 * `signPlaybackUrl` + `signLiveUrl` route through this primitive.
 */
export async function signHmacSha256(secret: string, input: string): Promise<string> {
  const key = await importHmacKey(secret);
  const sigBytes = await crypto.subtle.sign('HMAC', key, enc.encode(input));
  return bytesToBase64Url(new Uint8Array(sigBytes));
}

/** Append the signed `?exp=&sig=` suffix to a playback path. */
export async function signPlaybackUrl(
  baseUrl: string,
  path: string,
  expUnixSecs: number,
  secret: string,
): Promise<string> {
  const normalisedPath = path.startsWith('/') ? path : `/${path}`;
  const input = `${normalisedPath}?exp=${expUnixSecs}`;
  const sig = await signHmacSha256(secret, input);
  const baseTrim = baseUrl.replace(/\/+$/, '');
  return `${baseTrim}${normalisedPath}?exp=${expUnixSecs}&sig=${sig}`;
}

/**
 * Live-HLS / DASH variant. Signs `<scheme>:<broadcast>?exp=<ts>` and
 * appends `?exp=&sig=` to the master playlist URL. The broadcast-scoped
 * signature stays valid for every numbered / partial segment under the
 * broadcast prefix; cross-scheme replay is rejected by the relay because
 * the scheme tag is part of the signed input.
 */
export async function signLiveUrl(
  baseUrl: string,
  scheme: 'hls' | 'dash',
  broadcast: string,
  expUnixSecs: number,
  secret: string,
): Promise<string> {
  const input = `${scheme}:${broadcast}?exp=${expUnixSecs}`;
  const sig = await signHmacSha256(secret, input);
  const path = scheme === 'hls'
    ? `/hls/${encodeURIComponent(broadcast)}/master.m3u8`
    : `/dash/${encodeURIComponent(broadcast)}/manifest.mpd`;
  const baseTrim = baseUrl.replace(/\/+$/, '');
  return `${baseTrim}${path}?exp=${expUnixSecs}&sig=${sig}`;
}
