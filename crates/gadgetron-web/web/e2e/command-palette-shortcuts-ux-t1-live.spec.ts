import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_UX_T1_LIVE === "1";
const email = process.env.GADGETRON_UX_T1_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_UX_T1_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";

test("opens Cmd+K, navigates, runs an action, and persists a rail shortcut", async ({ page }) => {
  test.skip(!live, "set GADGETRON_UX_T1_LIVE=1 for the 18085 shell");
  expect(password, "GADGETRON_UX_T1_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);

  // The URL can settle before the client shell has hydrated its global
  // keyboard listener. Shortcuts are client-only and therefore make a stable
  // readiness signal for the same shell.
  await expect(page.getByTestId("rail-shortcuts")).toBeVisible();
  await page.keyboard.press("Control+K");
  const palette = page.getByTestId("command-palette");
  await expect(palette).toBeVisible();
  await palette.getByRole("combobox", { name: "Search commands" }).fill("dashboard");
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/web\/dashboard/);

  await expect(page.getByTestId("rail-shortcuts")).toBeVisible();
  await page.keyboard.press("Control+K");
  await page.getByRole("option", { name: /^Add material/ }).click();
  await expect(page).toHaveURL(/\/web\/knowledge\?.*workspace=sources/);
  await expect(page.getByRole("dialog", { name: "Add material" })).toBeVisible();
  await page.getByRole("button", { name: "Close" }).click();

  const shortcuts = page.getByTestId("rail-shortcuts");
  await expect(shortcuts.getByRole("link", { name: "Dashboard" })).toBeVisible();
  await shortcuts.getByRole("button", { name: "Pin Dashboard" }).click();
  await expect(shortcuts.getByText("Pinned")).toBeVisible();

  await page.reload();
  await expect(page.getByTestId("rail-shortcuts").getByRole("button", { name: "Unpin Dashboard" })).toBeVisible();
  await page.getByTestId("rail-shortcuts").getByRole("link", { name: "Dashboard" }).click();
  await expect(page).toHaveURL(/\/web\/dashboard/);
});
