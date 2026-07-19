import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_K12_LIVE === "1";
const email = process.env.GADGETRON_K12_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K12_PASSWORD ?? "";
const databaseUrl = process.env.GADGETRON_DATABASE_URL ?? "";
const postgresContainer = process.env.GADGETRON_POSTGRES_CONTAINER ?? "gadgetron-pg";

interface Identity {
  tenant_id: string;
  user_id: string;
}

interface Vault {
  id: string;
  space_id: string;
  home_bundle_id: string;
  owner_state: string;
}

interface Note {
  object_id: string;
  revision: number;
  git_revision: string;
  body: string;
}

interface ChangeSet {
  id: string;
  status: string;
  applied_git_revision?: string | null;
}

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

function databaseTarget() {
  if (!databaseUrl) throw new Error("GADGETRON_DATABASE_URL is required");
  const parsed = new URL(databaseUrl);
  const user = decodeURIComponent(parsed.username);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (!user || !database || !/^[a-zA-Z0-9_-]+$/.test(user) || !/^[a-zA-Z0-9_-]+$/.test(database)) {
    throw new Error("GADGETRON_DATABASE_URL has an unsupported user or database name");
  }
  return { user, database };
}

function runSql(sql: string, variables: Record<string, string> = {}) {
  const target = databaseTarget();
  const args = [
    "exec", "-i", postgresContainer,
    "psql", "-XAtq", "-v", "ON_ERROR_STOP=1",
    "-U", target.user, "-d", target.database,
  ];
  for (const [name, value] of Object.entries(variables)) {
    if (!/^[a-z_]+$/.test(name)) throw new Error(`invalid SQL variable name: ${name}`);
    args.push("-v", `${name}=${value}`);
  }
  const result = spawnSync("docker", args, {
    input: sql,
    encoding: "utf8",
    maxBuffer: 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new Error(`K12 fixture SQL failed: ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  }
}

function seedChangeSet(input: {
  identity: Identity;
  vault: Vault;
  jobId: string;
  changeSetId: string;
  title: string;
  operation: Record<string, unknown>;
  expectedGitRevision: string;
}) {
  runSql(
    `BEGIN;
INSERT INTO knowledge_jobs
  (id, tenant_id, space_id, output_vault_id, role, kind, status,
   service_actor_user_id, requested_by_user_id, input, input_hash, idempotency_key,
   runtime_backend, runtime_effort, runtime_model_source, prompt_contract_revision,
   tool_policy_revision, max_tokens, max_sources, max_wall_seconds, progress_percent,
   attempt, max_attempts, finished_at)
VALUES
  (:'job_id'::uuid, :'tenant_id'::uuid, :'space_id'::uuid, :'vault_id'::uuid,
   'gardener', 'on_demand', 'succeeded', :'user_id'::uuid, :'user_id'::uuid,
   '{}'::jsonb, repeat('0', 64), :'idempotency_key', 'codex_exec', 'low', 'default',
   'k12-browser', 'k12-browser', 256, 1, 5, 100, 1, 1, NOW());
INSERT INTO knowledge_change_sets
  (id, tenant_id, job_id, space_id, output_vault_id, status, title, summary,
   operations, citations, created_by_user_id, materialization_key, expected_git_revision)
VALUES
  (:'change_set_id'::uuid, :'tenant_id'::uuid, :'job_id'::uuid, :'space_id'::uuid,
   :'vault_id'::uuid, 'pending_user_review', :'title',
   'Review the visible before and after text, then clarify it before accepting.',
   :'operations'::jsonb, '[]'::jsonb, :'user_id'::uuid, :'materialization_key',
   :'expected_git_revision');
COMMIT;
`,
    {
      job_id: input.jobId,
      change_set_id: input.changeSetId,
      tenant_id: input.identity.tenant_id,
      user_id: input.identity.user_id,
      space_id: input.vault.space_id,
      vault_id: input.vault.id,
      idempotency_key: `k12-browser-${input.jobId}`,
      materialization_key: `k12-browser-${input.changeSetId}`,
      title: input.title,
      operations: JSON.stringify([input.operation]),
      expected_git_revision: input.expectedGitRevision,
    },
  );
}

function cleanupRows(changeSetId: string, jobId: string) {
  runSql(
    `BEGIN;
DELETE FROM knowledge_change_sets WHERE id = :'change_set_id'::uuid;
DELETE FROM knowledge_jobs WHERE id = :'job_id'::uuid;
COMMIT;
`,
    { change_set_id: changeSetId, job_id: jobId },
  );
}

async function findWritableVault(request: APIRequestContext): Promise<Vault> {
  const spacesResponse = await request.get("/api/v1/web/workbench/knowledge/spaces");
  expect(spacesResponse.ok()).toBe(true);
  const spaces = await spacesResponse.json() as {
    spaces: Array<{ id: string; status: string; effective_role: string }>;
  };
  for (const space of spaces.spaces) {
    if (space.status !== "active" || !["contributor", "curator", "manager"].includes(space.effective_role)) continue;
    const response = await request.get(`/api/v1/web/workbench/knowledge/spaces/${space.id}/vaults`);
    if (!response.ok()) continue;
    const body = await response.json() as { vaults: Vault[] };
    const vault = body.vaults.find((item) => item.owner_state === "enabled");
    if (vault) return vault;
  }
  throw new Error("K12 requires one actor-writable enabled Knowledge Vault");
}

async function listChangeSets(request: APIRequestContext, spaceId: string) {
  const response = await request.get(`/api/v1/web/workbench/knowledge/spaces/${spaceId}/change-sets`);
  expect(response.ok()).toBe(true);
  return (await response.json() as { change_sets: ChangeSet[] }).change_sets;
}

test("reviews a visible diff, edits it, and confirms the applied Git revision", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K12_LIVE=1 for the 18085 Knowledge review fixture");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_K12_PASSWORD is required").not.toBe("");

  await login(page);
  const identityResponse = await page.request.get("/api/v1/auth/whoami");
  expect(identityResponse.ok()).toBe(true);
  const identity = await identityResponse.json() as Identity;
  const vault = await findWritableVault(page.request);
  const suffix = randomUUID().slice(0, 8);
  const title = `K12 review ${suffix}`;
  const jobId = randomUUID();
  const changeSetId = randomUUID();
  let note: Note | undefined;

  try {
    const created = await page.request.post(
      `/api/v1/web/workbench/knowledge/vaults/${vault.id}/notes`,
      { data: { title, body: `# ${title}\n\nCheck the cooling loop once.\n` } },
    );
    expect(created.ok(), await created.text()).toBe(true);
    note = await created.json() as Note;
    seedChangeSet({
      identity,
      vault,
      jobId,
      changeSetId,
      title,
      expectedGitRevision: note.git_revision,
      operation: {
        op: "update_note",
        object_id: note.object_id,
        expected_revision: note.revision,
        title,
        body: `# ${title}\n\nCheck the cooling loop before declaring recovery.\n`,
      },
    });

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "candidates",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
    }).toString());
    await page.getByRole("button", { name: new RegExp(title) }).click();
    const diff = page.getByRole("region", { name: "Note body diff" });
    await expect(diff).toContainText("Before");
    await expect(diff).toContainText("After");
    await expect(diff).toContainText("Check the cooling loop once.");
    await expect(diff).toContainText("Check the cooling loop before declaring recovery.");
    await page.screenshot({ path: "../../../.gadgetron/k12-review-diff.png", fullPage: true });

    await page.getByRole("button", { name: "Edit & accept" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Note body").fill(`# ${title}\n\nVerify the cooling loop twice before declaring recovery.\n`);
    await dialog.getByRole("button", { name: "Apply edited change" }).click();
    await expect(page.getByText("Applied").first()).toBeVisible();
    await expect.poll(async () => (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === changeSetId)?.status).toBe("applied");

    const applied = (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === changeSetId);
    expect(applied?.applied_git_revision).toBeTruthy();
    const revisionDetails = page.locator("details").filter({ hasText: "Applied Git revision" });
    await revisionDetails.locator("summary").click();
    await expect(revisionDetails).toContainText(applied!.applied_git_revision!);
    const current = await page.request.get(`/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`);
    expect(current.ok()).toBe(true);
    note = await current.json() as Note;
    expect(note.body).toContain("Verify the cooling loop twice");
    await page.screenshot({ path: "../../../.gadgetron/k12-review-materialize.png", fullPage: true });
  } finally {
    cleanupRows(changeSetId, jobId);
    if (note) {
      const current = await page.request.get(`/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`);
      if (current.ok()) {
        const value = await current.json() as Note;
        await page.request.delete(`/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`, {
          data: { expected_revision: value.revision },
        });
      }
    }
  }
});
