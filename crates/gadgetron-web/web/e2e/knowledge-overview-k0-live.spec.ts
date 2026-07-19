import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_K0_LIVE === "1";
const email = process.env.GADGETRON_R1_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_4_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";
const knowledgeApi = "/api/v1/web/workbench/knowledge";

type Space = { id: string; title: string };
type ChangeSet = { status: string };
type KnowledgeObject = {
  id: string;
  knowledge_kind: string;
  freshness: string;
  review_state?: string;
};

test("opens Knowledge with a semantic overview and a direct review path", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K0_LIVE=1 for the 18085 Knowledge service");
  test.setTimeout(60_000);
  expect(password, "GADGETRON_ADMIN_PW or GADGETRON_R1_4_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.8.15");

  const spacesResponse = await page.request.get(`${knowledgeApi}/spaces`);
  expect(spacesResponse.ok(), await spacesResponse.text()).toBeTruthy();
  const spaces = (await spacesResponse.json() as { spaces: Space[] }).spaces;

  let selected: { space: Space; changes: ChangeSet[]; objects: KnowledgeObject[] } | undefined;
  for (const space of spaces) {
    const [changeResponse, objectResponse] = await Promise.all([
      page.request.get(`${knowledgeApi}/spaces/${space.id}/change-sets`),
      page.request.get(`${knowledgeApi}/spaces/${space.id}/objects?canonical_kind=note`),
    ]);
    if (!changeResponse.ok() || !objectResponse.ok()) continue;
    const changes = (await changeResponse.json() as { change_sets: ChangeSet[] }).change_sets;
    const objects = (await objectResponse.json() as { objects: KnowledgeObject[] }).objects;
    if (changes.some((change) => change.status === "pending_user_review")) {
      selected = { space, changes, objects };
      break;
    }
  }
  expect(selected, "at least one accessible Space must have a pending Knowledge review").toBeTruthy();

  const pendingCount = selected!.changes.filter((change) => change.status === "pending_user_review").length;
  expect(selected!.objects.every((object) => object.knowledge_kind && object.freshness)).toBeTruthy();

  await page.goto(`/web/knowledge?space=${selected!.space.id}`);
  await expect(page.getByRole("button", { name: "Overview" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByTestId("knowledge-next-action")).toContainText(
    `Review ${pendingCount} proposed knowledge change${pendingCount === 1 ? "" : "s"}`,
  );
  await expect(page.getByText("Working notes", { exact: true })).toBeVisible();
  await expect(page.getByText("Lessons", { exact: true })).toBeVisible();
  await expect(page.getByText("Insights", { exact: true })).toBeVisible();
  await expect(page.getByLabel("Knowledge Space")).not.toContainText(/R\d+(?:\.\d+)*\s/);
  await page.screenshot({ path: "../../../.gadgetron/knowledge-k0-overview.png", fullPage: true });

  await page.getByRole("button", { name: "Open review" }).click();
  await expect(
    page.getByTestId("knowledge-workspace-tabs").getByRole("button", { name: "Review", exact: true }),
  ).toHaveAttribute("aria-current", "page");
  await expect(page.getByText("Candidate service unavailable")).toHaveCount(0);
  await page.screenshot({ path: "../../../.gadgetron/knowledge-k0-review.png", fullPage: true });
});
