import { afterEach, describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import Button from '../../src/components/ui/Button.vue';
import Tally from '../../src/components/ui/Tally.vue';
import Badge from '../../src/components/ui/Badge.vue';
import KpiTile from '../../src/components/ui/KpiTile.vue';
import EmptyState from '../../src/components/ui/EmptyState.vue';

const wrappers: ReturnType<typeof mount>[] = [];

afterEach(() => {
  while (wrappers.length) wrappers.pop()?.unmount();
});

function track<W extends ReturnType<typeof mount>>(w: W): W {
  wrappers.push(w);
  return w;
}

describe('Button', () => {
  it('renders the slotted label', () => {
    const w = track(mount(Button, { slots: { default: 'mint' } }));
    expect(w.text()).toBe('mint');
    expect(w.element.tagName).toBe('BUTTON');
  });

  it('applies the variant class', () => {
    for (const variant of ['primary', 'wire', 'ghost', 'danger'] as const) {
      const w = track(mount(Button, { props: { variant } }));
      expect(w.classes()).toContain(`btn-${variant}`);
    }
  });

  it('disables itself when loading', () => {
    const w = track(mount(Button, { props: { loading: true } }));
    expect((w.element as HTMLButtonElement).disabled).toBe(true);
    expect(w.classes()).toContain('is-loading');
  });

  it('respects the type prop', () => {
    const w = track(mount(Button, { props: { type: 'submit' } }));
    expect((w.element as HTMLButtonElement).type).toBe('submit');
  });
});

describe('Tally', () => {
  it('renders a status-class wrapper for each status', () => {
    for (const status of ['on-air', 'ready', 'warn', 'idle'] as const) {
      const w = track(mount(Tally, { props: { status } }));
      expect(w.classes()).toContain(`tally-${status}`);
    }
  });

  it('renders the optional label', () => {
    const w = track(mount(Tally, { props: { status: 'ready', label: 'live' } }));
    expect(w.text()).toContain('live');
  });
});

describe('Badge', () => {
  it('falls back to the neutral variant', () => {
    const w = track(mount(Badge, { slots: { default: 'x' } }));
    expect(w.classes()).toContain('badge-neutral');
  });

  it('renders the requested variant', () => {
    const w = track(mount(Badge, { props: { variant: 'on-air' }, slots: { default: 'live' } }));
    expect(w.classes()).toContain('badge-on-air');
    expect(w.text()).toBe('live');
  });
});

describe('KpiTile', () => {
  it('renders label, value, unit, and hint', () => {
    const w = track(
      mount(KpiTile, {
        props: { label: 'Subscribers', value: '42', unit: 'live', hint: 'p99 200ms' },
      }),
    );
    expect(w.text()).toContain('Subscribers');
    expect(w.text()).toContain('42');
    expect(w.text()).toContain('live');
    expect(w.text()).toContain('p99 200ms');
  });

  it('defaults to the tally accent', () => {
    const w = track(mount(KpiTile, { props: { label: 'x', value: 1 } }));
    expect(w.classes()).toContain('kpi-accent-tally');
  });
});

describe('EmptyState', () => {
  it('renders title + body + action slot', () => {
    const w = track(
      mount(EmptyState, {
        props: { title: 'Connect', kicker: 'WELCOME' },
        slots: { default: 'add a relay', actions: '<button>Go</button>' },
      }),
    );
    expect(w.text()).toContain('Connect');
    expect(w.text()).toContain('add a relay');
    expect(w.html()).toContain('<button>Go</button>');
    expect(w.text()).toContain('WELCOME');
  });
});
