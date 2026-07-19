import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_A22_LIVE === "1";
const email = process.env.GADGETRON_A22_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_A22_PASSWORD ?? "";
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

interface Source {
  id: string;
  revision: number;
}

interface KnowledgeObject {
  id: string;
}

interface Note {
  object_id: string;
  revision: number;
  content_hash: string;
  git_revision: string;
  properties: Record<string, unknown>;
  body: string;
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
    throw new Error(`A22 fixture SQL failed: ${(result.stderr || result.stdout).trim().slice(0, 500)}`);
  }
}

function seedLearningReview(input: {
  identity: Identity;
  vault: Vault;
  source: Source;
  note: Note;
  title: string;
  researcherJobId: string;
  gardenerJobId: string;
  candidateId: string;
  changeSetId: string;
  outcomeId: string;
}) {
  const citations = [{
    source_id: input.source.id,
    locator: "verification window and delayed telemetry",
    claim: "The reviewed runbook contains both the successful check and its counterexample.",
    stance: "supports",
  }];
  const importance = [
    "operational_impact",
    "evidence_quality",
    "novelty",
    "recurrence",
    "cross_bundle_reuse",
    "contradiction_value",
    "outcome_support",
  ].map((factor) => ({
    factor,
    score: 0.64,
    reason: "Verified operation evidence and a counterexample need human review.",
  }));
  const candidate = {
    schema_version: 1,
    target_kind: "lesson",
    claim: "Verify cooling recovery long enough to observe current telemetry",
    claims: [{
      id: "verified-window",
      statement: "The verified recovery remained healthy through the observation window.",
      source_ids: [input.source.id],
    }, {
      id: "delayed-telemetry",
      statement: "A fixed five-minute window can fail when telemetry arrives late.",
      source_ids: [input.source.id],
    }],
    supporting_claim_ids: ["verified-window"],
    contradicting_claim_ids: ["delayed-telemetry"],
    applicability: ["Cooling recovery with current health telemetry"],
    limitations: ["Extend the window when sensor telemetry is delayed or incomplete"],
    freshness: {
      status: "current",
      reason: "Pinned reviewed Lesson, counterexample, and verified Outcome",
    },
    confidence: 0.64,
    importance,
    verified_outcome_ids: [input.outcomeId],
  };
  const gardenerInput = {
    question: "Prepare the reviewed cooling Lesson revision",
    candidate_artifact_id: input.candidateId,
    lesson_revision_target: {
      object_id: input.note.object_id,
      expected_revision: input.note.revision,
      content_hash: input.note.content_hash,
      title: input.title,
      body: input.note.body,
      source_ids: [input.source.id],
      originating_subject: null,
    },
  };
  const operations = [{
    op: "update_note",
    object_id: input.note.object_id,
    expected_revision: input.note.revision,
    title: input.title,
    body: "Verify cooling recovery long enough to observe current telemetry. Extend the window when sensor telemetry is delayed or incomplete.",
  }];

  runSql(
    `BEGIN;
INSERT INTO knowledge_outcome_feedback
  (id, tenant_id, actor_user_id, consumer_bundle_id, feedback_id,
   experience_revision, subject_owner_bundle, subject_kind, subject_stable_id,
   subject_revision, operation_id, predicate_result, verification_summary,
   before_state, after_state, used_citations, feedback_json)
VALUES
  (:'outcome_id'::uuid, :'tenant_id'::uuid, :'user_id'::uuid,
   'server-administrator', :'feedback_id', 'sha256:' || repeat('a', 64),
   'server-administrator', 'server-administrator.server-incident',
   'cooling-edge-browser', '1', 'cooling-recovery', 'satisfied',
   'Cooling recovery remained healthy through the observation window',
   '{"health":"degraded"}'::jsonb, '{"health":"healthy"}'::jsonb,
   :'used_citations'::jsonb,
   jsonb_build_object('authority', jsonb_build_object('allowed_space_ids', jsonb_build_array(:'space_id'))));
INSERT INTO knowledge_jobs
  (id, tenant_id, space_id, output_vault_id, role, kind, status,
   service_actor_user_id, requested_by_user_id, input, input_hash, idempotency_key,
   runtime_backend, runtime_effort, runtime_model_source, prompt_contract_revision,
   tool_policy_revision, max_tokens, max_sources, max_wall_seconds, progress_percent,
   attempt, max_attempts, finished_at)
VALUES
  (:'researcher_job_id'::uuid, :'tenant_id'::uuid, :'space_id'::uuid, :'vault_id'::uuid,
   'researcher', 'on_demand', 'succeeded', :'user_id'::uuid, :'user_id'::uuid,
   '{}'::jsonb, repeat('1', 64), :'researcher_key', 'codex_exec', 'low', 'default',
   'a22-browser', 'a22-browser', 256, 1, 5, 100, 1, 1, NOW()),
  (:'gardener_job_id'::uuid, :'tenant_id'::uuid, :'space_id'::uuid, :'vault_id'::uuid,
   'gardener', 'on_demand', 'succeeded', :'user_id'::uuid, :'user_id'::uuid,
   :'gardener_input'::jsonb, repeat('2', 64), :'gardener_key', 'codex_exec', 'low', 'default',
   'a22-browser', 'a22-browser', 256, 1, 5, 100, 1, 1, NOW());
INSERT INTO knowledge_job_artifacts
  (id, tenant_id, job_id, space_id, kind, title, summary, payload, citations, content_hash)
VALUES
  (:'candidate_id'::uuid, :'tenant_id'::uuid, :'researcher_job_id'::uuid,
   :'space_id'::uuid, 'candidate', :'title',
   'A verified success supports the Lesson while delayed telemetry narrows its scope.',
   :'candidate'::jsonb, :'citations'::jsonb, repeat('3', 64));
INSERT INTO knowledge_change_sets
  (id, tenant_id, job_id, space_id, output_vault_id, candidate_artifact_id,
   status, title, summary, operations, citations, created_by_user_id,
   materialization_key, expected_git_revision)
VALUES
  (:'change_set_id'::uuid, :'tenant_id'::uuid, :'gardener_job_id'::uuid,
   :'space_id'::uuid, :'vault_id'::uuid, :'candidate_id'::uuid,
   'pending_user_review', :'title',
   'A verified success supports the Lesson while delayed telemetry narrows its scope.',
   :'operations'::jsonb, :'citations'::jsonb, :'user_id'::uuid,
   :'materialization_key', :'expected_git_revision');
COMMIT;
`,
    {
      outcome_id: input.outcomeId,
      tenant_id: input.identity.tenant_id,
      user_id: input.identity.user_id,
      space_id: input.vault.space_id,
      vault_id: input.vault.id,
      feedback_id: `a22-browser-${input.outcomeId}`,
      used_citations: JSON.stringify([{
        citation_id: `${input.note.object_id}:${input.note.revision}`,
        source_revision: String(input.source.revision),
      }]),
      researcher_job_id: input.researcherJobId,
      gardener_job_id: input.gardenerJobId,
      researcher_key: `a22-browser-researcher-${input.researcherJobId}`,
      gardener_key: `a22-browser-gardener-${input.gardenerJobId}`,
      gardener_input: JSON.stringify(gardenerInput),
      candidate_id: input.candidateId,
      change_set_id: input.changeSetId,
      title: input.title,
      candidate: JSON.stringify(candidate),
      citations: JSON.stringify(citations),
      operations: JSON.stringify(operations),
      materialization_key: `a22-browser-${input.changeSetId}`,
      expected_git_revision: input.note.git_revision,
    },
  );
}

