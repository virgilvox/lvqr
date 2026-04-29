import type { Component } from 'vue';
import type { Router } from 'vue-router';

export interface AdminPlugin {
  /** Stable plugin id; used as a route name and dedup key. */
  id: string;
  /** Display label in the rail. */
  label: string;
  /** Path the plugin mounts under (e.g. `/plugins/foo`). */
  path: string;
  /** Vue component the path renders. */
  component: Component;
  /** Rail section id (one of `operations | pipeline | infrastructure | identity | system`). Defaults to `system`. */
  rail?: string;
  /** Optional icon identifier (consumer uses it to pick a glyph; UI tolerates unknown values). */
  icon?: string;
}

declare global {
  interface Window {
    __LVQR_ADMIN_PLUGINS__?: AdminPlugin[];
  }
}

/**
 * Register every plugin advertised on `window.__LVQR_ADMIN_PLUGINS__`. The
 * registration appends a route per plugin so the rail can render entries
 * driven by the same metadata.
 *
 * Intentional contract:
 *
 * - Plugins register before `app.mount`. The host page sets the global
 *   before loading the admin UI bundle.
 * - One plugin per `id`. Duplicate ids are skipped with a console warning.
 * - The `path` must be unique. Collision with built-in routes is rejected.
 *
 * v1.0 ships only the plumbing; example plugins ship in a future minor.
 */
export function registerPlugins(router: Router): void {
  const plugins = (typeof window !== 'undefined' && window.__LVQR_ADMIN_PLUGINS__) || [];
  const seen = new Set<string>();
  for (const plugin of plugins) {
    if (seen.has(plugin.id)) {
      // eslint-disable-next-line no-console
      console.warn(`[lvqr-admin-ui] duplicate plugin id "${plugin.id}" -- skipping`);
      continue;
    }
    if (router.hasRoute(plugin.id)) {
      // eslint-disable-next-line no-console
      console.warn(`[lvqr-admin-ui] plugin id "${plugin.id}" collides with a built-in route -- skipping`);
      continue;
    }
    seen.add(plugin.id);
    router.addRoute({
      path: plugin.path,
      name: plugin.id,
      component: plugin.component,
      meta: {
        rail: plugin.rail ?? 'system',
        label: plugin.label,
        icon: plugin.icon ?? 'plugin',
        plugin: true,
      },
    });
  }
}

/** Return registered plugins (read-only) for the rail to render entries. */
export function listPlugins(): AdminPlugin[] {
  return (typeof window !== 'undefined' && window.__LVQR_ADMIN_PLUGINS__) || [];
}
