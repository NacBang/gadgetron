import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_I18N_LIVE === "1";
const email = process.env.GADGETRON_I18N_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_I18N_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";

test("switches release-built login, chat, and Knowledge between English and Korean", async ({ page }) => {
  test.skip(!live, "set GADGETRON_I18N_LIVE=1 for the 18085 development service");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_I18N_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");

  await page.goto("/web/login");
  const loginEnglishSelector = page.getByRole("group", { name: "Language" });
  await expect(loginEnglishSelector.getByRole("button", { name: "Eng" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();
  await loginEnglishSelector.getByRole("button", { name: "Kor" }).click();
  const loginKoreanSelector = page.getByRole("group", { name: "언어" });
  await expect(loginKoreanSelector.getByRole("button", { name: "Kor" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByRole("heading", { name: "로그인" })).toBeVisible();
  await expect(page.getByRole("button", { name: "로그인", exact: true })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  await loginKoreanSelector.getByRole("button", { name: "Eng" }).click();

  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);

  const chatEmptyState = page.getByTestId("chat-empty-state");
  const shellEnglishSelector = page.getByRole("group", { name: "Language" });
  await expect(chatEmptyState.getByRole("heading", { name: "Ready" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Attach" })).toBeVisible();
  await shellEnglishSelector.getByRole("button", { name: "Kor" }).click();
  const shellKoreanSelector = page.getByRole("group", { name: "언어" });
  await expect(chatEmptyState.getByRole("heading", { name: "준비됨" })).toBeVisible();
  await expect(page.getByRole("button", { name: "첨부" })).toBeVisible();
  await shellKoreanSelector.getByRole("button", { name: "Eng" }).click();
  await expect(chatEmptyState.getByRole("heading", { name: "Ready" })).toBeVisible();

  await page.goto("/web/knowledge?workspace=sources");
  const workspaceTabs = page.getByTestId("knowledge-workspace-tabs");
  const englishSelector = page.getByRole("group", { name: "Language" });
  await expect(englishSelector.getByRole("button", { name: "Eng" })).toHaveAttribute("aria-pressed", "true");
  await expect(workspaceTabs.getByRole("button", { name: "Materials", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(workspaceTabs.getByRole("button", { name: "Knowledge", exact: true })).toBeVisible();
  await expect(workspaceTabs.getByRole("button", { name: "Review", exact: true })).toBeVisible();
  await expect(page.getByTestId("library-materials").getByRole("heading", { name: "Materials", exact: true })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");

  await englishSelector.getByRole("button", { name: "Kor" }).click();
  const koreanSelector = page.getByRole("group", { name: "언어" });
  await expect(koreanSelector.getByRole("button", { name: "Kor" })).toHaveAttribute("aria-pressed", "true");
  await expect(workspaceTabs.getByRole("button", { name: "자료", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(workspaceTabs.getByRole("button", { name: "지식", exact: true })).toBeVisible();
  await expect(workspaceTabs.getByRole("button", { name: "검토", exact: true })).toBeVisible();
  await expect(page.getByTestId("library-materials").getByRole("heading", { name: "자료", exact: true })).toBeVisible();
  await expect(page.locator("html")).toHaveAttribute("lang", "ko");
  expect(await page.evaluate(() => window.localStorage.getItem("gadgetron.locale"))).toBe("ko");

  await page.reload();
  await expect(page.getByRole("group", { name: "언어" }).getByRole("button", { name: "Kor" })).toHaveAttribute("aria-pressed", "true");
  await expect(workspaceTabs.getByRole("button", { name: "자료", exact: true })).toHaveAttribute("aria-current", "page");

  await page.getByRole("group", { name: "언어" }).getByRole("button", { name: "Eng" }).click();
  await expect(page.getByRole("group", { name: "Language" }).getByRole("button", { name: "Eng" })).toHaveAttribute("aria-pressed", "true");
  await expect(workspaceTabs.getByRole("button", { name: "Materials", exact: true })).toHaveAttribute("aria-current", "page");
});
