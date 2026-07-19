import { expect, test, type Page } from "@playwright/test";

type JsonRecord = Record<string, unknown>;

const live = process.env.GADGETRON_METRIC_T1_LIVE === "1";
const email = process.env.GADGETRON_METRIC_T1_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_METRIC_T1_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";

function record(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonRecord
    : null;
}

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("renders non-zero GPU bars beside a labeled multi-series live trend", async ({ page }, testInfo) => {
  test.skip(!live, "set GADGETRON_METRIC_T1_LIVE=1 for a 18085 service with GPU telemetry");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_ADMIN_PW or GADGETRON_METRIC_T1_PASSWORD is required").not.toBe("");
  await login(page);

  const response = await page.request.get("/api/v1/web/workbench/views/server-administrator.metrics/data");
  expect(response.ok()).toBe(true);
  const body = record(await response.json());
  const data = record(body?.data);
  const payload = record(data?.payload) ?? record(body?.payload) ?? data ?? body;
  const rows = Array.isArray(payload?.rows) ? payload.rows.flatMap((value) => {
    const row = record(value);
    return row ? [row] : [];
  }) : [];
  const gpuRow = rows.find((row) =>
    typeof row.metric === "string"
    && /^gpu\.\d+\.util$/.test(row.metric)
    && record(row.presentation)?.visual === "bar"
    && typeof row.target_id === "string",
  );
  expect(gpuRow, "the live Metrics payload must include a signed GPU utilization bar").toBeTruthy();
  const targetId = String(gpuRow?.target_id);

  await page.goto("/web/workspace?id=server-administrator.metrics");
  const workspace = page.getByTestId("live-telemetry-workspace");
  await expect(workspace).toBeVisible();
  await workspace.getByLabel("Telemetry server").selectOption(targetId);

  const coexistence = workspace.getByTestId("gpu-current-and-trend");
  await expect(coexistence).toBeVisible();
  const comparison = coexistence.getByRole("img", { name: /GPU utilization comparison/i });
  await expect(comparison).toBeVisible();
  const bars = comparison.locator('[data-testid="gpu-comparison-bar"]');
  expect(await bars.count()).toBeGreaterThan(0);
  for (let index = 0; index < await bars.count(); index += 1) {
    const box = await bars.nth(index).boundingBox();
    expect(box?.width ?? 0, `GPU bar ${index} must have rendered width`).toBeGreaterThan(0);
    const value = Number(await bars.nth(index).getAttribute("data-series-value"));
    if (value > 0) {
      expect(box?.height ?? 0, `non-zero GPU bar ${index} must have rendered height`).toBeGreaterThan(0);
    }
  }

  await expect(coexistence.getByRole("list", { name: "Series legend" })).toHaveCount(2);
  const trend = coexistence.getByRole("img", { name: /GPU utilization recent live trend/i });
  await expect(trend.locator("path[data-series-label]").first()).toBeVisible({ timeout: 20_000 });
  await coexistence.scrollIntoViewIfNeeded();
  await coexistence.screenshot({ path: testInfo.outputPath("gpu-bars-and-live-trend.png") });
});
