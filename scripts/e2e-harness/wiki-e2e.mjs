#!/usr/bin/env node
// /web/wiki interactive E2E — real browser drives the full CRUD loop.
//
// Usage:
//   node wiki-e2e.mjs <base-url> <api-key> <output-png>
//
// Scenario (all against the real harness-booted stack — NO mocks):
//   1. Navigate to <base-url>/web/wiki
//   2. Fill the API-key input + click Sign in → auth gate flips to workbench
//   3. Wait for wiki-list to populate the left rail (assert >= 1 page)
//   4. Click the first page → wiki-read populates the center pane
//   5. Click Edit → type a sentinel marker into the textarea → Save
//   6. Verify saved by re-reading the page (Cancel back to read mode)
//   7. Type the sentinel in the search bar → hit Enter → assert >= 1 hit
//   8. Screenshot the authed workbench to <output-png>
//
// Exits 0 on full success, non-zero on any assertion failure (with a
// diagnostic printed to stderr). Screenshot is written regardless so
// harness teardown has something to inspect.

import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import path from "node:path";

const [, , baseUrl, apiKey, outPath] = process.argv;
if (!baseUrl || !apiKey || !outPath) {
  console.error("usage: wiki-e2e.mjs <base-url> <api-key> <output-png>");
  process.exit(2);
}

// Same resolution strategy as screenshot.mjs — prefer gstack's vendored
// playwright-core (zero-setup), fall back to system.
const candidates = [
  process.env.HOME + "/.claude/skills/gstack/node_modules/playwright-core",
  process.env.HOME + "/.claude/skills/gstack/node_modules/playwright",
  "/opt/homebrew/lib/node_modules/playwright-core",
  "/usr/local/lib/node_modules/playwright-core",
];
let playwrightMod = null;
for (const root of candidates) {
  if (!existsSync(root)) continue;
  try {
    const require = createRequire(path.join(root, "package.json"));
    playwrightMod = require(root);
    break;
  } catch {
    /* next */
  }
}
if (!playwrightMod) {
  console.error(
    "[wiki-e2e] playwright-core not found. Install gstack or `npm i -g playwright-core`.",
  );
  process.exit(3);
}

const { chromium } = playwrightMod;

// ---------------------------------------------------------------------------

// Pick a sentinel shape that survives a markdown round-trip. An
// earlier version used `__FOO__` which was fine for the raw <pre>
// view but gets interpreted as GFM bold once ISSUE A.1 wired
// react-markdown — `__WORDS__` renders as `<strong>WORDS</strong>`
// and the surrounding underscores vanish from `textContent`. Plain
// ASCII + hyphens survive both.
const SENTINEL = `WIKI-E2E-SENTINEL-${Date.now()}`;

const browser = await chromium.launch({ headless: true });
let failure = null;
try {
  const ctx = await browser.newContext({
    viewport: { width: 1280, height: 800 },
  });
  const page = await ctx.newPage();

  // Surface browser console errors — a silent JS error would fail the
  // "wait for page list" step with a useless timeout otherwise.
  page.on("console", (msg) => {
    if (msg.type() === "error") {
      console.error(`[wiki-e2e] browser console.error: ${msg.text()}`);
    }
  });

  // Step 1 — navigate.
  await page.goto(`${baseUrl}/web/wiki`, {
    waitUntil: "networkidle",
    timeout: 15000,
  });

  // Step 2 — fill + submit the auth gate.
  await page.waitForSelector('[data-testid="wiki-auth-gate"]', {
    timeout: 5000,
  });
  await page.fill('input[type="password"]', apiKey);
  await page.keyboard.press("Enter");

  // Step 3 — workbench appears + page list populates.
  await page.waitForSelector('[data-testid="wiki-workbench"]', {
    timeout: 10000,
  });
  await page.waitForFunction(
    () => {
      const list = document.querySelector('[data-testid="wiki-page-list"]');
      if (!list) return false;
      const items = list.querySelectorAll(
        '[data-testid^="wiki-page-item-"]',
      );
      return items.length > 0;
    },
    { timeout: 10000 },
  );

  // Step 4 — click first page.
  const firstPage = await page.$(
    '[data-testid="wiki-page-list"] [data-testid^="wiki-page-item-"]',
  );
  if (!firstPage) throw new Error("no pages in list after wiki-list");
  const firstPageName = await firstPage.textContent();
  await firstPage.click();
  await page.waitForSelector('[data-testid="wiki-content-readonly"]', {
    timeout: 5000,
  });

  // Step 4b — ISSUE A.1: verify markdown is rendered (not just <pre>).
  // The README seed starts with `# Gadgetron 위키` (seeds/README.md).
  // A real markdown render puts that inside an <h1> under the
  // read-only container; raw <pre> passthrough would leave the `#`
  // in a text node with no heading tag. Assert at least one heading
  // tag exists so a regression that reverts to <pre> is caught.
  const hasRenderedHeading = await page.evaluate(() => {
    const root = document.querySelector(
      '[data-testid="wiki-content-readonly"]',
    );
    if (!root) return false;
    return !!root.querySelector("h1, h2, h3");
  });
  if (!hasRenderedHeading) {
    throw new Error(
      "markdown did not render a heading tag under wiki-content-readonly — is react-markdown wired?",
    );
  }

  // Step 5 — click Edit, add sentinel, Save.
  await page.click('[data-testid="wiki-edit-button"]');
  await page.waitForSelector('[data-testid="wiki-edit-textarea"]', {
    timeout: 2000,
  });
  // Append sentinel without losing existing content — click into the
  // textarea, move cursor to end, type sentinel marker.
  await page.focus('[data-testid="wiki-edit-textarea"]');
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.keyboard.type(SENTINEL);
  await page.click('[data-testid="wiki-save-button"]');
  // After save, textarea goes away and read-only pre returns.
  await page.waitForSelector('[data-testid="wiki-content-readonly"]', {
    timeout: 10000,
  });
  // Verify the sentinel is actually in the rendered content.
  const roundtrip = await page.textContent(
    '[data-testid="wiki-content-readonly"]',
  );
  if (!roundtrip || !roundtrip.includes(SENTINEL)) {
    throw new Error(
      `sentinel not in post-save read-only view: ${roundtrip?.slice(0, 160)}`,
    );
  }

  // Step 6 — search for the sentinel, expect >= 1 hit.
  await page.fill('[data-testid="wiki-search-input"]', SENTINEL);
  await page.click('[data-testid="wiki-search-button"]');
  await page.waitForFunction(
    () => {
      const hits = document.querySelector('[data-testid="wiki-search-hits"]');
      if (!hits) return false;
      // Look for the hits counter `Search hits (N)` where N >= 1
      const header = hits.querySelector("div:first-child");
      if (!header) return false;
      const match = /\((\d+)\)/.exec(header.textContent || "");
      return match && parseInt(match[1], 10) >= 1;
    },
    { timeout: 10000 },
  );

  // Step 7 — screenshot.
  await page.screenshot({ path: outPath, fullPage: false });
  console.log(
    `[wiki-e2e] OK — first page=${firstPageName?.trim()}, sentinel roundtripped, search found it, screenshot=${outPath}`,
  );
} catch (e) {
  failure = e;
  // Capture a failure screenshot if possible.
  try {
    const pages = browser.contexts().flatMap((c) => c.pages());
    if (pages.length > 0) {
      await pages[0].screenshot({ path: outPath, fullPage: false });
    }
  } catch {
    /* best effort */
  }
} finally {
  await browser.close();
}

if (failure) {
  console.error(`[wiki-e2e] FAIL: ${failure.message}`);
  process.exit(1);
}
