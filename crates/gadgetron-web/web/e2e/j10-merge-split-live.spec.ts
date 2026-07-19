import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R4_5_J10_LIVE === "1";
const email = process.env.GADGETRON_R4_5_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R4_5_PASSWORD ?? "";
const expectedVersion = process.env.GADGETRON_R4_5_VERSION ?? "0.8.5";
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
}

interface ChangeSet {
  id: string;
  status: string;
  materialized_object_id?: string | null;
  materialization_receipt?: {
    objects?: Array<{ id: string; path: string }>;
  } | null;
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
    throw new Error(`J10 fixture SQL failed: ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  }
  return result.stdout.trim();
}

function seedChangeSet(input: {
  identity: Identity;
  vault: Vault;
  jobId: string;
  changeSetId: string;
  title: string;
  summary: string;
  operations: Array<Record<string, unknown>>;
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
   'r4.5-j10', 'r4.5-j10', 256, 1, 5, 100, 1, 1, NOW());
INSERT INTO knowledge_change_sets
  (id, tenant_id, job_id, space_id, output_vault_id, status, title, summary,
   operations, citations, created_by_user_id, materialization_key)
VALUES
  (:'change_set_id'::uuid, :'tenant_id'::uuid, :'job_id'::uuid, :'space_id'::uuid,
   :'vault_id'::uuid, 'pending_user_review', :'title', :'summary', :'operations'::jsonb,
   '[]'::jsonb, :'user_id'::uuid, :'materialization_key');
COMMIT;
`,
    {
      job_id: input.jobId,
      change_set_id: input.changeSetId,
      tenant_id: input.identity.tenant_id,
      user_id: input.identity.user_id,
      space_id: input.vault.space_id,
      vault_id: input.vault.id,
      idempotency_key: `r4.5-j10-${input.jobId}`,
      materialization_key: `r4.5-j10-${input.changeSetId}`,
      title: input.title,
      summary: input.summary,
      operations: JSON.stringify(input.operations),
    },
  );
}

function cleanupRows(changeSetIds: string[], jobIds: string[]) {
  const uuidList = (values: string[]) => values
    .filter((value) => /^[0-9a-f-]{36}$/.test(value))
    .map((value) => `'${value}'::uuid`)
    .join(",");
  const changes = uuidList(changeSetIds);
  const jobs = uuidList(jobIds);
  if (!changes && !jobs) return;
  runSql(`BEGIN;
${changes ? `DELETE FROM knowledge_change_sets WHERE id IN (${changes});` : ""}
${jobs ? `DELETE FROM knowledge_jobs WHERE id IN (${jobs});` : ""}
COMMIT;
`);
}

async function findWritableVault(request: APIRequestContext): Promise<Vault> {
  const spacesResponse = await request.get("/api/v1/web/workbench/knowledge/spaces");
  expect(spacesResponse.ok()).toBe(true);
  const spaces = await spacesResponse.json() as {
    spaces: Array<{ id: string; status: string; effective_role: string }>;
  };
  for (const space of spaces.spaces) {
    if (space.status !== "active" || !["contributor", "curator", "manager"].includes(space.effective_role)) {
      continue;
    }
    const response = await request.get(
      `/api/v1/web/workbench/knowledge/spaces/${space.id}/vaults`,
    );
    if (!response.ok()) continue;
    const body = await response.json() as { vaults: Vault[] };
    const vault = body.vaults.find((item) => item.owner_state === "enabled");
    if (vault) return vault;
  }
  throw new Error("J10 requires one actor-writable enabled Knowledge Vault");
}

async function createNote(
  request: APIRequestContext,
  vaultId: string,
  title: string,
  body: string,
): Promise<Note> {
  const response = await request.post(
    `/api/v1/web/workbench/knowledge/vaults/${vaultId}/notes`,
    { data: { title, body } },
  );
  if (!response.ok()) {
    throw new Error(`create J10 note returned ${response.status()}: ${(await response.text()).slice(0, 500)}`);
  }
  return await response.json() as Note;
}

async function listChangeSets(request: APIRequestContext, spaceId: string) {
  const response = await request.get(
    `/api/v1/web/workbench/knowledge/spaces/${spaceId}/change-sets`,
  );
  expect(response.ok()).toBe(true);
  return (await response.json() as { change_sets: ChangeSet[] }).change_sets;
}

async function deleteNote(request: APIRequestContext, objectId: string) {
  const current = await request.get(
    `/api/v1/web/workbench/knowledge/objects/${objectId}/note`,
  );
  if (current.status() === 404) return;
  if (!current.ok()) throw new Error(`read J10 cleanup note ${objectId}: HTTP ${current.status()}`);
  const note = await current.json() as Note;
  const removed = await request.delete(
    `/api/v1/web/workbench/knowledge/objects/${objectId}/note`,
    { data: { expected_revision: note.revision } },
  );
  if (!removed.ok() && removed.status() !== 404) {
    throw new Error(`delete J10 cleanup note ${objectId}: HTTP ${removed.status()}`);
  }
}

