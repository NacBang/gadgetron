import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R2_4_LIVE === "1";
const email = process.env.GADGETRON_R1_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_4_PASSWORD ?? "";

type Space = { id: string; title: string; kind: string; effective_role?: string };
type Vault = { id: string; home_bundle_id: string; owner_state: string };
type Note = {
  object_id: string;
  revision: number;
  git_revision: string;
  properties: Record<string, unknown>;
};

test.describe("R2.4 Knowledge Workbench live journey", () => {
  test.skip(!live, "set GADGETRON_R2_4_LIVE=1 for the 18085 Knowledge fixture");

  test("searches, centers, inspects, shares, revokes and finds a typed path", async ({ page }) => {
    test.setTimeout(90_000);
    expect(password, "GADGETRON_R1_4_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);

    const listed = await page.request.get("/api/v1/web/workbench/knowledge/spaces");
    expect(listed.ok(), await listed.text()).toBeTruthy();
    const spaces = (await listed.json() as { spaces: Space[] }).spaces;
    expect(spaces.length).toBeGreaterThanOrEqual(2);

    let sourceSpace: Space | undefined;
    let sourceVault: Vault | undefined;
    const writableRoles = new Set(["contributor", "curator", "manager"]);
    for (const space of spaces.filter((candidate) => writableRoles.has(candidate.effective_role ?? ""))) {
      const response = await page.request.get(`/api/v1/web/workbench/knowledge/spaces/${space.id}/vaults`);
      if (!response.ok()) continue;
      const vaults = (await response.json() as { vaults: Vault[] }).vaults;
      const writableVault = vaults.find((vault) => vault.owner_state === "enabled");
      if (writableVault) {
        sourceSpace = space;
        sourceVault = writableVault;
        break;
      }
    }
    expect(sourceSpace).toBeTruthy();
    expect(sourceVault).toBeTruthy();
    const targetSpace = spaces.find((space) => space.id !== sourceSpace!.id)!;
    const suffix = Date.now().toString(36);
    const sourceTitle = `R2.4 Source ${suffix}`;
    const targetTitle = `R2.4 Target ${suffix}`;
    let sourceNote: Note | undefined;
    let targetNote: Note | undefined;

    try {
      const createTarget = await page.request.post(
        `/api/v1/web/workbench/knowledge/vaults/${sourceVault!.id}/notes`,
        { data: { title: targetTitle } },
      );
      expect(createTarget.ok(), await createTarget.text()).toBeTruthy();
      targetNote = await createTarget.json() as Note;

      const createSource = await page.request.post(
        `/api/v1/web/workbench/knowledge/vaults/${sourceVault!.id}/notes`,
        { data: { title: sourceTitle } },
      );
      expect(createSource.ok(), await createSource.text()).toBeTruthy();
      sourceNote = await createSource.json() as Note;

      const link = await page.request.put(
        `/api/v1/web/workbench/knowledge/objects/${sourceNote.object_id}/note`,
        {
          data: {
            expected_revision: sourceNote.revision,
            expected_git_revision: sourceNote.git_revision,
            properties: sourceNote.properties,
            body: `# ${sourceTitle}\n\n[[${targetTitle}]]\n`,
          },
        },
      );
      expect(link.ok(), await link.text()).toBeTruthy();
      sourceNote = await link.json() as Note;

      await page.getByTestId("nav-tab-wiki").click();
      await expect(page).toHaveURL(/\/web\/knowledge/);
      await expect(page.getByRole("button", { name: "Graph" })).toBeVisible();
      await page.getByLabel("Knowledge Space").selectOption(sourceSpace!.id);
      await page.getByLabel("Knowledge Domain").selectOption(sourceVault!.home_bundle_id);
      await page.getByRole("button", { name: "Graph" }).click();

      await page.getByLabel("Find graph center").fill(sourceTitle);
      await page.getByRole("button", { name: new RegExp(`${sourceTitle}.*note`) }).click();
      await expect(page.getByRole("complementary", { name: "Graph inspector" })).toContainText(sourceTitle);
      await expect(page.getByTestId(`graph-node-note:${sourceNote.object_id}`)).toBeVisible();

      await page.getByRole("button", { name: "Share", exact: true }).click();
      await page.getByLabel("Target Space").selectOption(targetSpace.id);
      await page.getByRole("dialog").getByRole("button", { name: "Share", exact: true }).click();
      const inspector = page.getByRole("complementary", { name: "Graph inspector" });
      await expect(inspector.getByText(targetSpace.title, { exact: true })).toBeVisible();
      await inspector.getByRole("button", { name: "Revoke share" }).click();
      await page.getByTestId("confirm-accept").click();
      await expect(inspector.getByText(targetSpace.title, { exact: true })).toHaveCount(0);

      await page.getByLabel("Path target").selectOption(`note:${targetNote.object_id}`);
      await page.getByRole("button", { name: "Find path" }).click();
      await expect(page.getByText("1 typed path found")).toBeVisible();
      await page.screenshot({ path: "../../../.gadgetron/r2-4-knowledge-workbench.png", fullPage: true });
    } finally {
      for (const note of [sourceNote, targetNote]) {
        if (!note) continue;
        const current = await page.request.get(`/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`);
        if (!current.ok()) continue;
        const value = await current.json() as Note;
        await page.request.delete(`/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`, {
          data: { expected_revision: value.revision },
        });
      }
    }
  });
});
