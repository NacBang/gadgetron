import { expect, test, type Page } from "@playwright/test";

import { expectAccessible, expectReadableTextControls } from "./support/ui-assertions";

const live = process.env.GADGETRON_K14_T3_LIVE === "1";
const email = process.env.GADGETRON_K14_T3_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K14_T3_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";
const knowledgeApi = "/api/v1/web/workbench/knowledge";

type Space = { id: string; effective_role?: string };
type Vault = { id: string; home_bundle_id: string; owner_state: string };
type Note = {
  object_id: string;
  revision: number;
  git_revision: string;
  properties: Record<string, unknown>;
};

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("opens a local related view, expands a neighbor, and finds its path", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K14_T3_LIVE=1 for the 18085 Knowledge service");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_K14_T3_PASSWORD or GADGETRON_ADMIN_PW is required").not.toBe("");
  await login(page);

  const spacesResponse = await page.request.get(`${knowledgeApi}/spaces`);
  expect(spacesResponse.ok(), await spacesResponse.text()).toBeTruthy();
  const spaces = (await spacesResponse.json() as { spaces: Space[] }).spaces;
  const writableRoles = new Set(["contributor", "curator", "manager"]);
  let fixture: { space: Space; vault: Vault } | undefined;
  for (const space of spaces.filter((candidate) => writableRoles.has(candidate.effective_role ?? ""))) {
    const vaultsResponse = await page.request.get(`${knowledgeApi}/spaces/${space.id}/vaults`);
    if (!vaultsResponse.ok()) continue;
    const vaults = (await vaultsResponse.json() as { vaults: Vault[] }).vaults;
    const vault = vaults.find((candidate) => candidate.owner_state === "enabled");
    if (vault) { fixture = { space, vault }; break; }
  }
  expect(fixture, "an accessible writable Knowledge vault is required").toBeTruthy();

  const suffix = Date.now().toString(36);
  const sourceTitle = `Local graph start ${suffix}`;
  const targetTitle = `Local graph neighbor ${suffix}`;
  let sourceNote: Note | undefined;
  let targetNote: Note | undefined;

  try {
    const createTarget = await page.request.post(
      `${knowledgeApi}/vaults/${fixture!.vault.id}/notes`,
      { data: { title: targetTitle } },
    );
    expect(createTarget.ok(), await createTarget.text()).toBeTruthy();
    targetNote = await createTarget.json() as Note;

    const createSource = await page.request.post(
      `${knowledgeApi}/vaults/${fixture!.vault.id}/notes`,
      { data: { title: sourceTitle } },
    );
    expect(createSource.ok(), await createSource.text()).toBeTruthy();
    sourceNote = await createSource.json() as Note;

    const linked = await page.request.put(
      `${knowledgeApi}/objects/${sourceNote.object_id}/note`,
      {
        data: {
          expected_revision: sourceNote.revision,
          expected_git_revision: sourceNote.git_revision,
          properties: sourceNote.properties,
          body: `# ${sourceTitle}\n\n[[${targetTitle}]]\n`,
        },
      },
    );
    expect(linked.ok(), await linked.text()).toBeTruthy();
    sourceNote = await linked.json() as Note;

    const currentTarget = await page.request.get(`${knowledgeApi}/objects/${targetNote.object_id}/note`);
    expect(currentTarget.ok(), await currentTarget.text()).toBeTruthy();
    targetNote = await currentTarget.json() as Note;
    const reverseLinked = await page.request.put(
      `${knowledgeApi}/objects/${targetNote.object_id}/note`,
      {
        data: {
          expected_revision: targetNote.revision,
          expected_git_revision: targetNote.git_revision,
          properties: targetNote.properties,
          body: `# ${targetTitle}\n\n[[${sourceTitle}]]\n`,
        },
      },
    );
    expect(reverseLinked.ok(), await reverseLinked.text()).toBeTruthy();
    targetNote = await reverseLinked.json() as Note;

    const fixtureNeighborhood = await page.request.post(`${knowledgeApi}/graph/neighborhood`, {
      data: {
        center_node_id: `note:${sourceNote.object_id}`,
        depth: 1,
        node_limit: 200,
        edge_limit: 500,
        direction: "both",
        relation_kinds: [],
        space_ids: [fixture!.space.id],
      },
    });
    expect(fixtureNeighborhood.ok(), await fixtureNeighborhood.text()).toBeTruthy();

    await page.goto(`/web/knowledge?workspace=notes&space=${fixture!.space.id}&bundle=${fixture!.vault.home_bundle_id}`);
    await expect(page.getByRole("button", { name: "Knowledge", exact: true })).toHaveAttribute("aria-current", "page");

    await page.getByRole("button", { name: "Graph explorer" }).click();
    await expect(page.getByTestId("graph-scope-step")).toBeVisible();
    await expect(page.getByTestId("interactive-graph-canvas")).toHaveCount(0);
    await page.getByRole("button", { name: "Knowledge", exact: true }).click();

    await page.getByLabel("Search knowledge").fill(sourceTitle);
    const neighborhoodResponse = page.waitForResponse((response) =>
      response.url().endsWith(`${knowledgeApi}/graph/neighborhood`)
        && response.request().method() === "POST");
    await page.getByRole("button", { name: `View related: ${sourceTitle}` }).click();
    const neighborhood = await neighborhoodResponse;
    expect(neighborhood.ok(), await neighborhood.text()).toBeTruthy();
    const related = page.getByTestId("related-knowledge-panel");
    await expect(related).toContainText(new RegExp(sourceTitle, "i"));
    await expect(related.getByTestId("interactive-graph-canvas")).toBeVisible();
    await expect(related.getByText("Confirmed relation")).toBeVisible();
    await related.getByTestId(`graph-node-note:${targetNote.object_id}`).click();
    await expect(related.getByRole("heading", { name: new RegExp(targetTitle, "i") })).toBeVisible();

    await related.getByRole("button", { name: "Explore path" }).click();
    await expect(page.getByRole("button", { name: "Graph explorer" })).toHaveAttribute("aria-current", "page");
    await expect(page.getByTestId("graph-scope-step")).toHaveCount(0);
    await page.getByLabel("Path destination").selectOption(`note:${sourceNote.object_id}`);
    await page.getByRole("button", { name: "Find path" }).click();
    await expect(page.getByText("1 knowledge path found")).toBeVisible();

    await expectAccessible(page, "main");
    await expectReadableTextControls(page, "main");
    await page.screenshot({ path: "../../../.gadgetron/k14-t3-local-graph-path.png", fullPage: true });
  } finally {
    for (const note of [sourceNote, targetNote]) {
      if (!note) continue;
      const current = await page.request.get(`${knowledgeApi}/objects/${note.object_id}/note`);
      if (!current.ok()) continue;
      const value = await current.json() as Note;
      await page.request.delete(`${knowledgeApi}/objects/${note.object_id}/note`, {
        data: { expected_revision: value.revision },
      });
    }
  }
});
