import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright config for the LVQR browser E2E suite.
 *
 * Tier 1 scope: the only target today is the static test-app shell, so
 * the `webServer` block spins up a Python http.server on port 9000
 * pointing at `../../test-app`. That gives every spec a stable origin
 * to load against with zero Rust build dependency.
 *
 * Tier 2 scope (later): a second `webServer` entry will run
 * `cargo run -p lvqr-cli -- serve` against an ephemeral admin port, the
 * config will propagate that port to specs via `process.env`, and specs
 * will connect real WebSockets into it. That requires a warmed cargo
 * build cache in CI which we are not paying for during Tier 1.
 */
export default defineConfig({
  testDir: ".",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 2 : undefined,
  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "playwright-report" }],
  ],
  timeout: 30_000,
  expect: { timeout: 5_000 },

  use: {
    baseURL: "http://127.0.0.1:9000",
    headless: true,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

  // The test-app is a plain static page. python3 is already installed
  // on the ubuntu-latest runner and contributor laptops, so this needs
  // no extra tool install. 127.0.0.1 (not 0.0.0.0) keeps the server
  // off the external network on dev machines.
  webServer: {
    command: "python3 -m http.server 9000 --bind 127.0.0.1 --directory ../../test-app",
    url: "http://127.0.0.1:9000/index.html",
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
