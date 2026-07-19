import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R2_7_LIVE === "1";
const email = process.env.GADGETRON_R2_7_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R2_7_PASSWORD ?? "";
const projectSpace = process.env.GADGETRON_R2_7_PROJECT_SPACE
  ?? "8cdec2a6-bcb5-4bce-b810-7bcf63e8ad65";
const sourceNote = process.env.GADGETRON_R2_7_SOURCE_NOTE
  ?? "54eada7d-5a28-459e-aad4-e0fd8bb44382";

async function login(page: Page): Promise<void> {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test.describe("R2.7 organization Knowledge Graph", () => {
  test.skip(!live, "set GADGETRON_R2_7_LIVE=1 for the 18085 organization fixture");

  test("cross-Space graph exposes direct actions without internal UI prose", async ({ page }) => {
    expect(password, "GADGETRON_R2_7_PASSWORD is required").not.toBe("");
    await login(page);
    await page.goto(`/web/knowledge?workspace=graph&space=${projectSpace}&center=note:${sourceNote}`);

    await expect(page.getByTestId("version-badge")).toContainText("0.6.10");
    await expect(page.getByRole("button", { name: "Current domain" })).toBeVisible();
    await page.getByRole("button", { name: "Shared Mesh" }).click();
    await expect(page.getByRole("heading", { name: "R2.6 cited restaurant fixture" })).toBeVisible();

    await page.getByRole("button", { name: "Share", exact: true }).click();
    await expect(page.getByRole("dialog")).toContainText("Live reference");
    await expect(page.getByRole("dialog")).toContainText("Pinned snapshot");
    await expect(page.getByRole("dialog")).toContainText("Independent fork");
    await page.getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("button", { name: "Ask Penny" }).click();
    await expect(page.getByTestId("penny-companion")).toContainText("R2.6 cited restaurant fixture");
  });
});
