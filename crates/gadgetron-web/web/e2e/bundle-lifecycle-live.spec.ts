import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_6_LIVE === "1";
const email = process.env.GADGETRON_R1_6_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_6_PASSWORD ?? "";

const SERVER = "server-administrator";
const TRAVEL = "travel-planner";
const RESTAURANT = "restaurant-research";

type PackageEnvelope = Record<string, unknown>;
type CapabilitySnapshot = {
  revision: string;
  bundles: Array<{ bundle_id: string; package_digest: string }>;
  views: Array<{ id: string; owner_bundle: string }>;
  actions: Array<{ id: string; owner_bundle: string }>;
  ui_contributions: Array<{ id: string; owner_bundle: string }>;
};
type BundleInspection = {
  source_sha256: string;
  package_manifest_sha256: string;
  permission_ids: string[];
};
type BundleRow = {
  bundle?: { id: string };
  runtime?: { state: string };
};

async function checkedJson<T>(response: Awaited<ReturnType<APIRequestContext["get"]>>): Promise<T> {
  if (!response.ok()) {
    throw new Error(`HTTP ${response.status()}: ${await response.text()}`);
  }
  return response.json() as Promise<T>;
}

async function capabilities(request: APIRequestContext): Promise<CapabilitySnapshot> {
  return checkedJson(await request.get("/api/v1/web/workbench/capabilities"));
}

async function viewRows(request: APIRequestContext, id: string): Promise<Array<Record<string, unknown>>> {
  const body = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
    await request.get(`/api/v1/web/workbench/views/${id}/data`),
  );
  return body.payload.rows;
}

