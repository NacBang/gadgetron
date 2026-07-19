import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R2_6_LIVE === "1";
const email = process.env.GADGETRON_R2_6_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R2_6_PASSWORD ?? "";

test.describe("R2.6 Restaurant Research live journey", () => {
  test.skip(!live, "set GADGETRON_R2_6_LIVE=1 for the signed 18085 fixture");

  test("shows a cited recommendation and its Travel-owned itinerary bridge", async ({ page }) => {
    test.setTimeout(45_000);
    expect(password, "GADGETRON_R2_6_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    await expect(page.getByTestId("version-badge")).toContainText("0.6.10");

    const restaurants = page.getByTestId("nav-workspace-restaurant-research-restaurants");
    await expect(restaurants).toBeVisible();
    await restaurants.click();
    await expect(page.getByRole("heading", { name: "Restaurants" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Mapo Evidence Table" })).toBeVisible();
    await expect(page.getByText("Menu and location fit the budget; verify the current seating noise before visiting.")).toBeVisible();
    await expect(page.getByText("Evidence cited")).toBeVisible();
    await expect(page.getByText("Conflicting evidence")).toBeVisible();
    await expect(page.getByText("8f67c51d-62d1-4375-8ab6-2ab3d88e3aeb")).toHaveCount(0);
    const discuss = page.getByRole("button", { name: "Ask Penny for card 1" });
    await expect(discuss).toBeVisible();
    await page.screenshot({ path: "../../../.gadgetron/r2-6-restaurants.png", fullPage: true });
    await discuss.click();
    await expect(page.getByTestId("penny-companion")).toContainText("Mapo Evidence Table");

    await page.getByTestId("nav-workspace-travel-planner-itinerary").click();
    await expect(page.getByRole("heading", { name: "Itinerary" })).toBeVisible();
    await expect(page.getByText("Mapo Evidence Table", { exact: true })).toBeVisible();
    await expect(page.getByText(/Seoul Systems Tour — revised · Mapo-gu, Seoul · Asia\/Seoul · meal · proposed/)).toBeVisible();

    await page.screenshot({ path: "../../../.gadgetron/r2-6-restaurant-research.png", fullPage: true });
  });
});
