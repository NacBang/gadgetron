import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R3_3_LIVE === "1";
const email = process.env.GADGETRON_R3_3_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_3_PASSWORD ?? "";

test("shows domain outcomes as a Manager mission view without exposing raw identifiers", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R3_3_LIVE=1 for the signed 18085 fixture");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_R3_3_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.7.6");

  await page.getByTestId("nav-tab-dashboard").click();
  await expect(page.getByText("Mission status", { exact: true })).toBeVisible();
  await expect(page.getByTestId("dashboard-vitals")).toContainText("System");
  await expect(page.getByText("Server fleet", { exact: true })).toBeVisible();
  await expect(page.getByText("Travel plans", { exact: true })).toBeVisible();
  await expect(page.getByText(/\{"type"|"tenant_id"|"operation_id"/)).toHaveCount(0);

  await page.getByTestId("nav-workspace-travel-planner-disruptions").click();
  await expect(page.getByRole("heading", { name: "Travel changes" })).toBeVisible();
  await expect(page.getByText("Transfer route closed", { exact: true })).toBeVisible();
  await expect(page.getByText("Airport transfer", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("Previous Plan Restored", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("3626a47c-57b6-4ec9-a01b-293e8429abcb", { exact: true })).toHaveCount(0);

  await page.goto("/web/workspace?id=server-administrator.servers");
  await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
  await expect(page.getByText("MOREH-JUNGHO", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Check monitoring" }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: "Restore monitoring" }).first()).toBeVisible();

  await page.screenshot({ path: "../../../.gadgetron/r3-3-browser.png", fullPage: true });
});