function cleanupRows(input: {
  changeSetId: string;
  candidateId: string;
  researcherJobId: string;
  gardenerJobId: string;
  outcomeId: string;
}) {
  runSql(
    `BEGIN;
DELETE FROM knowledge_change_sets WHERE id = :'change_set_id'::uuid;
DELETE FROM knowledge_job_artifacts WHERE id = :'candidate_id'::uuid;
DELETE FROM knowledge_jobs WHERE id IN (:'researcher_job_id'::uuid, :'gardener_job_id'::uuid);
DELETE FROM knowledge_outcome_feedback WHERE id = :'outcome_id'::uuid;
COMMIT;
`,
    {
      change_set_id: input.changeSetId,
      candidate_id: input.candidateId,
      researcher_job_id: input.researcherJobId,
      gardener_job_id: input.gardenerJobId,
      outcome_id: input.outcomeId,
    },
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
  throw new Error("A22 requires one actor-writable enabled Knowledge Vault");
}

test("reviews contradictory outcome feedback before promoting the Lesson", async ({ page }) => {
  test.skip(!live, "set GADGETRON_A22_LIVE=1 for the 18085 learning feedback fixture");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_A22_PASSWORD is required").not.toBe("");

  await login(page);
  const identityResponse = await page.request.get("/api/v1/auth/whoami");
  expect(identityResponse.ok()).toBe(true);
  const identity = await identityResponse.json() as Identity;
  const vault = await findWritableVault(page.request);
  const suffix = randomUUID().slice(0, 8);
  const title = `Cooling recovery learning ${suffix}`;
  const researcherJobId = randomUUID();
  const gardenerJobId = randomUUID();
  const candidateId = randomUUID();
  const changeSetId = randomUUID();
  const outcomeId = randomUUID();
  let source: Source | undefined;
  let object: KnowledgeObject | undefined;

  try {
    const uploaded = await page.request.post(
      `/api/v1/web/workbench/knowledge/vaults/${vault.id}/sources/upload`,
      {
        multipart: {
          title,
          file: {
            name: "cooling-recovery.md",
            mimeType: "text/markdown",
            buffer: Buffer.from(
              `# ${title}\n\nVerify health for five minutes before closing the incident. A fixed window can be insufficient when telemetry is delayed.\n`,
            ),
          },
        },
      },
    );
    expect(uploaded.ok(), await uploaded.text()).toBe(true);
    const upload = await uploaded.json() as { source: Source; object: KnowledgeObject };
    source = upload.source;
    object = upload.object;
    const fetched = await page.request.get(
      `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
    );
    expect(fetched.ok()).toBe(true);
    let note = await fetched.json() as Note;
    const updated = await page.request.put(
      `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
      {
        data: {
          expected_revision: note.revision,
          expected_git_revision: note.git_revision,
          properties: {
            ...note.properties,
            knowledge_kind: "lesson",
            review_state: "reviewed",
          },
          body: note.body,
        },
      },
    );
    expect(updated.ok(), await updated.text()).toBe(true);
    const current = await page.request.get(
      `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
    );
    expect(current.ok()).toBe(true);
    note = await current.json() as Note;

    seedLearningReview({
      identity,
      vault,
      source,
      note,
      title,
      researcherJobId,
      gardenerJobId,
      candidateId,
      changeSetId,
      outcomeId,
    });

    await page.goto("/web/knowledge?" + new URLSearchParams({
      workspace: "candidates",
      space: vault.space_id,
      bundle: vault.home_bundle_id,
    }).toString());
    await page.getByRole("button", { name: new RegExp(title) }).click();
    await expect(page.getByText("Review needed").first()).toBeVisible();
    await expect(page.getByText("Not canonical yet")).toBeVisible();
    await expect(page.getByText("Moderate confidence")).toBeVisible();
    await expect(page.getByText("Applies when")).toBeVisible();
    await expect(page.getByText("Cooling recovery with current health telemetry")).toBeVisible();
    await expect(page.getByText("Limits and counterexamples")).toBeVisible();
    await expect(page.getByText("Extend the window when sensor telemetry is delayed or incomplete").first()).toBeVisible();
    await expect(page.getByText("A fixed five-minute window can fail when telemetry arrives late.")).toBeVisible();
    await expect(page.getByText("Importance orders review work. It does not prove the claim or approve it automatically.")).toBeVisible();
    await page.screenshot({ path: "../../../.gadgetron/a22-learning-feedback-review.png", fullPage: true });

    await page.getByRole("button", { name: "Accept", exact: true }).click();
    await expect(page.getByText("Applied").first()).toBeVisible();
    await expect(page.getByText("In the Knowledge Vault")).toBeVisible();
    const promoted = await page.request.get(
      `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
    );
    expect(promoted.ok()).toBe(true);
    const promotedNote = await promoted.json() as Note;
    expect(promotedNote.properties.review_state).toBe("verified");
    expect(promotedNote.body).toContain("sensor telemetry is delayed or incomplete");
    await page.screenshot({ path: "../../../.gadgetron/a22-learning-feedback-promoted.png", fullPage: true });
  } finally {
    cleanupRows({ changeSetId, candidateId, researcherJobId, gardenerJobId, outcomeId });
    if (source) {
      await page.request.delete(
        `/api/v1/web/workbench/knowledge/sources/${source.id}`,
        { data: { expected_revision: source.revision } },
      );
    } else if (object) {
      const current = await page.request.get(
        `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
      );
      if (current.ok()) {
        const note = await current.json() as Note;
        await page.request.delete(
          `/api/v1/web/workbench/knowledge/objects/${object.id}/note`,
          { data: { expected_revision: note.revision } },
        );
      }
    }
  }
});
