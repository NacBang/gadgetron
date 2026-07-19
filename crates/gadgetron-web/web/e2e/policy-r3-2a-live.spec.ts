import { expect, test } from "@playwright/test";

const live = (process.env.GADGETRON_R3_2B_LIVE ?? process.env.GADGETRON_R3_2A_LIVE) === "1";
const email = process.env.GADGETRON_R3_2B_EMAIL ?? process.env.GADGETRON_R3_2A_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_2B_PASSWORD ?? process.env.GADGETRON_R3_2A_PASSWORD ?? "";

test("shows enforced paths, recent decisions, and a safe preview", async ({ page }) => {
  test.setTimeout(60_000);
  test.skip(!live, "set GADGETRON_R3_2B_LIVE=1 for the 18085 policy fixture");
  expect(password, "GADGETRON_R3_2B_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);

  await page.getByTestId("nav-tab-review").click();
  await page.getByRole("tab", { name: "Policy", exact: true }).click();

  const workspace = page.getByTestId("policy-workspace");
  await expect(workspace).toContainText("Active revision");
  await expect(workspace).toContainText("4 / 4 Enforced");
  await expect(workspace).toContainText("Tool calls");
  await expect(workspace).toContainText("Background jobs");
  await expect(workspace).toContainText("Bundle Gadgets");
  await expect(workspace).toContainText("Review resume");
  await expect(workspace).toContainText("Recent decisions");
  await expect(workspace.getByRole("button", { name: "Create revision" })).toBeDisabled();
  await page.getByTestId("review-page-header").scrollIntoViewIfNeeded();
  await page.screenshot({ path: "../../../.gadgetron/r3-2b-policy.png", fullPage: true });

  await workspace.getByRole("button", { name: "Evaluate decision" }).click();
  const result = page.getByTestId("policy-preview-result");
  await expect(result).toBeVisible();
  await expect(result).toContainText(/Auto|Review|Deny/);
  await expect(result).toContainText("Revision");
  await expect(result.getByText("Technical details", { exact: true })).toBeVisible();
  await page.screenshot({ path: "../../../.gadgetron/r3-2b-policy-result.png", fullPage: true });
});
