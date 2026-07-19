import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_K14_T7_LIVE === "1";
const email = process.env.GADGETRON_K14_T7_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K14_T7_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";
const bodyOnlyQuery = process.env.GADGETRON_K14_T7_BODY_QUERY ?? "c691888c";

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("redirects legacy Wiki URLs and exposes a server body-text match", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K14_T7_LIVE=1 for the 18085 Knowledge service");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_K14_T7_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");
  await login(page);

  for (const [legacy, expectedQuery] of [
    ["/web/wiki", null],
    ["/web/wiki?q=thermal%20runbook", "thermal runbook"],
    ["/web/wiki?page=ops%2Frecovery", "ops/recovery"],
  ] as const) {
    await page.goto(legacy);
    await expect.poll(() => new URL(page.url()).pathname).toBe("/web/knowledge");
    await expect(page.getByLabel("Search knowledge")).toHaveValue(expectedQuery ?? "");
    await expect.poll(() => new URL(page.url()).searchParams.get("q")).toBe(expectedQuery);
  }

  const search = page.getByLabel("Search knowledge");
  await search.fill(bodyOnlyQuery);
  const result = page
    .getByTestId("knowledge-full-text-result")
    .filter({ hasText: bodyOnlyQuery })
    .first();
  await expect(result).toBeVisible();
  await expect(result).toContainText("Full text");
  const title = result.locator("span.block").first();
  expect((await title.innerText()).toLocaleLowerCase()).not.toContain(
    bodyOnlyQuery.toLocaleLowerCase(),
  );
});
