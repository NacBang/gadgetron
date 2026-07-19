import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_8_LIVE_METRICS === "1";
const email = process.env.GADGETRON_R1_8_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_8_PASSWORD ?? "";
const targetId = process.env.GADGETRON_R1_8_TARGET ?? "r1-fixture";

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("shows selected-server live metrics with freshness, pause and recent trend", async ({ page }, testInfo) => {
  test.skip(!live, "set GADGETRON_R1_8_LIVE_METRICS=1 for the actual OpenSSH fixture");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_R1_8_PASSWORD is required").not.toBe("");
  await login(page);

  await page.goto("/web/workspace?id=server-administrator.metrics");
  await expect(page.getByRole("heading", { name: "Metrics" })).toBeVisible();
  const workspace = page.getByTestId("live-telemetry-workspace");
  await expect(workspace).toBeVisible();
  await expect(workspace.getByLabel("Telemetry server")).toHaveValue(targetId);
  await expect(workspace.getByText("Live · 3s")).toBeVisible();
  await expect(workspace.getByText(/^Updated /).first()).toBeVisible();
  await expect(workspace.getByRole("progressbar", { name: "CPU utilization" })).toBeVisible();
  await expect(workspace.getByRole("img", { name: "CPU utilization recent live trend" })).toBeVisible({ timeout: 15_000 });
  await expect(workspace.getByText("cpu.util", { exact: true })).toHaveCount(0);
  await page.screenshot({ path: testInfo.outputPath("live-metrics.png"), fullPage: true });

  await workspace.getByRole("button", { name: "Pause" }).click();
  await expect(workspace.getByText("Paused", { exact: true })).toBeVisible();
  await expect(workspace.getByRole("button", { name: "Resume" })).toBeVisible();
});
