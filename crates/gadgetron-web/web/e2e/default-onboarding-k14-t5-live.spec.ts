import { expect, test, type Browser, type Page } from "@playwright/test";

const live = process.env.GADGETRON_K14_T5_LIVE === "1";
const userAEmail = process.env.GADGETRON_K14_T5_USER_A_EMAIL ?? "";
const userAPassword = process.env.GADGETRON_K14_T5_USER_A_PASSWORD ?? "";
const userBEmail = process.env.GADGETRON_K14_T5_USER_B_EMAIL ?? "";
const userBPassword = process.env.GADGETRON_K14_T5_USER_B_PASSWORD ?? "";
const privateSpaceId = process.env.GADGETRON_K14_T5_PRIVATE_SPACE_ID ?? "";
const privateSpaceTitle = process.env.GADGETRON_K14_T5_PRIVATE_SPACE_TITLE ?? "A팀 운영 지식";
const privateNoteTitle = process.env.GADGETRON_K14_T5_PRIVATE_NOTE_TITLE ?? "A팀 전용 운영 절차";

type Space = { id: string; title: string; kind: string };

async function login(browser: Browser, email: string, password: string): Promise<Page> {
  const context = await browser.newContext();
  const page = await context.newPage();
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  return page;
}

async function spaces(page: Page): Promise<Space[]> {
  const response = await page.request.get("/api/v1/web/workbench/knowledge/spaces");
  expect(response.ok(), await response.text()).toBeTruthy();
  return (await response.json() as { spaces: Space[] }).spaces;
}

async function openLibrary(page: Page, spaceId: string): Promise<void> {
  await page.goto(`/web/knowledge?workspace=notes&space=${spaceId}`);
  await expect(page.getByRole("button", { name: "Knowledge", exact: true })).toHaveAttribute(
    "aria-current",
    "page",
  );
}

test("default guide is shared while a private Team note stays hidden from non-members", async ({ browser }) => {
  test.skip(!live, "set GADGETRON_K14_T5_LIVE=1 with the two-user fixture");
  test.setTimeout(60_000);
  for (const [name, value] of Object.entries({
    userAEmail,
    userAPassword,
    userBEmail,
    userBPassword,
    privateSpaceId,
  })) {
    expect(value, `${name} is required`).not.toBe("");
  }

  const pageA = await login(browser, userAEmail, userAPassword);
  const spacesA = await spaces(pageA);
  const defaultSpaceA = spacesA.find((space) => space.kind === "team" && space.title === "Operations");
  expect(defaultSpaceA, "new users must see the default Operations Space").toBeTruthy();
  expect(spacesA.some((space) => space.id === privateSpaceId && space.title === privateSpaceTitle)).toBeTruthy();

  await openLibrary(pageA, defaultSpaceA!.id);
  await expect(pageA.getByText("What this Space is for", { exact: true })).toBeVisible();
  await openLibrary(pageA, privateSpaceId);
  await expect(pageA.getByText(privateNoteTitle, { exact: true })).toBeVisible();
  await pageA.screenshot({ path: "../../../.gadgetron/k14-t5-user-a.png", fullPage: true });

  const pageB = await login(browser, userBEmail, userBPassword);
  const spacesB = await spaces(pageB);
  const defaultSpaceB = spacesB.find((space) => space.kind === "team" && space.title === "Operations");
  expect(defaultSpaceB, "new users must see the default Operations Space").toBeTruthy();
  expect(spacesB.some((space) => space.id === privateSpaceId)).toBeFalsy();

  await openLibrary(pageB, defaultSpaceB!.id);
  await expect(pageB.getByText("What this Space is for", { exact: true })).toBeVisible();
  await expect(pageB.getByText(privateSpaceTitle, { exact: false })).toHaveCount(0);
  await expect(pageB.getByText(privateNoteTitle, { exact: true })).toHaveCount(0);
  const hiddenObjects = await pageB.request.get(
    `/api/v1/web/workbench/knowledge/spaces/${privateSpaceId}/objects?canonical_kind=note`,
  );
  expect([403, 404]).toContain(hiddenObjects.status());
  await pageB.screenshot({ path: "../../../.gadgetron/k14-t5-user-b.png", fullPage: true });

  await pageA.context().close();
  await pageB.context().close();
});
