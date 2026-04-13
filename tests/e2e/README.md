# LVQR Playwright E2E

Browser end-to-end tests for the LVQR `test-app`. This directory is the
E2E slot for the 5-artifact test contract described at
`tests/CONTRACT.md`.

## Running locally

```bash
cd tests/e2e
npm install
npm run install-browsers    # one-time: pull headless Chromium
npm test
```

Playwright spins up a Python `http.server` on `127.0.0.1:9000` serving
the `test-app/` directory. See `playwright.config.ts` for the config.

To watch the tests run in a headed browser:

```bash
npm run test:headed
```

The HTML reporter drops a report under `tests/e2e/playwright-report/`;
open `index.html` from there after a run.

## Scope

Tier 1 (now): the specs under this directory are shallow. They load
the test-app shell, verify the three-tab navigation, and check that
the Watch tab's video element is present. They do NOT drive real
media, because Tier 1 does not yet run a live LVQR binary alongside
playwright.

Tier 2 (soon): the roadmap adds a second `webServer` entry to
`playwright.config.ts` that runs `cargo run -p lvqr-cli -- serve`
against an ephemeral admin port, and the specs start real streams via
`TestServer`-style fixtures and assert `videoElement.buffered.length
> 0` within five seconds of connect. That requires a warmed cargo
build in CI, which Tier 1 does not pay for.

## CI

`.github/workflows/e2e.yml` runs this suite on every PR as a
`continue-on-error` job. Soft-fail during Tier 1, gating from Tier 2
once the full-stack specs land.
