import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_NAV_LIVE === "1";
const email = process.env.GADGETRON_NAV_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_NAV_PASSWORD ?? "";

test("groups Core and Bundle navigation by product purpose", async ({ page }) => {
  test.skip(!live, "set GADGETRON_NAV_LIVE=1 for the 18085 navigation fixture");
  expect(password, "GADGETRON_NAV_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);

  const operations = page.getByTestId("nav-section-operations");
  const planning = page.getByTestId("nav-section-planning");
  await expect(operations.getByText("Monitoring", { exact: true })).toBeVisible();
  await expect(planning).toHaveCount(0);
  await expect(
    operations.getByTestId("nav-workspace-server-administrator-fleet"),
  ).toBeVisible();
  await expect(
    operations.getByTestId("nav-workspace-server-administrator-topology"),
  ).toHaveCount(0);
  const sections = await page.evaluate(async () => {
    const response = await fetch("/api/v1/web/workbench/capabilities", {
      credentials: "include",
    });
    const body = await response.json() as {
      ui_contributions: Array<{
        id: string;
        kind: string;
        navigation_section?: string;
      }>;
    };
    return Object.fromEntries(
      body.ui_contributions
        .filter((item) => item.kind === "navigation")
        .map((item) => [item.id, item.navigation_section]),
    );
  });
  expect(sections).toMatchObject({
    "server-administrator.fleet-navigation": "operations",
    "server-administrator.servers-navigation": "operations",
    "server-administrator.logs-navigation": "diagnostics",
  });
  expect(sections).not.toHaveProperty("travel-planner.trips-navigation");
  expect(sections).not.toHaveProperty("travel-planner.itinerary-navigation");

  await operations.getByTestId("nav-workspace-server-administrator-fleet").click();
  const workspaceTabs = page.getByTestId("workspace-tabs");
  await expect(workspaceTabs.getByRole("link", { name: "Overview" })).toHaveAttribute(
    "aria-current",
    "page",
  );
  await expect(workspaceTabs.getByRole("link", { name: "Servers" })).toBeVisible();
  await expect(workspaceTabs.getByRole("link", { name: "Incidents" })).toBeVisible();
  await expect(workspaceTabs.getByRole("link", { name: "Metrics" })).toBeVisible();
  await expect(workspaceTabs.getByRole("link", { name: "Topology" })).toBeVisible();

  await page.getByRole("button", { name: "Collapse Monitoring" }).click();
  await expect(
    operations.getByTestId("nav-workspace-server-administrator-fleet"),
  ).toHaveCount(0);
  await expect(workspaceTabs.getByRole("link", { name: "Overview" })).toBeVisible();

  await page.getByTestId("left-rail-collapse-btn").click();
  await expect(page.getByText("Monitoring", { exact: true })).toHaveCount(0);
  await expect(operations.locator(":scope > div[aria-hidden='true']")).toHaveClass(/border-t/);
});
