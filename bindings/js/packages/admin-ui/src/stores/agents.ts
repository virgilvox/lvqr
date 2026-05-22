import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { AddAgentRequest, AgentState } from '@lvqr/core';
import { useConnectionStore } from './connection';

/**
 * In-process agent introspection backing the Agents view. `state` holds the
 * configured agent set + live counters; `error` captures transport failures
 * so the poll loop never crashes the view.
 */
export const useAgentsStore = defineStore('agents', () => {
  const state = ref<AgentState | null>(null);
  const error = ref<string | null>(null);
  const lastFetchedAt = ref<number | null>(null);

  async function fetch(): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) return;
    try {
      state.value = await conn.client.agents();
      error.value = null;
      lastFetchedAt.value = Date.now();
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e);
    }
  }

  /** Start an agent at runtime, then refresh. Throws on failure (e.g. 409). */
  async function addAgent(req: AddAgentRequest): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.addAgent(req);
    await fetch();
  }

  /** Stop an agent at runtime, then refresh. Throws on failure (e.g. 404). */
  async function removeAgent(name: string): Promise<void> {
    const conn = useConnectionStore();
    if (!conn.client) throw new Error('no active connection');
    await conn.client.removeAgent(name);
    await fetch();
  }

  return { state, error, lastFetchedAt, fetch, addAgent, removeAgent };
});
