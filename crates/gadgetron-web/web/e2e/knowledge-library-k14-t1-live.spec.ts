import { expect, test, type Page } from "@playwright/test";

import { expectAccessible, expectReadableTextControls } from "./support/ui-assertions";

const live = process.env.GADGETRON_K14_T1_LIVE === "1";
const email = process.env.GADGETRON_K14_T1_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K14_T1_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";
const knowledgeApi = "/api/v1/web/workbench/knowledge";

type Space = { id: string; kind: string; title: string };
type Source = {
  id: string;
  title: string;
  original_name: string;
  status: string;
  extracted_object_id?: string;
  failure_detail?: string;
};

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("browses the Library preview, opens Quick Look, and retries an honest failure", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K14_T1_LIVE=1 for the 18085 Knowledge service");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_K14_T1_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");
  await login(page);

  const spacesResponse = await page.request.get(`${knowledgeApi}/spaces`);
  expect(spacesResponse.ok(), await spacesResponse.text()).toBeTruthy();
  const spaces = (await spacesResponse.json() as { spaces: Space[] }).spaces;
  let fixture: { space: Space; extracted: Source; failed: Source } | undefined;
  for (const space of spaces) {
    const response = await page.request.get(`${knowledgeApi}/spaces/${space.id}/sources`);
    if (!response.ok()) continue;
    const sources = (await response.json() as { sources: Source[] }).sources;
    const extracted = sources.find((source) => source.status === "extracted" && source.extracted_object_id);
    const failed = sources.find((source) => source.status === "failed" || source.status === "needs_ocr");
    if (extracted && failed) { fixture = { space, extracted, failed }; break; }
  }
  expect(fixture, "an accessible Space needs one previewable and one retryable source").toBeTruthy();

  await page.goto(`/web/knowledge?workspace=sources&space=${fixture!.space.id}`);
  await expect(page.getByRole("button", { name: "Materials", exact: true })).toHaveAttribute("aria-current", "page");
  const scope = page.getByRole("group", { name: "Visibility" });
  await expect(scope).toBeVisible();
  await expect(scope.getByRole("button")).toHaveCount(3);
  const domainToggle = page.getByRole("button", { name: "Topic library" });
  await expect(domainToggle).toHaveAttribute("aria-expanded", "false");
  await domainToggle.click();
  await expect(page.getByRole("complementary", { name: "Topic library" })).toBeVisible();

  const extractedRow = page.locator(`[data-source-row="${fixture!.extracted.id}"]`);
  await extractedRow.click();
  await expect(page.getByTestId("library-source-preview")).toContainText(fixture!.extracted.title || fixture!.extracted.original_name);
  await expect(page.getByRole("button", { name: "Open Quick Look" })).toBeEnabled();
  await extractedRow.focus();
  await page.keyboard.press("Space");
  const quickLook = page.getByRole("dialog");
  await expect(quickLook).toBeVisible();
  await expect(quickLook.getByRole("button", { name: "Skim" })).toBeVisible();
  await quickLook.getByRole("button", { name: "Read" }).click();
  await expect(quickLook.getByTestId("quick-look-body")).toHaveClass(/max-w-3xl/);
  await quickLook.getByRole("button", { name: "Close" }).click();

  const failedRow = page.locator(`[data-source-row="${fixture!.failed.id}"]`);
  const failureDot = failedRow.getByTestId("source-status-dot");
  await expect(failureDot).toHaveAttribute("title", /Extraction failed:/);
  await failedRow.click();
  await expect(page.getByTestId("library-source-preview")).toContainText(fixture!.failed.title || fixture!.failed.original_name);
  await expect(page.getByRole("button", { name: "Open Quick Look" })).toBeEnabled();
  const retryResponse = page.waitForResponse((response) => response.request().method() === "POST" && response.url().endsWith(`/sources/${fixture!.failed.id}/retry`));
  await page.getByRole("button", { name: "Retry" }).click();
  const retried = await retryResponse;
  expect([200, 422]).toContain(retried.status());
  if (retried.status() === 422) {
    await expect(failureDot).toHaveAttribute("title", /Extraction failed:/);
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  }

  await expectAccessible(page, "[data-testid='library-materials']");
  await expectReadableTextControls(page, "[data-testid='library-materials']");
  await page.screenshot({ path: "../../../.gadgetron/k14-t1-library-preview.png", fullPage: true });
});
