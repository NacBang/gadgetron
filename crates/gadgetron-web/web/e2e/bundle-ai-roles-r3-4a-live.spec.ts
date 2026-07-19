import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R3_4A_LIVE === "1";
const email = process.env.GADGETRON_R3_4A_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_4A_PASSWORD ?? "";

test("shows signed Bundle AI roles and collection policy in human terms", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R3_4A_LIVE=1 for the signed 18085 service");
  expect(password, "GADGETRON_R3_4A_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.7.12");

  await page.goto("/web/admin");
  await page.getByRole("tab", { name: "Bundles" }).click();
  await page
    .getByTestId("bundle-class-intelligence")
    .getByRole("button", { name: /restaurant-research/ })
    .click();
  await page.getByRole("button", { name: "AI roles" }).click();

  await expect(page.getByText("Restaurant researcher", { exact: true })).toBeVisible();
  await expect(page.getByText("Tenant default", { exact: true }).first()).toBeVisible();
  await expect(page.getByText("Effective model", { exact: true })).toBeVisible();
  await expect(page.getByText("Restaurant source collection", { exact: true })).toBeVisible();
  await expect(page.getByText("official", { exact: true })).toBeVisible();
  await expect(page.getByText("editorial", { exact: true })).toBeVisible();
  await expect(page.getByText("1d", { exact: true })).toBeVisible();
  await expect(page.getByText("On demand", { exact: true })).toBeVisible();

  await page.getByText("Technical details", { exact: true }).click();
  await expect(page.getByText("restaurant-core-source-collection", { exact: true })).toBeVisible();
});
