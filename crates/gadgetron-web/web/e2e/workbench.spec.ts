import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Workbench e2e: mocked /health=200 + mocked /v1/chat/completions streaming
// Verifies 3-panel shell is present and panels don't collapse unexpectedly.
// ---------------------------------------------------------------------------

test.beforeEach(async ({ page }) => {
  // Mock /health to return 200 healthy
  await page.route("**/health", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ status: "ok", degraded_reasons: [] }),
    });
  });

  // Mock /v1/chat/completions with a minimal SSE stream
  await page.route("**/chat/completions", async (route) => {
    const body = [
      `data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"role":"assistant","content":"Hello"},"index":0}]}`,
      `data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{"content":" world"},"index":0}]}`,
      `data: {"id":"chatcmpl-1","object":"chat.completion.chunk","choices":[{"delta":{},"finish_reason":"stop","index":0}]}`,
      `data: [DONE]`,
    ]
      .map((l) => l + "\n\n")
      .join("");

    await route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body,
    });
  });

  // Mock /v1/models
  await page.route("**/models", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ object: "list", data: [{ id: "penny", object: "model" }] }),
    });
  });

  // Navigate and set up API key in localStorage before page runs
  await page.goto("/web");
  await page.evaluate(() => {
    localStorage.setItem("gadgetron_api_key", "gad_live_test_key");
  });
  await page.reload();
});

test("3-panel shell renders: left rail, chat column, evidence pane", async ({
  page,
}) => {
  await expect(page.getByTestId("workbench-shell")).toBeVisible();
  await expect(page.getByTestId("left-rail")).toBeVisible();
  await expect(page.getByTestId("chat-column")).toBeVisible();
  await expect(page.getByTestId("evidence-pane")).toBeVisible();
});

test("left rail nav tabs present (Chat functional, others P2B stub)", async ({
  page,
}) => {
  await expect(page.getByTestId("nav-tab-chat")).toBeVisible();
  await expect(page.getByTestId("nav-tab-knowledge")).toBeVisible();
  await expect(page.getByTestId("nav-tab-bundles")).toBeVisible();
});

test("status strip shows healthy state", async ({ page }) => {
  const strip = page.getByRole("status", { name: "Workbench status" });
  await expect(strip).toBeVisible();
  await expect(strip).toContainText("Gateway healthy");
});

test("evidence pane collapses and expands without losing chat content", async ({
  page,
}) => {
  // Start: evidence pane open
  await expect(page.getByTestId("evidence-pane")).toBeVisible();

  // Collapse
  await page.getByTestId("evidence-pane-collapse-btn").click();
  await expect(page.getByTestId("evidence-pane")).not.toBeVisible();
  await expect(page.getByTestId("evidence-pane-collapsed")).toBeVisible();

  // Chat column still visible
  await expect(page.getByTestId("chat-column")).toBeVisible();

  // Re-expand
  await page.getByTestId("evidence-pane-expand-btn").click();
  await expect(page.getByTestId("evidence-pane")).toBeVisible();
});

test("empty state text does not contain banned AI-template phrases", async ({
  page,
}) => {
  // Wait for chat empty state to render
  await expect(page.getByTestId("chat-empty-state")).toBeVisible();

  const pageText = await page.getByTestId("chat-empty-state").innerText();

  // Banned phrases per §1.4 principle 6 and spec snapshot requirements
  expect(pageText).not.toContain("무엇이든 물어보세요");
  expect(pageText).not.toContain("무엇을 도와드릴까요");
  expect(pageText).not.toContain("How can I help");

  // Expected workbench copy
  expect(pageText).toContain("Ready");
});

test("panels don't collapse unexpectedly during chat message flow", async ({
  page,
}) => {
  // Type and send a message
  const composer = page.locator("textarea, [role='textbox']").first();
  await composer.fill("test message");
  await composer.press("Enter");

  // Wait briefly for streaming to start/finish
  await page.waitForTimeout(500);

  // Panels should still be present
  await expect(page.getByTestId("left-rail")).toBeVisible();
  await expect(page.getByTestId("chat-column")).toBeVisible();
  await expect(page.getByTestId("evidence-pane")).toBeVisible();
});

// ROADMAP ISSUE 29 TASK 29.6 — clicking between Chat / Wiki / Dashboard
// through the LeftRail must keep the shell chrome (StatusStrip + LeftRail)
// mounted. Before the route-group refactor the shell unmounted on every
// navigation, so the chat thread state was lost when the operator clicked
// Dashboard mid-generation.
test("shell chrome persists across Chat → Wiki → Dashboard navigation", async ({
  page,
}) => {
  // Mock the workbench endpoints hit by wiki + dashboard so navigation
  // doesn't stall on 401s. These responses don't need to be realistic —
  // we're asserting DOM persistence, not page content.
  await page.route("**/workbench/actions/wiki-list", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ result: { payload: { pages: [] } } }),
    });
  });
  await page.route("**/workbench/usage/summary", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        window_hours: 24,
        chat: {
          requests: 0,
          errors: 0,
          total_input_tokens: 0,
          total_output_tokens: 0,
          total_cost_cents: 0,
          avg_latency_ms: 0,
        },
        actions: {
          total: 0,
          success: 0,
          error: 0,
          pending_approval: 0,
          avg_elapsed_ms: 0,
        },
        tools: { total: 0, errors: 0 },
      }),
    });
  });

  // Record the initial StatusStrip / LeftRail DOM handles. If the shell
  // remounts on navigation these handles detach; the re-query will
  // return fresh nodes. We assert the *same* handles stay attached.
  const initialShell = await page.getByTestId("workbench-shell").elementHandle();
  expect(initialShell).not.toBeNull();

  // Chat → Wiki
  await page.getByTestId("nav-tab-wiki").click();
  await page.waitForURL(/\/wiki/);
  await expect(page.getByTestId("workbench-shell")).toBeVisible();
  await expect(page.getByTestId("left-rail")).toBeVisible();
  await expect(page.getByTestId("wiki-header")).toBeVisible();

  // Wiki → Dashboard
  await page.getByTestId("nav-tab-dashboard").click();
  await page.waitForURL(/\/dashboard/);
  await expect(page.getByTestId("workbench-shell")).toBeVisible();
  await expect(page.getByTestId("left-rail")).toBeVisible();
  await expect(page.getByTestId("dashboard-header")).toBeVisible();

  // Dashboard → Chat
  await page.getByTestId("nav-tab-chat").click();
  await page.waitForURL(/\/web\/?$/);
  await expect(page.getByTestId("workbench-shell")).toBeVisible();
  await expect(page.getByTestId("left-rail")).toBeVisible();
  await expect(page.getByTestId("chat-header")).toBeVisible();

  // The initial workbench-shell element handle must still be attached —
  // route transitions inside the (shell) route group do not remount it.
  const persisted = await initialShell!.evaluate(
    (el) => el.isConnected,
  );
  expect(persisted).toBe(true);
});
