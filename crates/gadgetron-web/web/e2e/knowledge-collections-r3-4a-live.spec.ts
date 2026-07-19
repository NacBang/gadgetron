import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R3_4A_LIVE === "1";
const email = process.env.GADGETRON_R3_4A_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_4A_PASSWORD ?? "";

type Space = { id: string };
type Collection = { id: string; topic: string; bundle_id: string };

test("shows recurring source collection health and runs in human terms", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R3_4A_LIVE=1 for the signed 18085 service");
  expect(password, "GADGETRON_R3_4A_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.7.12");

  const spaceResponse = await page.request.get("/api/v1/web/workbench/knowledge/spaces");
  expect(spaceResponse.ok(), await spaceResponse.text()).toBeTruthy();
  const spaces = (await spaceResponse.json() as { spaces: Space[] }).spaces;
  let spaceId = "";
  let collection: Collection | undefined;
  for (const space of spaces) {
    const response = await page.request.get(
      `/api/v1/web/workbench/knowledge/spaces/${space.id}/collections`,
    );
    if (!response.ok()) continue;
    const rows = (await response.json() as { collections: Collection[] }).collections;
    const restaurant = rows.find((row) => row.bundle_id === "restaurant-research");
    if (restaurant) {
      spaceId = space.id;
      collection = restaurant;
      break;
    }
  }
  expect(collection).toBeTruthy();

  await page.goto(`/web/knowledge?workspace=collections&space=${spaceId}&bundle=restaurant-research`);
  await expect(page.getByRole("button", { name: "Topics" })).toHaveAttribute("aria-current", "page");
  const inspector = page.getByRole("complementary", { name: "Collection inspector" });
  await expect(inspector).toContainText(collection!.topic);
  await expect(inspector).toContainText("Source health");
  await expect(inspector).toContainText("Recent runs");
  await expect(inspector.getByText("Time Out Seoul restaurants", { exact: true }).first()).toBeVisible();
  await expect(inspector.getByText("Michelin Guide Korea restaurants", { exact: true }).first()).toBeVisible();
  await inspector.getByText("Technical details", { exact: true }).click();
  await expect(inspector.getByText("core-source-fetch", { exact: true })).toBeVisible();
});
