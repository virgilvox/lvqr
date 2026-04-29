/**
 * Build-time + runtime defaults for the admin UI.
 *
 * Build-time values come from Vite env (`VITE_*`); runtime values come from
 * an `app-config.json` fetched at bootstrap from the served root, when
 * present. Runtime config is the right knob for static-hosted deployments
 * that want to point a single dist/ at different LVQR backends without
 * rebuilding.
 *
 * Both layers are optional. With neither, the UI defaults to the connection
 * profile UI showing an empty list and prompting the operator to add their
 * first relay.
 */

export interface AppConfig {
  /** Default LVQR base URL operators see when no profile is registered yet. */
  defaultRelayUrl?: string;
  /** Optional Grafana base URL the Observability view links into. */
  grafanaUrl?: string;
  /** Optional bearer token to pre-fill on the first profile. Avoid in production. */
  defaultBearerToken?: string;
}

const DEFAULTS: AppConfig = {
  defaultRelayUrl: 'http://localhost:8080',
  grafanaUrl: undefined,
  defaultBearerToken: undefined,
};

/**
 * Resolve runtime config. Prefers `/app-config.json` when present, falls back
 * to Vite env, then `DEFAULTS`. Failures are silent so a missing config file
 * does not block bootstrap.
 */
export async function loadAppConfig(): Promise<AppConfig> {
  let runtime: AppConfig = {};
  try {
    const resp = await fetch('/app-config.json', { cache: 'no-store' });
    if (resp.ok) {
      runtime = (await resp.json()) as AppConfig;
    }
  } catch {
    // ignore
  }
  return {
    defaultRelayUrl:
      runtime.defaultRelayUrl ?? import.meta.env.VITE_LVQR_RELAY_URL ?? DEFAULTS.defaultRelayUrl,
    grafanaUrl: runtime.grafanaUrl ?? import.meta.env.VITE_LVQR_GRAFANA_URL ?? DEFAULTS.grafanaUrl,
    defaultBearerToken: runtime.defaultBearerToken ?? DEFAULTS.defaultBearerToken,
  };
}
