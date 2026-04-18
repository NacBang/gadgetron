import { test, expect } from "@playwright/test";

// ---------------------------------------------------------------------------
// Failure e2e: mocked /health=503 — FailurePanel should appear
// ---------------------------------------------------------------------------

test.beforeEach(async ({ page }) => {
  await page.route("**/health", async (route) => {
    await route.fulfill({
      status: 503,
      contentType: "application/json",
      body: JSON.stringify({ status: "unavailable" }),
    });
  });

  await page.goto("/web");
  // Inject API key so we don't land on auth screen
  await page.evaluate(() => {
    localStorage.setItem("gadgetron_api_key", "gad_live_test_key");
  });
  await page.reload();
  // Allow polling interval to fire
  await page.waitForTimeout(1500);
});

test("FailurePanel appears with recovery text when health=503", async ({
  page,
}) => {
  await expect(page.getByTestId("failure-panel")).toBeVisible();

  const title = page.getByTestId("failure-title");
  await expect(title).toBeVisible();
  // 503 maps to degraded, not blocked
  await expect(title).toHaveText("Gateway degraded");

  const recovery = page.getByTestId("failure-recovery");
  await expect(recovery).toBeVisible();
  // Should contain actionable recovery text
  const recoveryText = await recovery.innerText();
  expect(recoveryText.length).toBeGreaterThan(20);
});

test("no empty assistant bubble appears alongside FailurePanel", async ({
  page,
}) => {
  await expect(page.getByTestId("failure-panel")).toBeVisible();
  // There should be no assistant avatar or empty response bubble in the DOM
  // (the failure panel is the only feedback the user needs)
  const avatarFallbacks = page.locator("[class*='AvatarFallback']");
  // Either none exist, or any that exist have non-empty text content
  const count = await avatarFallbacks.count();
  for (let i = 0; i < count; i++) {
    const text = await avatarFallbacks.nth(i).innerText();
    // An empty string avatar fallback would be an empty bubble
    expect(text.trim().length).toBeGreaterThan(0);
  }
});

test("status strip shows degraded state", async ({ page }) => {
  const strip = page.getByRole("status", { name: "Workbench status" });
  await expect(strip).toBeVisible();
  // 503 is degraded
  await expect(strip).toContainText(/Gateway degraded|Gateway unreachable/);
});

test("snapshot: empty state must not contain banned AI-template phrases", async ({
  page,
}) => {
  // Even under failure, if the empty state is visible it must not have banned copy
  const emptyState = page.getByTestId("chat-empty-state");
  const isVisible = await emptyState.isVisible().catch(() => false);
  if (isVisible) {
    const text = await emptyState.innerText();
    expect(text).not.toContain("무엇이든 물어보세요");
    expect(text).not.toContain("무엇을 도와드릴까요");
    expect(text).not.toContain("How can I help");
  }
});
