#!/usr/bin/env node
// Headless screenshot helper for the E2E harness.
//
// Usage: node screenshot.mjs <url> <output-png>
//
// Resolves `playwright-core` via the local `node_modules` first (gstack
// already installs it); falls back to the system-wide install (pipx /
// Homebrew `playwright` CLI). Non-zero exit on any failure.

import { fileURLToPath } from 'node:url';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

const [, , targetUrl, outPath] = process.argv;
if (!targetUrl || !outPath) {
  console.error('usage: screenshot.mjs <url> <output-png>');
  process.exit(2);
}

// Resolution candidates — check gstack's vendored copy first, then the
// system one. Gstack bundles playwright-core with Chromium binaries
// already downloaded, so it's the zero-setup path on most dev machines.
const candidates = [
  process.env.HOME + '/.claude/skills/gstack/node_modules/playwright-core',
  process.env.HOME + '/.claude/skills/gstack/node_modules/playwright',
  // Homebrew-installed playwright (npm -g)
  '/opt/homebrew/lib/node_modules/playwright-core',
  '/usr/local/lib/node_modules/playwright-core',
];

let playwrightMod = null;
for (const root of candidates) {
  if (!existsSync(root)) continue;
  try {
    const require = createRequire(path.join(root, 'package.json'));
    playwrightMod = require(root);
    break;
  } catch (_) {
    /* try next */
  }
}

if (!playwrightMod) {
  console.error(
    '[screenshot] playwright-core not found. Install gstack or `npm i -g playwright-core`.'
  );
  process.exit(3);
}

const { chromium } = playwrightMod;

const browser = await chromium.launch({ headless: true });
try {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 } });
  const page = await ctx.newPage();
  await page.goto(targetUrl, { waitUntil: 'networkidle', timeout: 15000 });
  // Give the React workbench shell a beat to render beyond "Checking..."
  await page.waitForTimeout(750);
  await page.screenshot({ path: outPath, fullPage: false });
  console.log(`[screenshot] wrote ${outPath}`);
} finally {
  await browser.close();
}
