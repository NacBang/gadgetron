import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_FLEET_MAP_LIVE === "1";
const email = process.env.GADGETRON_FLEET_MAP_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_FLEET_MAP_PASSWORD ?? "";

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("renders the actual fleet as an accessible grouped hex map", async ({ page }) => {
  test.skip(!live, "set GADGETRON_FLEET_MAP_LIVE=1 for the 18085 development service");
  test.setTimeout(30_000);
  expect(password, "GADGETRON_FLEET_MAP_PASSWORD is required").not.toBe("");

  await login(page);
  const overviewResponse = await page.request.get(
    "/api/v1/web/workbench/views/server-administrator.fleet/data",
  );
  expect(overviewResponse.ok(), await overviewResponse.text()).toBe(true);
  const overviewBody = await overviewResponse.json() as {
    payload: { clusters: unknown[] };
  };
  expect(overviewBody.payload.clusters.length).toBeGreaterThanOrEqual(2);

  const mapResponse = await page.request.get(
    "/api/v1/web/workbench/views/server-administrator.fleet-map/data",
  );
  expect(mapResponse.ok(), await mapResponse.text()).toBe(true);
  const mapBody = await mapResponse.json() as {
    payload: { fleet: { total_servers: number; truncated: boolean }; servers: unknown[] };
  };
  expect(mapBody.payload.servers.length).toBeGreaterThanOrEqual(2);
  expect(mapBody.payload.fleet.total_servers).toBe(mapBody.payload.servers.length);
  expect(mapBody.payload.fleet.truncated).toBe(false);

  await page.goto("/web/workspace?id=server-administrator.fleet");
  await expect(page.getByTestId("fleet-host-map")).toHaveCount(0);
  await expect(page.getByTestId("workspace-tabs").getByRole("link", { name: "Fleet Map" })).toBeVisible();

  await page.goto("/web/workspace?id=server-administrator.fleet-map");
  const map = page.getByTestId("fleet-host-map");
  await expect(map).toBeVisible();
  await expect(map.locator('[role="list"] button')).toHaveCount(mapBody.payload.servers.length);
  await expect(map.getByText("Development operations A · Operations", { exact: true })).toBeVisible();
  await expect(map.getByText("GPU research B · Compute", { exact: true })).toBeVisible();

  await map.getByLabel("Fill").selectOption("gpu");
  await expect(map.getByTestId("fleet-fill-legend")).toContainText("Color shows magnitude, not health.");
  await expect(page).toHaveURL(/fleet_fill=gpu/);
  const firstServer = map.locator('[role="list"] button').first();
  await firstServer.focus();
  await expect(firstServer).toBeFocused();
  await firstServer.press("Enter");
  await expect(map.getByTestId("fleet-server-detail")).toBeVisible();
  const metricsLink = map.getByRole("link", { name: "Open Metrics" });
  await expect(metricsLink).toHaveAttribute("href", /asset=server%3A/);

  await map.getByTestId("fleet-list-fallback").locator("summary").click();
  await expect(map.getByRole("table")).toBeVisible();
  await map.getByLabel("Sort list").selectOption("status");
  await page.reload();
  await expect(page.getByTestId("fleet-host-map").getByLabel("Fill")).toHaveValue("gpu");
  await page.getByTestId("fleet-host-map").screenshot({
    path: "../../../.gadgetron/r4-4-fleet-map-tab.png",
  });
  const href = await page.getByTestId("fleet-host-map").getByRole("link", { name: "Open Metrics" }).getAttribute("href");
  const selectedAsset = new URL(href!, page.url()).searchParams.get("asset");
  expect(selectedAsset).toMatch(/^server:/);
  const selectedTarget = selectedAsset!.slice("server:".length);
  await page.getByTestId("fleet-host-map").getByRole("link", { name: "Open Metrics" }).click();
  await expect(page.getByLabel("Telemetry server")).toHaveValue(selectedTarget);
  await page.getByRole("group", { name: "Telemetry range" }).getByRole("button", { name: "30m" }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get("range")).toBe("30m");

  await page.getByTestId("workspace-tabs").getByRole("link", { name: "Incidents" }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get("id")).toBe("server-administrator.alerts");
  expect(new URL(page.url()).searchParams.get("asset")).toBe(selectedAsset);
  expect(new URL(page.url()).searchParams.get("range")).toBe("30m");
  await expect(page.getByLabel("Platform scope")).toContainText(`Server · ${selectedTarget} · 30m`);

  await page.getByTestId("workspace-tabs").getByRole("link", { name: "Metrics" }).click();
  await expect(page.getByLabel("Telemetry server")).toHaveValue(selectedTarget);
  await page.reload();
  await expect(page.getByLabel("Telemetry server")).toHaveValue(selectedTarget);
  await expect(page.getByRole("group", { name: "Telemetry range" }).getByRole("button", { name: "30m" })).toHaveAttribute("aria-pressed", "true");
  expect(new URL(page.url()).searchParams.get("asset")).toBe(selectedAsset);
  expect(new URL(page.url()).searchParams.get("range")).toBe("30m");
});