async function exportPackage(request: APIRequestContext, bundleId: string): Promise<PackageEnvelope> {
  return checkedJson(await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/export`));
}

async function reloadCatalog(request: APIRequestContext): Promise<void> {
  await checkedJson(await request.post("/api/v1/web/workbench/admin/reload-catalog"));
}

async function installedBundles(request: APIRequestContext): Promise<BundleRow[]> {
  const body = await checkedJson<{ bundles: BundleRow[] }>(
    await request.get("/api/v1/web/workbench/admin/bundles"),
  );
  return body.bundles;
}

async function inspectPackage(
  request: APIRequestContext,
  envelope: PackageEnvelope,
): Promise<BundleInspection> {
  return checkedJson(
    await request.post("/api/v1/web/workbench/admin/bundles/inspect", {
      data: { source: { kind: "inline", envelope } },
    }),
  );
}

async function ensureEnabled(
  request: APIRequestContext,
  bundleId: string,
  envelope: PackageEnvelope,
): Promise<void> {
  const inspection = await inspectPackage(request, envelope);
  const rows = await installedBundles(request);
  const installed = rows.find((row) => row.bundle?.id === bundleId);
  if (!installed) {
    await checkedJson(
      await request.post("/api/v1/web/workbench/admin/bundles/install", {
        data: {
          source: { kind: "inline", envelope },
          expected_source_sha256: inspection.source_sha256,
        },
      }),
    );
    await reloadCatalog(request);
  }
  if (installed?.runtime?.state !== "enabled") {
    await checkedJson(
      await request.put(`/api/v1/web/workbench/admin/bundles/${bundleId}/permissions`, {
        data: {
          package_manifest_sha256: inspection.package_manifest_sha256,
          permission_ids: inspection.permission_ids,
        },
      }),
    );
    await checkedJson(
      await request.post(`/api/v1/web/workbench/admin/bundles/${bundleId}/enable`),
    );
  }
}

async function removeBundle(request: APIRequestContext, bundleId: string): Promise<void> {
  await checkedJson(
    await request.post(`/api/v1/web/workbench/admin/bundles/${bundleId}/disable`),
  );
  const removed = await checkedJson<{ state_preserved: boolean }>(
    await request.delete(`/api/v1/web/workbench/admin/bundles/${bundleId}`),
  );
  expect(removed.state_preserved).toBe(true);
  await reloadCatalog(request);
}

async function markdownExport(request: APIRequestContext, tripId: string): Promise<Record<string, unknown>> {
  const body = await checkedJson<{ result: { status: string; payload: Record<string, unknown> } }>(
    await request.post(
      "/api/v1/web/workbench/actions/travel-planner.trips.action.travel.export",
      { data: { args: { trip_id: tripId, format: "markdown" } } },
    ),
  );
  expect(body.result.status).toBe("ok");
  return body.result.payload;
}

function bundleIds(snapshot: CapabilitySnapshot): string[] {
  return snapshot.bundles.map((bundle) => bundle.bundle_id).sort();
}

async function login(page: Page): Promise<void> {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test.describe("signed Bundle lifecycle closure", () => {
  test.skip(!live, "set GADGETRON_R1_6_LIVE=1 for the signed 18085 lifecycle fixture");

  test("three Bundles preserve other surfaces and orphaned records through selective reinstall", async ({ page }) => {
    test.setTimeout(150_000);
    expect(password, "GADGETRON_R1_6_PASSWORD is required").not.toBe("");
    await login(page);

    const request = page.request;
    const baseline = await capabilities(request);
    expect(bundleIds(baseline)).toEqual([RESTAURANT, SERVER, TRAVEL]);

    const packages = {
      [SERVER]: await exportPackage(request, SERVER),
      [TRAVEL]: await exportPackage(request, TRAVEL),
      [RESTAURANT]: await exportPackage(request, RESTAURANT),
    };
    // Never remove the only installed copy until both exported envelopes have
    // passed the same validator used by reinstall.
    await inspectPackage(request, packages[SERVER]);
    await inspectPackage(request, packages[TRAVEL]);
    await inspectPackage(request, packages[RESTAURANT]);
    const serverRowsBefore = await viewRows(request, "server-administrator.servers");
    const tripRowsBefore = await viewRows(request, "travel-planner.trips");
    const itineraryRowsBefore = await viewRows(request, "travel-planner.itinerary");
    const restaurantRowsBefore = await viewRows(request, "restaurant-research.restaurants");
    expect(serverRowsBefore.length).toBeGreaterThan(0);
    expect(tripRowsBefore.length).toBeGreaterThan(0);
    expect(itineraryRowsBefore.length).toBeGreaterThan(0);
    expect(restaurantRowsBefore.length).toBeGreaterThan(0);
    const tripId = String(tripRowsBefore[0].trip_id);
    const exportBefore = await markdownExport(request, tripId);

    let mutationStarted = false;
    try {
      mutationStarted = true;
      await removeBundle(request, RESTAURANT);
      expect(bundleIds(await capabilities(request))).toEqual([SERVER, TRAVEL]);
      expect(await viewRows(request, "server-administrator.servers")).toEqual(serverRowsBefore);
      expect(await viewRows(request, "travel-planner.itinerary")).toEqual(itineraryRowsBefore);

      await page.goto("/web/workspace?id=restaurant-research.restaurants");
      await expect(page.getByText("Workspace unavailable", { exact: true })).toBeVisible();
      await expect(page.getByTestId("nav-workspace-restaurant-research-restaurants")).toHaveCount(0);
      await expect(page.getByTestId("nav-workspace-server-administrator-fleet")).toBeVisible();
      await expect(page.getByTestId("nav-workspace-travel-planner-trips")).toBeVisible();

      await removeBundle(request, TRAVEL);
      expect(bundleIds(await capabilities(request))).toEqual([SERVER]);

      await page.goto("/web/workspace?id=travel-planner.itinerary");
      await expect(page.getByText("Workspace unavailable", { exact: true })).toBeVisible();
      await expect(page.getByRole("link", { name: "Open Bundle management" })).toBeVisible();
      await expect(page.getByTestId("nav-workspace-server-administrator-fleet")).toBeVisible();
      await expect(page.getByTestId("nav-workspace-travel-planner-trips")).toHaveCount(0);

      await removeBundle(request, SERVER);
      const coreOnly = await capabilities(request);
      expect(bundleIds(coreOnly)).toEqual([]);
      expect(coreOnly.views).toEqual([]);
      expect(coreOnly.actions).toEqual([]);
      expect(coreOnly.ui_contributions).toEqual([]);
      expect((await request.get("/health")).ok()).toBe(true);

      await page.goto("/web");
      await expect(page.getByTestId("chat-column")).toBeVisible();
      await expect(page.getByTestId("nav-workspace-server-administrator-fleet")).toHaveCount(0);
      await expect(page.getByTestId("nav-workspace-travel-planner-trips")).toHaveCount(0);

      await ensureEnabled(request, SERVER, packages[SERVER]);
      expect(bundleIds(await capabilities(request))).toEqual([SERVER]);
      expect(await viewRows(request, "server-administrator.servers")).toEqual(serverRowsBefore);

      await page.reload();
      const servers = page.getByTestId("nav-workspace-server-administrator-fleet");
      await expect(servers).toBeVisible();
      await expect(page.getByTestId("nav-workspace-travel-planner-trips")).toHaveCount(0);
      await servers.click();
      await page.getByTestId("workspace-tabs").getByRole("link", { name: "Servers" }).click();
      await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
      await page.getByRole("button", { name: "Ask Penny" }).click();
      await expect(page.getByTestId("penny-companion")).toContainText("Servers");

      await ensureEnabled(request, TRAVEL, packages[TRAVEL]);
      expect(bundleIds(await capabilities(request))).toEqual([SERVER, TRAVEL]);
      expect(await viewRows(request, "server-administrator.servers")).toEqual(serverRowsBefore);
      expect(await viewRows(request, "travel-planner.trips")).toEqual(tripRowsBefore);
      expect(await viewRows(request, "travel-planner.itinerary")).toEqual(itineraryRowsBefore);
      expect(await markdownExport(request, tripId)).toEqual(exportBefore);

      await page.reload();
      const trips = page.getByTestId("nav-workspace-travel-planner-trips");
      await expect(trips).toBeVisible();
      await expect(page.getByTestId("nav-workspace-server-administrator-fleet")).toBeVisible();
      await trips.click();
      await expect(page.getByRole("heading", { name: "Trips" })).toBeVisible();
      await page.getByRole("button", { name: "Ask Penny" }).click();
      await expect(page.getByTestId("penny-companion")).toContainText("Trips");

      await ensureEnabled(request, RESTAURANT, packages[RESTAURANT]);
      expect(bundleIds(await capabilities(request))).toEqual([RESTAURANT, SERVER, TRAVEL]);
      expect(await viewRows(request, "server-administrator.servers")).toEqual(serverRowsBefore);
      expect(await viewRows(request, "travel-planner.itinerary")).toEqual(itineraryRowsBefore);
      expect(await viewRows(request, "restaurant-research.restaurants")).toEqual(restaurantRowsBefore);

      await page.reload();
      const restaurants = page.getByTestId("nav-workspace-restaurant-research-restaurants");
      await expect(restaurants).toBeVisible();
      await expect(page.getByTestId("nav-workspace-server-administrator-fleet")).toBeVisible();
      await expect(page.getByTestId("nav-workspace-travel-planner-trips")).toBeVisible();
    } finally {
      if (mutationStarted) {
        await ensureEnabled(request, SERVER, packages[SERVER]);
        await ensureEnabled(request, TRAVEL, packages[TRAVEL]);
        await ensureEnabled(request, RESTAURANT, packages[RESTAURANT]);
        await reloadCatalog(request);
      }
    }
  });
});
