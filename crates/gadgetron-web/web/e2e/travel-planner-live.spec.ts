import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R1_5_LIVE === "1";
const email = process.env.GADGETRON_R1_5_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_5_PASSWORD ?? "";

test.describe("Travel Planner live product projection", () => {
  test.skip(!live, "set GADGETRON_R1_5_LIVE=1 for the signed 18085 fixture");

  test("Trips, Itinerary, Dashboard, Penny, and Server coexist in one capability snapshot", async ({ page }) => {
    test.setTimeout(45_000);
    expect(password, "GADGETRON_R1_5_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    await expect(page.getByTestId("version-badge")).toContainText("0.6.3");

    const trips = page.getByTestId("nav-workspace-travel-planner-trips");
    const itinerary = page.getByTestId("nav-workspace-travel-planner-itinerary");
    const servers = page.getByTestId("nav-workspace-server-administrator-fleet");
    await expect(trips).toBeVisible();
    await expect(itinerary).toBeVisible();
    await expect(servers).toBeVisible();

    await trips.click();
    await expect(page.getByRole("heading", { name: "Trips" })).toBeVisible();
    await expect(page.getByText("Seoul Systems Tour — revised", { exact: true })).toBeVisible();
    await expect(page.getByText("Asia/Seoul", { exact: true })).toBeVisible();
    const tripUpsert = page.locator("details").filter({ hasText: "Create a Trip or replace its current revision" });
    await expect(tripUpsert.getByLabel("title *")).not.toBeVisible();
    await tripUpsert.locator("summary").click();
    await expect(tripUpsert.getByLabel("title *")).toBeVisible();

    await page.getByRole("button", { name: "Ask Penny" }).click();
    await expect(page.getByTestId("penny-companion")).toContainText("Trips");

    await itinerary.click();
    await expect(page.getByRole("heading", { name: "Itinerary" })).toBeVisible();
    await expect(page.getByText("KTX arrival", { exact: true })).toBeVisible();
    await expect(page.getByText(/Seoul Systems Tour — revised · Seoul Station · Asia\/Seoul · transport · confirmed/)).toBeVisible();

    await page.getByTestId("nav-tab-dashboard").click();
    await expect(page.getByText("Travel plans", { exact: true })).toBeVisible();
    await expect(page.getByText("Server fleet", { exact: true })).toBeVisible();

    const projection = await page.evaluate(async () => {
      const response = await fetch("/api/v1/web/workbench/capabilities", { credentials: "include" });
      return response.json() as Promise<{
        revision: string;
        ui_contributions: Array<{ owner_bundle: string; kind: string; gadget_name?: string }>;
      }>;
    });
    expect(projection.revision).toHaveLength(64);
    expect(projection.ui_contributions).toEqual(expect.arrayContaining([
      expect.objectContaining({ owner_bundle: "travel-planner", kind: "tool_result", gadget_name: "travel.trip-get" }),
      expect.objectContaining({ owner_bundle: "travel-planner", kind: "tool_result", gadget_name: "travel.itinerary-list" }),
      expect.objectContaining({ owner_bundle: "server-administrator", kind: "dashboard_widget" }),
    ]));

    await page.screenshot({ path: "../../../.gadgetron/r1-5-browser.png", fullPage: true });
  });
});
