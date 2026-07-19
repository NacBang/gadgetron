import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R4_4_INCIDENT_SIGNALS_LIVE === "1";
const email = process.env.GADGETRON_R4_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R4_4_PASSWORD ?? "";
const marker = process.env.GADGETRON_R4_4_INCIDENT_MARKER ?? "B98 correlated memory pressure";

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("shows one human-readable incident for correlated metric and log signals", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_4_INCIDENT_SIGNALS_LIVE=1 after staging the bounded DB signals");
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");

  await login(page);
  await page.goto("/web/workspace?id=server-administrator.alerts");

  await expect(page.getByRole("heading", { name: "Incidents", exact: true })).toBeVisible();
  const card = page.getByText(marker, { exact: true }).locator("..", { has: page.getByText("2 signals · Logs, Metrics") });
  await expect(card).toBeVisible();
  await expect(card).toContainText("Log anomaly detected");
  await expect(card).toContainText("Open the finding and compare a verified runbook");
  await expect(card.getByRole("button", { name: /Ask Penny/ })).toBeVisible();
});
