import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_8_ALERTS_LIVE === "1";
const email = process.env.GADGETRON_R1_8_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_8_PASSWORD ?? "";
const targetId = process.env.GADGETRON_R1_8_TARGET ?? "r1-fixture";
const evidenceRule = process.env.GADGETRON_R1_8_ALERT_RULE ?? "r1-cpu-util-evidence";
const bundleId = "server-administrator";

async function checkedJson<T>(response: Awaited<ReturnType<APIRequestContext["get"]>>): Promise<T> {
  if (!response.ok()) throw new Error(`HTTP ${response.status()}: ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

async function startCycle(request: APIRequestContext): Promise<{ job_id: string }> {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await request.post(
      `/api/v1/web/workbench/admin/bundles/${bundleId}/job-recipes/server-duty-cycle/start`,
      { data: { parameters: { target_id: targetId } } },
    );
    if (response.status() !== 409) return checkedJson(response);
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error("server target remained busy beyond its bounded monitoring cycle");
}

test("materializes a metric rule and renders persisted history through the signed workspace", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R1_8_ALERTS_LIVE=1 after seeding the bounded evidence rule");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_R1_8_PASSWORD is required").not.toBe("");
  await login(page);

  const accepted = await startCycle(page.request);
  await expect.poll(async () => {
    const report = await checkedJson<{ status: string }>(await page.request.get(
      `/api/v1/web/workbench/admin/bundles/${bundleId}/jobs/${accepted.job_id}`,
    ));
    return report.status;
  }, { timeout: 60_000 }).toBe("succeeded");

  const metrics = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
    await page.request.get(`/api/v1/web/workbench/views/${bundleId}.metrics/data`),
  );
  expect(metrics.payload.rows).toEqual(expect.arrayContaining([
    expect.objectContaining({ target_id: targetId, metric: "cpu.util", unit: "percent" }),
  ]));

  const alerts = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
    await page.request.get(`/api/v1/web/workbench/views/${bundleId}.alerts/data`),
  );
  expect(alerts.payload.rows).toEqual(expect.arrayContaining([
    expect.objectContaining({ source_id: evidenceRule, status: "firing", firing_count: 1 }),
  ]));

  await page.goto(`/web/workspace?id=${bundleId}.alerts`);
  await expect(page.getByRole("heading", { name: "Incidents" })).toBeVisible();
  await expect(page.getByRole("table").getByText(evidenceRule, { exact: true })).toBeVisible();

  await page.goto(`/web/workspace?id=${bundleId}.metrics`);
  await expect(page.getByRole("heading", { name: "Metrics" })).toBeVisible();
  const workspace = page.getByTestId("live-telemetry-workspace");
  await workspace.getByRole("button", { name: "5m" }).click();
  await expect(workspace.getByLabel("Metric").locator("option:checked")).toContainText("CPU utilization");
  await expect(workspace.getByRole("img", { name: /CPU utilization.*points.*percent/ })).toBeVisible();
  await expect(workspace.getByText("Sample table")).toBeVisible();
  await expect(page.locator("details").filter({ hasText: "server.metric-series" })).toHaveCount(0);
});
