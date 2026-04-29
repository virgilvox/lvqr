import { afterEach, describe, expect, it } from 'vitest';
import { defineComponent, h } from 'vue';
import { createMemoryHistory, createRouter } from 'vue-router';
import { listPlugins, registerPlugins } from '../../src/plugins';

const Stub = defineComponent({ render: () => h('div', 'stub') });

afterEach(() => {
  delete (window as Window & { __LVQR_ADMIN_PLUGINS__?: unknown }).__LVQR_ADMIN_PLUGINS__;
});

describe('plugins', () => {
  it('listPlugins returns [] when nothing is registered', () => {
    expect(listPlugins()).toEqual([]);
  });

  it('registers each plugin as a route', () => {
    window.__LVQR_ADMIN_PLUGINS__ = [
      { id: 'foo', label: 'Foo', path: '/plugins/foo', component: Stub, icon: 'plugin' },
      { id: 'bar', label: 'Bar', path: '/plugins/bar', component: Stub, rail: 'pipeline' },
    ];
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: Stub }] });
    registerPlugins(router);
    expect(router.hasRoute('foo')).toBe(true);
    expect(router.hasRoute('bar')).toBe(true);
    const foo = router.getRoutes().find((r) => r.name === 'foo');
    expect(foo?.path).toBe('/plugins/foo');
    expect((foo?.meta as { rail?: string }).rail).toBe('system');
    const bar = router.getRoutes().find((r) => r.name === 'bar');
    expect((bar?.meta as { rail?: string }).rail).toBe('pipeline');
  });

  it('skips duplicate ids', () => {
    window.__LVQR_ADMIN_PLUGINS__ = [
      { id: 'dupe', label: 'A', path: '/a', component: Stub },
      { id: 'dupe', label: 'B', path: '/b', component: Stub },
    ];
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', component: Stub }] });
    registerPlugins(router);
    const matches = router.getRoutes().filter((r) => r.name === 'dupe');
    expect(matches.length).toBe(1);
    expect(matches[0].path).toBe('/a');
  });

  it('skips collisions with built-in routes', () => {
    window.__LVQR_ADMIN_PLUGINS__ = [
      { id: 'home', label: 'Plugin Home', path: '/from-plugin', component: Stub },
    ];
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', name: 'home', component: Stub }],
    });
    registerPlugins(router);
    const home = router.getRoutes().find((r) => r.name === 'home');
    expect(home?.path).toBe('/');
  });
});
