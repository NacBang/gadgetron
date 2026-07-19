import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_INCIDENT_LOG_EVIDENCE_LIVE === "1";
const email = process.env.GADGETRON_INCIDENT_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_INCIDENT_PASSWORD ?? "";

type EvidencePreview = {
  reference: string;
  summary: string;
  excerpt: string;
  source: string;
  category: string;
  occurrences: number;
  last_observed_at: string;
  classifier: string;
  cause: string;
  solution: string;
  finding_id?: string;
};

type IncidentEnrichment = {
  status: string;
  subject_revision: string;
  data: {
    summary?: string;
    citations?: Array<{ evidence_ref: string; reason: string }>;
  };
};

type IncidentRow = {
  incident_id: string;
  revision: string;
  rule_key: string;
  evidence_total: number;
  evidence_preview: EvidencePreview[];
  enrichments?: Record<string, IncidentEnrichment>;
};

async function checkedJson<T>(response: Awaited<ReturnType<APIRequestContext["get"]>>): Promise<T> {
  if (!response.ok()) throw new Error(`HTTP ${response.status()}: ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("shows exact rule evidence with a cited additive incident enrichment", async ({ page }) => {
  test.skip(!live, "set GADGETRON_INCIDENT_LOG_EVIDENCE_LIVE=1 against a runtime with a log incident");
  expect(password, "GADGETRON_INCIDENT_PASSWORD is required").not.toBe("");

  await login(page);
  const response = await checkedJson<{ payload: { rows: IncidentRow[] } }>(
    await page.request.get("/api/v1/web/workbench/views/server-administrator.alerts/data"),
  );
  const incident = response.payload.rows.find((row) =>
    row.rule_key === "log_finding"
      && row.evidence_preview?.length > 0
      && Object.values(row.enrichments ?? {}).some((item) => item.status === "Ready"),
  );
  expect(incident, "an existing log incident must have exact evidence and a ready enrichment").toBeDefined();
  const evidence = incident!.evidence_preview[0];
  const enrichment = Object.values(incident!.enrichments ?? {}).find((item) => item.status === "Ready")!;
  const citations = enrichment.data.citations ?? [];
  const evidenceReferences = new Set(incident!.evidence_preview.map((item) => item.reference));
  expect(incident!.revision).toMatch(/^[0-9a-f]{64}$/);
  expect(enrichment.subject_revision).toBe(incident!.revision);
  expect(enrichment.data.summary?.length).toBeGreaterThan(0);
  expect(citations.length).toBeGreaterThan(0);
  expect(citations.every((citation) => evidenceReferences.has(citation.evidence_ref))).toBe(true);
  expect(incident!.evidence_total).toBeGreaterThanOrEqual(1);
  expect(evidence).toEqual(expect.objectContaining({
    summary: expect.any(String),
    excerpt: expect.any(String),
    source: expect.any(String),
    category: expect.any(String),
    occurrences: expect.any(Number),
    last_observed_at: expect.any(String),
    classifier: expect.any(String),
    cause: expect.any(String),
    solution: expect.any(String),
  }));
  expect(evidence.excerpt.length).toBeGreaterThan(0);
  expect(evidence.cause.length).toBeGreaterThan(0);
  expect(evidence.solution.length).toBeGreaterThan(0);
  expect(evidence).not.toHaveProperty("finding_id");

  await page.goto("/web/workspace?id=server-administrator.alerts");
  await expect(page.getByRole("heading", { name: "Incidents", exact: true })).toBeVisible();
  const preview = page.getByTestId("card-evidence-preview").filter({ hasText: evidence.excerpt }).first();
  await expect(preview).toBeVisible();
  await expect(page.getByText(`Classified by ${evidence.classifier[0].toUpperCase()}${evidence.classifier.slice(1)}`).first()).toBeVisible();
  await expect(preview.getByText(evidence.cause, { exact: true }).first()).toBeVisible();
  await expect(preview.getByText(evidence.solution, { exact: true }).first()).toBeVisible();
  const aiSection = page.getByLabel("AI 보강").filter({ hasText: enrichment.data.summary! }).first();
  await expect(aiSection).toBeVisible();
  await expect(aiSection.getByText(enrichment.data.summary!, { exact: true })).toBeVisible();
  await expect(aiSection.locator(`a[href="#incident-${citations[0].evidence_ref}"]`)).toBeVisible();
  await preview.screenshot({ path: "../../../.gadgetron/r4-4-incident-log-evidence.png" });
});
