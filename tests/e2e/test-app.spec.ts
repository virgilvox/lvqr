import { expect, test } from "@playwright/test";

/**
 * Smoke tests for the LVQR test-app shell.
 *
 * These assertions are deliberately shallow: they verify the browser
 * can load the static page, the three-tab navigation renders, and the
 * Watch tab exposes its key DOM nodes. They do NOT exercise any
 * streaming code path, because Tier 1 does not yet run a real LVQR
 * binary alongside playwright (see playwright.config.ts for the
 * roadmap to that).
 *
 * This file exists so the 5-artifact test contract's E2E slot is
 * satisfied for the workspace starting in Tier 1, and so the Tier 2
 * upgrade to full-stack browser E2E is a mechanical extension of an
 * already-working harness.
 */

test.describe("test-app shell", () => {
  test("loads the landing page and renders the app shell", async ({ page }) => {
    await page.goto("/index.html");

    await expect(page).toHaveTitle(/LVQR Test/i);
    await expect(page.locator("header h1")).toHaveText(/LVQR Test/i);

    // The three-tab navigation is the app's entry surface. Every tab
    // must render as a clickable link.
    const nav = page.locator("nav a");
    await expect(nav).toHaveCount(3);
    await expect(nav.nth(0)).toHaveText(/Watch/i);
    await expect(nav.nth(1)).toHaveText(/Stream/i);
    await expect(nav.nth(2)).toHaveText(/Admin/i);
  });

  test("Watch tab exposes the video element and broadcast input", async ({ page }) => {
    await page.goto("/index.html");

    // Watch is the default active tab.
    await expect(page.locator("#page-watch")).toBeVisible();

    // The fMP4 MSE target element must exist on page load; the player
    // wires its SourceBuffer into this element once a broadcast is
    // selected.
    const video = page.locator("#watch-video");
    await expect(video).toHaveCount(1);
    await expect(video).toHaveAttribute("autoplay", "");

    // Broadcast input defaults to live/webcam; this is what the Tier 2
    // full-stack E2E will parameterize per test.
    await expect(page.locator("#watch-broadcast")).toHaveValue(/live\/webcam/);
  });

  test("Stream tab is reachable and renders its form", async ({ page }) => {
    await page.goto("/index.html");

    // Click the Stream nav link and wait for the page to become active.
    // The app uses onclick handlers rather than real routing, so we
    // click + assert visibility rather than waitForURL.
    await page.locator("nav a", { hasText: /Stream/i }).click();
    await expect(page.locator("#page-stream")).toBeVisible();

    // Key form inputs must be present so the Tier 2 E2E can drive them.
    await expect(page.locator("#stream-server")).toBeVisible();
    await expect(page.locator("#stream-key")).toBeVisible();
  });
});
