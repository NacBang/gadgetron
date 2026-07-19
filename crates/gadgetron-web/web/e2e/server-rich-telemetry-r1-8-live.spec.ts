import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_8_TELEMETRY_LIVE === "1";
const email = process.env.GADGETRON_R1_8_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_8_PASSWORD ?? "";
const targetId = process.env.GADGETRON_R1_8_TARGET ?? "r1-fixture";
const expectGpu = process.env.GADGETRON_R1_8_EXPECT_GPU === "1";
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

test("collects rich server telemetry and explores the signed row in place", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R1_8_TELEMETRY_LIVE=1 for the actual OpenSSH fixture");
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

  const view = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
    await page.request.get(`/api/v1/web/workbench/views/${bundleId}.servers/data`),
  );
  const server = view.payload.rows.find((row) => row.target_id === targetId);
  expect(server).toMatchObject({
    health_status: "healthy",
    telemetry_status: "current",
    inventory: expect.objectContaining({ hostname: expect.any(String), gpus: expect.any(Array) }),
    telemetry: expect.objectContaining({
      cpu: expect.any(Object),
      memory: expect.any(Object),
      disks: expect.any(Array),
      network: expect.any(Array),
      availability: expect.any(Object),
      warnings: expect.any(Array),
    }),
  });
  if (expectGpu) {
    expect(Number(server!.gpu_count)).toBeGreaterThan(0);
    expect(server!.telemetry).toEqual(expect.objectContaining({
      gpus: expect.arrayContaining([expect.objectContaining({
        name: expect.any(String),
        memory_total_mib: expect.any(Number),
        temperature_c: expect.any(Number),
        power_w: expect.any(Number),
      })]),
    }));
  }

  await page.goto(`/web/workspace?id=${bundleId}.servers`);
  const table = page.getByRole("table");
  await expect(table.getByText(targetId, { exact: true })).toBeVisible();
  await table.getByRole("button", { name: "Inspect row 1" }).click();
  await expect(table.getByText("telemetry", { exact: true })).toBeVisible();
  await expect(table.getByText("availability", { exact: true }).first()).toBeVisible();
  await expect(table.getByText("cpu", { exact: true })).toBeVisible();
  await expect(table.getByText("memory", { exact: true })).toBeVisible();

  const graph = await checkedJson<{ payload: { nodes: Array<Record<string, unknown>>; edges: Array<Record<string, unknown>> } }>(
    await page.request.get(`/api/v1/web/workbench/views/${bundleId}.topology/data`),
  );
  expect(graph.payload.nodes).toEqual(expect.arrayContaining([
    expect.objectContaining({ id: `host:${targetId}`, kind: "host" }),
    expect.objectContaining({ kind: "network" }),
  ]));
  expect(graph.payload.edges).toEqual(expect.arrayContaining([
    expect.objectContaining({ source: `host:${targetId}`, kind: "membership" }),
  ]));

  await page.goto(`/web/workspace?id=${bundleId}.topology`);
  await expect(page.getByRole("heading", { name: "Topology" })).toBeVisible();
  await expect(page.getByTestId("interactive-graph-canvas")).toBeVisible();
  const hostNode = page.getByTestId(`graph-node-host:${targetId}`);
  await expect(hostNode).toBeVisible();
  await hostNode.click();
  await expect(page.getByText("Selected host")).toBeVisible();
});
