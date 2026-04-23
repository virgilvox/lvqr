// Vitest config for @lvqr/core SDK tests.
//
// Expects a locally-running `lvqr serve` on `LVQR_TEST_ADMIN_URL`
// (default `http://127.0.0.1:18090`). The CI workflow boots the
// binary as a child process; locally operators can either boot
// `lvqr serve --admin-port 18090` themselves or set the env var
// to point at an already-running instance.
//
// Tests live under `tests/sdk/` alongside the existing Playwright
// `tests/e2e/` tree. Separating the directories keeps Vitest's
// scanner off the Playwright specs (which require `@playwright/test`
// not Vitest).

import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['tests/sdk/**/*.spec.ts'],
    testTimeout: 15_000,
    hookTimeout: 30_000,
    reporters: ['default'],
    pool: 'forks',
  },
});
