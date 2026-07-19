import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

import { expectAccessible, expectReadableTextControls } from "./support/ui-assertions";

const live = process.env.GADGETRON_K14_T2_LIVE === "1";
const email = process.env.GADGETRON_K14_T2_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K14_T2_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";
const databaseUrl = process.env.GADGETRON_DATABASE_URL ?? "";
const postgresContainer = process.env.GADGETRON_POSTGRES_CONTAINER ?? "gadgetron-pg";
const knowledgeApi = "/api/v1/web/workbench/knowledge";

interface Vault {
  id: string;
  space_id: string;
  home_bundle_id: string;
  owner_state: string;
}

interface Note {
  object_id: string;
  revision: number;
  body: string;
}

interface ChangeSet {
  id: string;
  status: string;
  summary: string;
  materialized_object_id?: string | null;
}

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

async function findWritableVault(request: APIRequestContext): Promise<Vault> {
  const spacesResponse = await request.get(`${knowledgeApi}/spaces`);
  expect(spacesResponse.ok(), await spacesResponse.text()).toBeTruthy();
  const spaces = await spacesResponse.json() as {
    spaces: Array<{ id: string; status: string; effective_role: string }>;
  };
  for (const space of spaces.spaces) {
    if (space.status !== "active" || !["contributor", "curator", "manager"].includes(space.effective_role)) continue;
    const response = await request.get(`${knowledgeApi}/spaces/${space.id}/vaults`);
    if (!response.ok()) continue;
    const vaults = (await response.json() as { vaults: Vault[] }).vaults;
    const vault = vaults.find((item) => item.owner_state === "enabled");
    if (vault) return vault;
  }
  throw new Error("K14-T2 requires one actor-writable enabled Knowledge domain");
}

async function listChangeSets(request: APIRequestContext, spaceId: string) {
  const response = await request.get(`${knowledgeApi}/spaces/${spaceId}/change-sets`);
  expect(response.ok(), await response.text()).toBeTruthy();
  return (await response.json() as { change_sets: ChangeSet[] }).change_sets;
}

async function archiveNote(request: APIRequestContext, objectId: string) {
  const current = await request.get(`${knowledgeApi}/objects/${objectId}/note`);
  if (!current.ok()) return;
  const note = await current.json() as Note;
  const removed = await request.delete(`${knowledgeApi}/objects/${objectId}/note`, {
    data: { expected_revision: note.revision },
  });
  expect(removed.ok(), await removed.text()).toBeTruthy();
}

function cleanupChangeSets(ids: string[]) {
  if (ids.length === 0 || !databaseUrl) return;
  if (ids.some((id) => !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(id))) {
    throw new Error("K14-T2 cleanup received an invalid change-set id");
  }
  const parsed = new URL(databaseUrl);
  const user = decodeURIComponent(parsed.username);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (!/^[a-zA-Z0-9_-]+$/.test(user) || !/^[a-zA-Z0-9_-]+$/.test(database)) {
    throw new Error("GADGETRON_DATABASE_URL has an unsupported user or database name");
  }
  const values = ids.map((id) => `'${id}'::uuid`).join(",");
  const result = spawnSync(
    "docker",
    ["exec", "-i", postgresContainer, "psql", "-XAtq", "-v", "ON_ERROR_STOP=1", "-U", user, "-d", database],
    { input: `DELETE FROM knowledge_change_sets WHERE id IN (${values});\n`, encoding: "utf8", maxBuffer: 1024 * 1024 },
  );
  if (result.status !== 0) throw new Error(`K14-T2 cleanup failed: ${(result.stderr || result.stdout).trim().slice(0, 300)}`);
}

