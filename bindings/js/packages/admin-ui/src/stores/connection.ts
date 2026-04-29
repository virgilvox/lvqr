import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import { LvqrAdminClient } from '@lvqr/core';
import { normalizeRelayUrl } from '@/api/url';

const STORAGE_KEY = 'lvqr.admin.connection.v1';

export interface ConnectionProfile {
  id: string;
  label: string;
  baseUrl: string;
  bearerToken?: string;
}

interface PersistedState {
  profiles: ConnectionProfile[];
  activeId: string | null;
}

function loadPersisted(): PersistedState {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { profiles: [], activeId: null };
    const parsed = JSON.parse(raw) as PersistedState;
    return {
      profiles: Array.isArray(parsed.profiles) ? parsed.profiles : [],
      activeId: parsed.activeId ?? null,
    };
  } catch {
    return { profiles: [], activeId: null };
  }
}

function savePersisted(state: PersistedState) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // ignore quota / private-mode failures; the UI still works for the session.
  }
}

function newProfileId(): string {
  return `cp-${Math.random().toString(36).slice(2, 10)}`;
}

/**
 * Active connection profile state. Persists to localStorage so operators do
 * not have to re-enter their relay URL + token every page load. Shape is
 * intentionally minimal so future fields (mTLS cert path, SSO redirect URL,
 * etc.) can land additively without a schema migration.
 */
export const useConnectionStore = defineStore('connection', () => {
  const persisted = loadPersisted();
  const profiles = ref<ConnectionProfile[]>(persisted.profiles);
  const activeId = ref<string | null>(persisted.activeId);

  const activeProfile = computed<ConnectionProfile | null>(() => {
    if (!activeId.value) return null;
    return profiles.value.find((p) => p.id === activeId.value) ?? null;
  });

  const client = computed<LvqrAdminClient | null>(() => {
    if (!activeProfile.value) return null;
    return new LvqrAdminClient(activeProfile.value.baseUrl, {
      bearerToken: activeProfile.value.bearerToken,
      fetchTimeoutMs: 10_000,
    });
  });

  function addProfile(input: Omit<ConnectionProfile, 'id'>): ConnectionProfile {
    const profile: ConnectionProfile = {
      id: newProfileId(),
      label: input.label.trim() || 'untitled',
      baseUrl: normalizeRelayUrl(input.baseUrl),
      bearerToken: input.bearerToken?.trim() || undefined,
    };
    profiles.value.push(profile);
    if (!activeId.value) activeId.value = profile.id;
    return profile;
  }

  function updateProfile(id: string, patch: Partial<Omit<ConnectionProfile, 'id'>>): void {
    const idx = profiles.value.findIndex((p) => p.id === id);
    if (idx < 0) return;
    const existing = profiles.value[idx];
    profiles.value[idx] = {
      ...existing,
      ...patch,
      baseUrl: patch.baseUrl !== undefined ? normalizeRelayUrl(patch.baseUrl) : existing.baseUrl,
      bearerToken: patch.bearerToken !== undefined ? patch.bearerToken?.trim() || undefined : existing.bearerToken,
    };
  }

  function removeProfile(id: string): void {
    profiles.value = profiles.value.filter((p) => p.id !== id);
    if (activeId.value === id) {
      activeId.value = profiles.value[0]?.id ?? null;
    }
  }

  function setActive(id: string): void {
    if (profiles.value.some((p) => p.id === id)) {
      activeId.value = id;
    }
  }

  watch(
    [profiles, activeId],
    ([p, a]) => savePersisted({ profiles: p, activeId: a }),
    { deep: true, flush: 'sync' },
  );

  return {
    profiles,
    activeId,
    activeProfile,
    client,
    addProfile,
    updateProfile,
    removeProfile,
    setActive,
  };
});
