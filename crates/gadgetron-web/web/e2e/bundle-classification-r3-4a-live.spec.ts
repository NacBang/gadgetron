import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R3_4A_LIVE === "1";
const email = process.env.GADGETRON_R3_4A_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_4A_PASSWORD ?? "";

test("groups installed Bundles by their signed product class", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R3_4A_LIVE=1 for the signed 18085 fixture");
  expect(password, "GADGETRON_R3_4A_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.7.6");

  await page.goto("/web/admin");
  await page.getByRole("tab", { name: "Bundles" }).click();

  const operational = page.getByTestId("bundle-class-operational");
  await expect(operational).toContainText("Operational");
  await expect(operational).toContainText("server-administrator");
  await expect(operational).toContainText("travel-planner");

  const intelligence = page.getByTestId("bundle-class-intelligence");
  await expect(intelligence).toContainText("Intelligence");
  await expect(intelligence).toContainText("restaurant-research");
  await expect(page.getByTestId("bundle-class-legacy")).toHaveCount(0);

  await operational.getByRole("button", { name: /travel-planner/ }).click();
  await page.getByRole("button", { name: "Dependencies" }).click();
  await expect(page.getByText("Optional enhancements")).toBeVisible();
  await expect(page.getByText("restaurant assisted planning")).toBeVisible();
  await expect(page.getByText("satisfied", { exact: true })).toBeVisible();

  await intelligence.getByRole("button", { name: /restaurant-research/ }).click();
  await page.getByRole("button", { name: "Lifecycle" }).click();
  await page.getByRole("button", { name: "Preview disable" }).click();
  await expect(page.getByText("Disable impact")).toBeVisible();
  await expect(page.getByText(/travel-planner · restaurant assisted planning/)).toBeVisible();
  await expect(page.getByText("provider not enabled", { exact: true })).toBeVisible();
});
