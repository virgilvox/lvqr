import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useConnectionStore } from '../../src/stores/connection';

describe('connection store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    localStorage.clear();
  });

  it('starts empty', () => {
    const conn = useConnectionStore();
    expect(conn.profiles).toEqual([]);
    expect(conn.activeId).toBeNull();
    expect(conn.activeProfile).toBeNull();
    expect(conn.client).toBeNull();
  });

  it('addProfile assigns an id, normalizes url, becomes active by default', () => {
    const conn = useConnectionStore();
    const p = conn.addProfile({ label: 'staging', baseUrl: 'http://x:8080/' });
    expect(p.id).toMatch(/^cp-/);
    expect(p.baseUrl).toBe('http://x:8080');
    expect(conn.activeId).toBe(p.id);
    expect(conn.activeProfile?.id).toBe(p.id);
    expect(conn.client).not.toBeNull();
  });

  it('rejects bad URLs', () => {
    const conn = useConnectionStore();
    expect(() => conn.addProfile({ label: 'bad', baseUrl: 'no-scheme' })).toThrow();
  });

  it('removeProfile re-points active to the next profile', () => {
    const conn = useConnectionStore();
    const a = conn.addProfile({ label: 'a', baseUrl: 'http://a' });
    const b = conn.addProfile({ label: 'b', baseUrl: 'http://b' });
    expect(conn.activeId).toBe(a.id);
    conn.removeProfile(a.id);
    expect(conn.activeId).toBe(b.id);
    conn.removeProfile(b.id);
    expect(conn.activeId).toBeNull();
  });

  it('setActive only accepts known ids', () => {
    const conn = useConnectionStore();
    conn.addProfile({ label: 'a', baseUrl: 'http://a' });
    conn.setActive('not-a-real-id');
    expect(conn.activeProfile?.label).toBe('a');
  });

  it('updateProfile patches and re-normalizes', () => {
    const conn = useConnectionStore();
    const p = conn.addProfile({ label: 'orig', baseUrl: 'http://x' });
    conn.updateProfile(p.id, { label: 'new', baseUrl: 'https://y/' });
    const updated = conn.profiles.find((x) => x.id === p.id);
    expect(updated?.label).toBe('new');
    expect(updated?.baseUrl).toBe('https://y');
  });

  it('persists profiles across hydrations via localStorage', () => {
    const first = useConnectionStore();
    first.addProfile({ label: 'persist', baseUrl: 'http://x', bearerToken: 't' });

    setActivePinia(createPinia());
    const second = useConnectionStore();
    expect(second.profiles.length).toBe(1);
    expect(second.profiles[0].bearerToken).toBe('t');
    expect(second.activeId).not.toBeNull();
  });
});