test("reviews merge and split, then explores the resulting note and Graph", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_5_J10_LIVE=1 after the R4.3 soak");
  test.setTimeout(120_000);
  expect(password, "GADGETRON_R4_5_PASSWORD is required").not.toBe("");

  await login(page);
  await expect(page.getByTestId("version-badge")).toContainText(expectedVersion);
  const identityResponse = await page.request.get("/api/v1/auth/whoami");
  expect(identityResponse.ok()).toBe(true);
  const identity = await identityResponse.json() as Identity;
  const vault = await findWritableVault(page.request);
  const suffix = randomUUID().slice(0, 8);
  const jobIds: string[] = [];
  const changeSetIds: string[] = [];
  const noteIds: string[] = [];

  const actorMatches = runSql(
    "SELECT COUNT(*) FROM users WHERE tenant_id = :'tenant_id'::uuid AND id = :'user_id'::uuid;\n",
    { tenant_id: identity.tenant_id, user_id: identity.user_id },
  );
  expect(actorMatches).toBe("1");

  try {
    const queue = await createNote(
      page.request,
      vault.id,
      `J10 queue ordering ${suffix}`,
      "Queues preserve work ordering.",
    );
    noteIds.push(queue.object_id);
    const lease = await createNote(
      page.request,
      vault.id,
      `J10 worker leases ${suffix}`,
      "Leases prevent duplicate workers.",
    );
    noteIds.push(lease.object_id);

    const mergeJobId = randomUUID();
    const mergeId = randomUUID();
    const mergeTitle = `Unify worker coordination ${suffix}`;
    jobIds.push(mergeJobId);
    changeSetIds.push(mergeId);
    seedChangeSet({
      identity,
      vault,
      jobId: mergeJobId,
      changeSetId: mergeId,
      title: mergeTitle,
      summary: "Queues and leases describe one operating mechanism.",
      operations: [{
        op: "merge_notes",
        sources: [
          { object_id: queue.object_id, expected_revision: queue.revision },
          { object_id: lease.object_id, expected_revision: lease.revision },
        ],
        title: `J10 durable worker coordination ${suffix}`,
        body: "Queues preserve ordering while leases prevent duplicate workers.",
      }],
    });

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "candidates",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
    }).toString());
    await page.getByRole("button", { name: new RegExp(mergeTitle) }).click();
    await expect(page.getByText("2 source notes → 1 new note")).toBeVisible();
    await expect(page.getByText("Revisions are pinned. Original notes remain available.")).toBeVisible();
    await page.getByRole("button", { name: "Accept", exact: true }).click();
    await expect(page.getByRole("button", { name: "Accept", exact: true })).toBeHidden();
    await expect.poll(async () => (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === mergeId)?.status).toBe("applied");

    const mergedChange = (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === mergeId);
    expect(mergedChange?.status).toBe("applied");
    const mergedId = mergedChange?.materialized_object_id;
    expect(mergedId).toBeTruthy();
    noteIds.push(mergedId!);
    const mergedNoteResponse = await page.request.get(
      `/api/v1/web/workbench/knowledge/objects/${mergedId}/note`,
    );
    expect(mergedNoteResponse.ok()).toBe(true);
    const mergedNote = await mergedNoteResponse.json() as Note;

    const splitJobId = randomUUID();
    const splitId = randomUUID();
    const splitTitle = `Separate ordering from lease recovery ${suffix}`;
    jobIds.push(splitJobId);
    changeSetIds.push(splitId);
    seedChangeSet({
      identity,
      vault,
      jobId: splitJobId,
      changeSetId: splitId,
      title: splitTitle,
      summary: "Each procedure needs its own reusable note.",
      operations: [{
        op: "split_note",
        source_object_id: mergedId,
        expected_revision: mergedNote.revision,
        outputs: [
          { title: `J10 queue result ${suffix}`, body: "Queues preserve work ordering." },
          { title: `J10 lease result ${suffix}`, body: "Leases prevent duplicate workers." },
        ],
      }],
    });

    await page.getByRole("button", { name: "Refresh candidates" }).click();
    await page.getByRole("button", { name: new RegExp(splitTitle) }).click();
    await expect(page.getByText("1 source note → 2 new notes")).toBeVisible();
    await page.getByRole("button", { name: "Accept", exact: true }).click();
    await expect(page.getByRole("button", { name: "Accept", exact: true })).toBeHidden();
    await expect.poll(async () => (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === splitId)?.status).toBe("applied");

    const splitChange = (await listChangeSets(page.request, vault.space_id))
      .find((row) => row.id === splitId);
    expect(splitChange?.status).toBe("applied");
    const splitObjects = splitChange?.materialization_receipt?.objects ?? [];
    noteIds.push(...splitObjects.map((item) => item.id));
    expect(splitObjects).toHaveLength(2);
    const firstResult = splitObjects[0];
    const graphResponse = await page.request.post(
      "/api/v1/web/workbench/knowledge/graph/neighborhood",
      { data: {
        center_node_id: `note:${firstResult.id}`,
        depth: 1,
        node_limit: 20,
        edge_limit: 20,
        direction: "both",
        relation_kinds: ["derived_from"],
        space_ids: [vault.space_id],
      } },
    );
    if (!graphResponse.ok()) {
      throw new Error(
        `split result graph lookup returned ${graphResponse.status()}: ${(await graphResponse.text()).slice(0, 500)}`,
      );
    }
    const graph = await graphResponse.json() as {
      edges: Array<{ from_node_id: string; to_node_id: string; relation_kind: string }>;
    };
    expect(graph.edges).toContainEqual(expect.objectContaining({
      from_node_id: `note:${firstResult.id}`,
      to_node_id: `note:${mergedId}`,
      relation_kind: "derived_from",
    }));

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "notes",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
      selected: firstResult.id,
    }).toString());
    await expect(page.getByRole("heading", { name: `J10 queue result ${suffix}`, level: 2 })).toBeVisible();

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "graph",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
      center: `note:${firstResult.id}`,
    }).toString());
    const graphNode = page.getByTestId(`graph-node-note:${firstResult.id}`);
    await expect(graphNode).toBeVisible();
    await graphNode.click();
    await expect(page.getByRole("complementary", { name: "Graph inspector" }))
      .toContainText(`J10 queue result ${suffix}`);

  } finally {
    cleanupRows(changeSetIds, jobIds);
    for (const objectId of [...noteIds].reverse()) {
      await deleteNote(page.request, objectId);
    }
  }
});
