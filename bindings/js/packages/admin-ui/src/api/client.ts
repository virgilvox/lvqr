import { LvqrAdminClient } from '@lvqr/core';
import type { LvqrAdminClientOptions } from '@lvqr/core';

/**
 * Thin factory around `@lvqr/core`'s `LvqrAdminClient` so every store goes
 * through one place. The active connection profile drives the construction;
 * tests can override by passing options directly.
 */
export function buildClient(baseUrl: string, options: LvqrAdminClientOptions = {}): LvqrAdminClient {
  return new LvqrAdminClient(baseUrl, {
    fetchTimeoutMs: options.fetchTimeoutMs ?? 10_000,
    bearerToken: options.bearerToken,
  });
}