test("compares exact duplicates, undoes one proposal, then reviews and applies the merge", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K14_T2_LIVE=1 for the 18085 cleanup inbox");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_K14_T2_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");
  expect(databaseUrl, "GADGETRON_DATABASE_URL is required for fixture cleanup").not.toBe("");
  await login(page);
  const vault = await findWritableVault(page.request);
  const suffix = randomUUID().slice(0, 8);
  const title = `K14 cleanup ${suffix}`;
  const createdIds: string[] = [];

  try {
    for (const [candidateTitle, body] of [
      [title, `# ${title}\n\nCheck the queue before retrying.\n`],
      [title.toUpperCase(), `# ${title}\n\nRecord the final worker state.\n`],
    ]) {
      const response = await page.request.post(`${knowledgeApi}/vaults/${vault.id}/notes`, {
        data: { title: candidateTitle, body },
      });
      expect(response.ok(), await response.text()).toBeTruthy();
      createdIds.push((await response.json() as Note).object_id);
    }

    await expect.poll(async () => {
      const response = await page.request.get(`${knowledgeApi}/spaces/${vault.space_id}/duplicate-groups`);
      if (!response.ok()) return false;
      const groups = (await response.json() as { groups: Array<{ candidates: Array<{ object_id: string }> }> }).groups;
      return groups.some((group) => createdIds.every((id) => group.candidates.some((candidate) => candidate.object_id === id)));
    }).toBe(true);

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "cleanup",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
    }).toString());
    await expect(page.getByRole("button", { name: "Cleanup", exact: true })).toHaveAttribute("aria-current", "page");
    const group = page.getByTestId("duplicate-group").filter({ hasText: title });
    await group.getByRole("button", { name: /notes may be the same knowledge/ }).click();
    await expect(group.locator("[data-conflict='true']").first()).toBeVisible();
    await expect(group.locator("[data-conflict='false']").first()).toBeVisible();
    await expect(group.locator("[data-paragraph-conflict='true']").first()).toBeVisible();
    await group.getByRole("button", { name: "Keep both" }).click();
    await group.getByRole("button", { name: "Prepare merge" }).click();
    await expect(page.getByText("1 merge ready for review").first()).toBeVisible();

    let fixtureChanges = (await listChangeSets(page.request, vault.space_id)).filter((change) => change.summary.toLowerCase().includes(title.toLowerCase()));
    expect(fixtureChanges.some((change) => change.status === "pending_user_review")).toBe(true);
    await page.getByRole("button", { name: "Undo" }).click();
    await expect.poll(async () => (await listChangeSets(page.request, vault.space_id))
      .filter((change) => change.summary.toLowerCase().includes(title.toLowerCase()))
      .some((change) => change.status === "rejected")).toBe(true);

    await group.getByRole("button", { name: "Prepare merge" }).click();
    await page.getByRole("button", { name: "Open review" }).click();
    await expect(page.getByRole("button", { name: "Review", exact: true })).toHaveAttribute("aria-current", "page");
    fixtureChanges = (await listChangeSets(page.request, vault.space_id)).filter((change) => change.summary.toLowerCase().includes(title.toLowerCase()));
    const pending = fixtureChanges.find((change) => change.status === "pending_user_review");
    expect(pending).toBeTruthy();
    const listButton = page.locator("button").filter({ hasText: "Merge 2 duplicate notes" }).first();
    await listButton.click();
    await expect(page.getByText(new RegExp(`Keep .${title}`, "i")).first()).toBeVisible();
    await page.getByRole("button", { name: "Accept", exact: true }).click();
    await expect.poll(async () => (await listChangeSets(page.request, vault.space_id))
      .find((change) => change.id === pending!.id)?.status).toBe("applied");
    const applied = (await listChangeSets(page.request, vault.space_id)).find((change) => change.id === pending!.id)!;
    expect(applied.materialized_object_id).toBeTruthy();
    createdIds.push(applied.materialized_object_id!);
    const mergedResponse = await page.request.get(`${knowledgeApi}/objects/${applied.materialized_object_id}/note`);
    expect(mergedResponse.ok(), await mergedResponse.text()).toBeTruthy();
    const merged = await mergedResponse.json() as Note;
    expect(merged.body).toContain("Check the queue before retrying.");
    expect(merged.body).toContain("Record the final worker state.");

    await expectAccessible(page, "[data-testid='knowledge-workspace-tabs']");
    await expectReadableTextControls(page, "[data-testid='knowledge-workspace-tabs']");
    await page.screenshot({ path: "../../../.gadgetron/k14-t2-cleanup-merge.png", fullPage: true });
  } finally {
    const fixtureChanges = (await listChangeSets(page.request, vault.space_id)).filter((change) => change.summary.toLowerCase().includes(title.toLowerCase()));
    for (const change of fixtureChanges) {
      if (change.materialized_object_id && !createdIds.includes(change.materialized_object_id)) createdIds.push(change.materialized_object_id);
    }
    for (const objectId of [...new Set(createdIds)].reverse()) await archiveNote(page.request, objectId);
    cleanupChangeSets(fixtureChanges.map((change) => change.id));
  }
});
