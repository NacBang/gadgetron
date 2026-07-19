import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

import {
  expectAccessible,
  expectReadableTextControls,
} from "./support/ui-assertions";

const live = process.env.GADGETRON_K14_T4_LIVE === "1";
const email = process.env.GADGETRON_K14_T4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_K14_T4_PASSWORD ?? "";
const databaseUrl = process.env.GADGETRON_DATABASE_URL ?? "";
const postgresContainer = process.env.GADGETRON_POSTGRES_CONTAINER ?? "gadgetron-pg";

interface Identity {
  tenant_id: string;
  user_id: string;
}

interface Vault {
  id: string;
  space_id: string;
  owner_state: string;
}

interface Note {
  object_id: string;
  revision: number;
  git_revision: string;
  body: string;
}

interface PolicyDocument {
  schema_version: number;
  default_decision: "auto" | "review" | "deny";
  default_reason: string;
  rules: Array<Record<string, unknown> & { id: string }>;
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
    throw new Error(
      `K14-T4 fixture SQL failed: ${(result.stderr || result.stdout).trim().slice(0, 500)}`,
    );
  }
}

function seedChangeSet(input: {
  identity: Identity;
  vault: Vault;
  jobId: string;
  changeSetId: string;
  evidenceSourceId: string;
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
   'k14-t4-browser', 'k14-t4-browser', 256, 1, 5, 100, 1, 1, NOW());
INSERT INTO knowledge_change_sets
  (id, tenant_id, job_id, space_id, output_vault_id, status, title, summary,
   operations, citations, created_by_user_id, materialization_key, expected_git_revision)
VALUES
  (:'change_set_id'::uuid, :'tenant_id'::uuid, :'job_id'::uuid, :'space_id'::uuid,
   :'vault_id'::uuid, 'pending_user_review', :'title',
   'Require two checks before the recovery is recorded.', :'operations'::jsonb,
   :'citations'::jsonb, :'user_id'::uuid, :'materialization_key', :'expected_git_revision');
COMMIT;
`,
    {
      job_id: input.jobId,
      change_set_id: input.changeSetId,
      tenant_id: input.identity.tenant_id,
      user_id: input.identity.user_id,
      space_id: input.vault.space_id,
      vault_id: input.vault.id,
      idempotency_key: `k14-t4-browser-${input.jobId}`,
      materialization_key: `k14-t4-browser-${input.changeSetId}`,
      title: input.title,
      operations: JSON.stringify([input.operation]),
      citations: JSON.stringify([{
        source_id: input.evidenceSourceId,
        locator: "K14-T4 live review fixture",
        claim: "Two checks reduce false recovery reports.",
      }]),
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
  throw new Error("K14-T4 requires one actor-writable enabled Knowledge Vault");
}

test("processes one action and one Knowledge change from the unified Review trust gate", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K14_T4_LIVE=1 for the release-built 18085 service");
  test.setTimeout(120_000);
  expect(password, "GADGETRON_K14_T4_PASSWORD is required").not.toBe("");

  await login(page);
  const identityResponse = await page.request.get("/api/v1/auth/whoami");
  expect(identityResponse.ok()).toBe(true);
  const identity = await identityResponse.json() as Identity;
  const vault = await findWritableVault(page.request);
  const suffix = randomUUID().slice(0, 8);
  const title = `K14-T4 review ${suffix}`;
  const jobId = randomUUID();
  const changeSetId = randomUUID();
  const evidenceSourceId = randomUUID();
  const reviewPolicyRuleId = `k14-t4-live-${suffix}`;
  const actionId = "server-administrator.logs.action.loganalysis.finding-dismiss";
  let note: Note | undefined;
  let approvalId = "";
  let restoreDocument: PolicyDocument | null = null;
  let reviewPolicyRevision = 0;

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
      evidenceSourceId,
      title,
      expectedGitRevision: note.git_revision,
      operation: {
        op: "update_note",
        object_id: note.object_id,
        expected_revision: note.revision,
        title,
        body: `# ${title}\n\nCheck the cooling loop twice before recovery.\n`,
      },
    });

    const policyResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
    expect(policyResponse.ok(), await policyResponse.text()).toBe(true);
    const activePolicy = await policyResponse.json() as {
      policy: { revision: number; document: PolicyDocument };
    };
    restoreDocument = activePolicy.policy.document;
    const reviewDocument: PolicyDocument = {
      ...restoreDocument,
      rules: [{
        id: reviewPolicyRuleId,
        priority: 0,
        enabled: true,
        match: { action_ids: [actionId] },
        decision: "review",
        reason: "Verify the unified Action and Knowledge review journey",
      }, ...restoreDocument.rules],
    };
    const policyUpdate = await page.request.post(
      "/api/v1/web/workbench/admin/policy/revisions",
      { data: { expected_revision: activePolicy.policy.revision, document: reviewDocument } },
    );
    expect(policyUpdate.ok(), await policyUpdate.text()).toBe(true);
    reviewPolicyRevision = (await policyUpdate.json() as {
      policy: { revision: number };
    }).policy.revision;

    const logsResponse = await page.request.get(
      "/api/v1/web/workbench/views/server-administrator.logs/data",
    );
    expect(logsResponse.ok(), await logsResponse.text()).toBe(true);
    const logs = await logsResponse.json() as {
      payload: { rows: Array<{ finding_id?: string }> };
    };
    const findingId = logs.payload.rows.find((row) => row.finding_id)?.finding_id;
    expect(findingId, "K14-T4 requires one actual Server finding for Review").toBeTruthy();

    const actionRequest = await page.request.post(
      `/api/v1/web/workbench/actions/${actionId}`,
      { data: { args: { finding_id: findingId } } },
    );
    expect(actionRequest.ok(), await actionRequest.text()).toBe(true);
    const actionResult = await actionRequest.json() as {
      result: { status: string; approval_id?: string };
    };
    expect(actionResult.result.status).toBe("pending_approval");
    approvalId = actionResult.result.approval_id ?? "";
    expect(approvalId).not.toBe("");

    await page.goto("/web/review?tab=knowledge");
    const workspace = page.getByTestId("knowledge-review-workspace");
    await expect(workspace).toBeVisible();
    const actionCard = workspace.locator('[data-review-kind="action"]').filter({
      hasText: "loganalysis.finding-dismiss",
    });
    const knowledgeCard = workspace.locator('[data-review-kind="knowledge"]').filter({
      hasText: title,
    });
    await expect(actionCard).toContainText("Will run");
    await expect(knowledgeCard).toContainText("Will be recorded");
    await expect(page.getByTestId("review-trust-summary")).toContainText("Evidence-backed");
    await expectAccessible(page, '[data-testid="knowledge-review-workspace"]');
    await expectReadableTextControls(page, '[data-testid="knowledge-review-workspace"]');
    await workspace.screenshot({
      path: "../../../.gadgetron/k14-t4-review-convergence.png",
    });

    await actionCard.getByRole("button", { name: "Open decision" }).click();
    const detail = page.getByTestId("approval-detail");
    await expect(detail).toContainText("loganalysis.finding-dismiss");
    await detail.getByRole("button", { name: /Cancel my request|Reject request/ }).click();
    const actionDialog = page.getByRole("dialog");
    await actionDialog.getByPlaceholder("What should change before this can proceed?").fill(
      "The integrated Review journey verified the requested target without executing it.",
    );
    await actionDialog.getByRole("button", { name: /Cancel my request|Reject request/ }).click();
    await expect(page.getByTestId(`approval-row-${approvalId}`)).toHaveCount(0);

    await page.getByRole("tab", { name: /Knowledge changes/ }).click();
    const refreshedKnowledgeCard = workspace.locator('[data-review-kind="knowledge"]').filter({
      hasText: title,
    });
    await refreshedKnowledgeCard.getByRole("checkbox", { name: `Select ${title}` }).check();
    await page.keyboard.press("Control+Enter");
    const batchDialog = page.getByRole("dialog");
    await expect(batchDialog).toContainText("Accept 1 Knowledge change?");
    await batchDialog.getByRole("button", { name: "Accept changes" }).click();
    await expect(refreshedKnowledgeCard).toHaveCount(0);

    const current = await page.request.get(
      `/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`,
    );
    expect(current.ok()).toBe(true);
    note = await current.json() as Note;
    expect(note.body).toContain("Check the cooling loop twice before recovery.");
  } finally {
    if (approvalId) {
      await page.request.post(`/api/v1/web/workbench/approvals/${approvalId}/deny`, {
        data: { reason: "K14-T4 live fixture cleanup" },
      });
    }
    cleanupRows(changeSetId, jobId);
    if (note) {
      const current = await page.request.get(
        `/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`,
      );
      if (current.ok()) {
        const value = await current.json() as Note;
        await page.request.delete(
          `/api/v1/web/workbench/knowledge/objects/${note.object_id}/note`,
          { data: { expected_revision: value.revision } },
        );
      }
    }
    if (restoreDocument && reviewPolicyRevision > 0) {
      const currentResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
      if (!currentResponse.ok()) {
        throw new Error(`Policy cleanup read returned ${currentResponse.status()}`);
      }
      const current = await currentResponse.json() as {
        policy: { revision: number; document: PolicyDocument };
      };
      if (!current.policy.document.rules.some((rule) => rule.id === reviewPolicyRuleId)) {
        throw new Error("Review policy changed before cleanup; refusing to overwrite it");
      }
      const restored = await page.request.post(
        "/api/v1/web/workbench/admin/policy/revisions",
        { data: { expected_revision: current.policy.revision, document: restoreDocument } },
      );
      if (!restored.ok()) {
        throw new Error(`Policy cleanup returned ${restored.status()}: ${await restored.text()}`);
      }
    }
  }
});
