import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page, type Route } from "@playwright/test";

const live = process.env.GADGETRON_KNOW_T2_LIVE === "1";
const email = process.env.GADGETRON_KNOW_T2_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_KNOW_T2_PASSWORD ?? "";
const databaseUrl = process.env.GADGETRON_DATABASE_URL ?? "";
const postgresContainer = process.env.GADGETRON_POSTGRES_CONTAINER ?? "gadgetron-pg";
const backendBase = (process.env.GADGETRON_KNOW_T2_BACKEND_URL ?? "http://127.0.0.1:18085").replace(/\/$/, "");

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

interface Source {
  id: string;
  title?: string;
  revision: number;
}

async function proxyBackend(page: Page) {
  const proxy = async (route: Route) => {
    const target = new URL(route.request().url());
    const backend = new URL(backendBase);
    target.protocol = backend.protocol;
    target.hostname = backend.hostname;
    target.port = backend.port;
    await route.fulfill({ response: await route.fetch({ url: target.toString() }) });
  };
  await page.route(/http:\/\/127\.0\.0\.1:3000\/(?:api|v1)\//, proxy);
  await page.route("http://127.0.0.1:3000/health", proxy);
}

async function login(page: Page) {
  await proxyBackend(page);
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
    throw new Error(`KNOW-T2 fixture SQL failed: ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  }
}

async function findWritableVault(request: APIRequestContext): Promise<Vault> {
  const spacesResponse = await request.get(`${backendBase}/api/v1/web/workbench/knowledge/spaces`);
  expect(spacesResponse.ok()).toBe(true);
  const spaces = await spacesResponse.json() as {
    spaces: Array<{ id: string; status: string; effective_role: string }>;
  };
  for (const space of spaces.spaces) {
    if (space.status !== "active" || !["contributor", "curator", "manager"].includes(space.effective_role)) continue;
    const response = await request.get(`${backendBase}/api/v1/web/workbench/knowledge/spaces/${space.id}/vaults`);
    if (!response.ok()) continue;
    const body = await response.json() as { vaults: Vault[] };
    const vault = body.vaults.find((item) => item.owner_state === "enabled");
    if (vault) return vault;
  }
  throw new Error("KNOW-T2 requires one actor-writable enabled Knowledge Vault");
}

async function cleanupStaleSources(request: APIRequestContext, spaceId: string) {
  const response = await request.get(
    `${backendBase}/api/v1/web/workbench/knowledge/spaces/${spaceId}/sources`,
  );
  if (!response.ok()) return;
  const body = await response.json() as { sources: Source[] };
  for (const source of body.sources.filter((item) => item.title?.startsWith("Recovery source "))) {
    await request.delete(`${backendBase}/api/v1/web/workbench/knowledge/sources/${source.id}`, {
      data: { expected_revision: source.revision },
      timeout: 5_000,
    }).catch(() => undefined);
  }
}

function seedChangeSet(input: {
  identity: Identity;
  vault: Vault;
  sourceId: string;
  jobId: string;
  changeSetId: string;
  title: string;
  claim: string;
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
   'know-t2-browser', 'know-t2-browser', 256, 1, 5, 100, 1, 1, NOW());
INSERT INTO knowledge_change_sets
  (id, tenant_id, job_id, space_id, output_vault_id, status, title, summary,
   operations, citations, created_by_user_id, materialization_key)
VALUES
  (:'change_set_id'::uuid, :'tenant_id'::uuid, :'job_id'::uuid, :'space_id'::uuid,
   :'vault_id'::uuid, 'pending_user_review', :'title',
   'Confirm the cited recovery instruction before recording it.',
   :'operations'::jsonb, :'citations'::jsonb, :'user_id'::uuid, :'materialization_key');
COMMIT;
`,
    {
      job_id: input.jobId,
      change_set_id: input.changeSetId,
      tenant_id: input.identity.tenant_id,
      user_id: input.identity.user_id,
      space_id: input.vault.space_id,
      vault_id: input.vault.id,
      idempotency_key: `know-t2-browser-${input.jobId}`,
      materialization_key: `know-t2-browser-${input.changeSetId}`,
      title: input.title,
      operations: JSON.stringify([{
        op: "create_note",
        title: input.title,
        body: `# ${input.title}\n\n${input.claim}\n`,
      }]),
      citations: JSON.stringify([{
        source_id: input.sourceId,
        locator: "paragraph 2",
        claim: input.claim,
      }]),
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

test("opens a Review citation and highlights the exact stored source passage", async ({ page }) => {
  test.skip(!live, "set GADGETRON_KNOW_T2_LIVE=1 for the KNOW-T2 browser fixture");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_KNOW_T2_PASSWORD is required").not.toBe("");

  await login(page);
  const identityResponse = await page.request.get(`${backendBase}/api/v1/auth/whoami`);
  expect(identityResponse.ok()).toBe(true);
  const identity = await identityResponse.json() as Identity;
  const vault = await findWritableVault(page.request);
  await cleanupStaleSources(page.request, vault.space_id);
  const suffix = randomUUID().slice(0, 8);
  const title = `KNOW-T2 citation ${suffix}`;
  const claim = `Verify the coolant loop twice before recovery ${suffix}.`;
  const jobId = randomUUID();
  const changeSetId = randomUUID();
  let source: Source | undefined;

  try {
    const upload = await page.request.post(
      `${backendBase}/api/v1/web/workbench/knowledge/vaults/${vault.id}/sources/upload`,
      {
        multipart: {
          title: `Recovery source ${suffix}`,
          file: {
            name: `recovery-${suffix}.txt`,
            mimeType: "text/plain",
            buffer: Buffer.from(`Recovery checklist\n\n${claim}\n\nRecord the verification outcome.\n`),
          },
        },
      },
    );
    expect(upload.ok(), await upload.text()).toBe(true);
    source = (await upload.json() as { source: Source }).source;
    seedChangeSet({ identity, vault, sourceId: source.id, jobId, changeSetId, title, claim });

    await page.goto("/web/review?tab=knowledge");
    const card = page.locator('[data-review-kind="knowledge"]').filter({ hasText: title });
    await expect(card).toBeVisible();
    await card.locator("summary").click();
    await card.getByRole("button", { name: /paragraph 2/i }).click({ timeout: 5_000 });

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible({ timeout: 5_000 });
    await expect(dialog.getByText("Exact passage found in the stored source")).toBeVisible({ timeout: 10_000 });
    await expect(dialog.getByTestId("citation-passage-highlight")).toHaveText(claim, { timeout: 10_000 });
    await page.screenshot({ path: "../../../.gadgetron/know-t2-citation-highlight.png", fullPage: true });
  } finally {
    cleanupRows(changeSetId, jobId);
    if (source) {
      try {
        const detail = await page.request.get(
          `${backendBase}/api/v1/web/workbench/knowledge/sources/${source.id}`,
          { timeout: 5_000 },
        );
        if (detail.ok()) {
          const current = (await detail.json() as { source: Source }).source;
          await page.request.delete(`${backendBase}/api/v1/web/workbench/knowledge/sources/${source.id}`, {
            data: { expected_revision: current.revision },
            timeout: 5_000,
          });
        }
      } catch {
        // The fixture remains clearly titled and can be removed by a later bounded cleanup.
      }
    }
  }
});
