import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R2_5_LIVE === "1";
const email = process.env.GADGETRON_R1_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_4_PASSWORD ?? "";

test.describe("R2.5 Researcher and Gardener workspaces", () => {
  test.skip(!live, "set GADGETRON_R2_5_LIVE=1 for the 18085 Knowledge fixture");

  test("shows the durable Codex run and its reviewed change without placeholder copy", async ({ page }) => {
    test.setTimeout(60_000);
    expect(password, "GADGETRON_R1_4_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    await page.getByTestId("nav-tab-wiki").click();
    await page.getByLabel("Knowledge Space").selectOption("8cdec2a6-bcb5-4bce-b810-7bcf63e8ad65");

    await page.getByRole("button", { name: "Automation" }).click();
    await expect(page.getByText("What operational facts are supported by these two R2 knowledge fixtures?").first()).toBeVisible();
    const researcherRow = page.getByRole("row").filter({ hasText: "Researcher" }).filter({ hasText: "What operational facts" });
    await researcherRow.getByRole("button").click();
    await expect(page.getByText("Gadgetron 운영 상태 및 R2.3 기준선")).toBeVisible();
    await expect(page.getByText("Knowledge jobs unavailable")).toHaveCount(0);
    await page.screenshot({ path: "../../../.gadgetron/r2-5-jobs.png", fullPage: true });

    await page.getByRole("button", { name: "Review" }).click();
    await expect(page.getByText("Gadgetron 지식 정원 정리 — 2026-07-11 기준").first()).toBeVisible();
    await expect(page.getByText("Rejected").first()).toBeVisible();
    await expect(page.getByText("Candidate service unavailable")).toHaveCount(0);
    await page.screenshot({ path: "../../../.gadgetron/r2-5-candidates.png", fullPage: true });
  });
});
