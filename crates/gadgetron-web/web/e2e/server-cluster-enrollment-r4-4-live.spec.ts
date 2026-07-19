import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

import { expect, test, type Page } from "@playwright/test";

import { unwrapPayload, type ActionResponse } from "../app/lib/workbench-client";

const live = process.env.GADGETRON_R4_4_CLUSTER_LIVE === "1";
const recoveryLive = process.env.GADGETRON_R4_4_RECOVERY_LIVE === "1";
const rolloutLive = process.env.GADGETRON_R4_4_ROLLOUT_LIVE === "1";
const setupRolloutLive = process.env.GADGETRON_R4_4_SETUP_ROLLOUT_LIVE === "1";
const secondTargetLive = process.env.GADGETRON_R4_4_SECOND_TARGET_LIVE === "1";
const email = process.env.GADGETRON_R4_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R4_4_PASSWORD ?? "";
const sudoPassword = process.env.GADGETRON_R4_4_SUDO_PASSWORD ?? "";
const targetLabel = process.env.GADGETRON_R4_4_TARGET_LABEL ?? "Jungho workstation";
const secondTargetAddress = process.env.GADGETRON_R4_4_SECOND_TARGET_ADDRESS ?? "";
const secondTargetPort = Number.parseInt(process.env.GADGETRON_R4_4_SECOND_TARGET_PORT ?? "22", 10);
const secondTargetUser = process.env.GADGETRON_R4_4_SECOND_TARGET_USER ?? "";
const secondTargetPassword = process.env.GADGETRON_R4_4_SECOND_TARGET_PASSWORD ?? "";
const secondTargetSudoPassword = process.env.GADGETRON_R4_4_SECOND_TARGET_SUDO_PASSWORD ?? "";
const secondTargetLabel = process.env.GADGETRON_R4_4_SECOND_TARGET_LABEL ?? "";
const monitoringMarker = process.env.GADGETRON_R4_4_MONITORING_MARKER ?? "";
const expectedCompliance = process.env.GADGETRON_R4_4_EXPECTED_COMPLIANCE ?? "Compliant";
const transitionAction = "server-administrator.fleet.action.server.enrollment-transition";
const rolloutAction = "server-administrator.fleet.action.server.enrollment-rollout-apply";
const monitoringRepairAction = "server-administrator.servers.action.server.monitoring-repair";
const reviewPolicyRuleId = "r4-4-enrollment-recovery-review";
const rolloutPolicyRuleId = "r4-4-profile-rollout-review";
const profileListAction = "server-administrator.fleet.action.server.profiles-list";
const profileCreateAction = "server-administrator.fleet.action.server.profile-revision-create";
const clusterListAction = "server-administrator.fleet.action.server.clusters-list";
const clusterUpsertAction = "server-administrator.fleet.action.server.cluster-upsert";

type PolicyDocument = {
  schema_version: number;
  default_decision: "auto" | "review" | "deny";
  default_reason: string;
  rules: Array<Record<string, unknown> & { id: string }>;
};

const clusters = [
  {
    id: "development-operations-a",
    label: "Development operations A",
    environment: "development",
    purpose: "Live development server operations and qualification",
    roleId: "operations",
    roleLabel: "Operations node",
  },
  {
    id: "gpu-research-b",
    label: "GPU research B",
    environment: "research",
    purpose: "GPU research capacity and qualification",
    roleId: "compute",
    roleLabel: "Compute node",
  },
] as const;

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

async function ensureCluster(page: Page, cluster: typeof clusters[number]) {
  const existing = page.getByRole("heading", { name: cluster.label, exact: true }).first();
  if (await existing.isVisible().catch(() => false)) return;
  if (await existing.waitFor({ state: "visible", timeout: 10_000 }).then(() => true).catch(() => false)) return;

  await page.getByRole("button", { name: "Cluster", exact: true }).click();
  await page.getByLabel("Cluster name").fill(cluster.label);
  await page.getByLabel("Environment").fill(cluster.environment);
  await page.getByLabel("Cluster ID").fill(cluster.id);
  await page.getByLabel("Purpose").fill(cluster.purpose);
  await page.getByLabel("Initial role").fill(cluster.roleLabel);
  await page.getByLabel("Role ID").fill(cluster.roleId);
  await page.getByRole("button", { name: "Create cluster", exact: true }).click();
  await expect(page.getByText(cluster.label, { exact: true }).first()).toBeVisible({ timeout: 30_000 });
}

async function invokeAction(page: Page, actionId: string, args: Record<string, unknown>) {
  const response = await page.request.post(
    `/api/v1/web/workbench/actions/${actionId}`,
    { data: { args } },
  );
  expect(response.ok(), await response.text()).toBe(true);
  const body = await response.json() as ActionResponse;
  expect(body.result?.status).toBe("ok");
  return unwrapPayload(body);
}

async function runBundleJob(page: Page, recipeId: string, parameters: Record<string, unknown>) {
  const start = await page.request.post(
    `/api/v1/web/workbench/admin/bundles/server-administrator/job-recipes/${recipeId}/start`,
    { data: { parameters } },
  );
  expect(start.ok(), await start.text()).toBe(true);
  const { job_id: jobId } = await start.json() as { job_id: string };
  const deadline = Date.now() + 2 * 60_000;
  while (Date.now() < deadline) {
    const response = await page.request.get(
      `/api/v1/web/workbench/admin/bundles/server-administrator/jobs/${jobId}`,
    );
    expect(response.ok(), await response.text()).toBe(true);
    const report = await response.json() as { status: string };
    if (report.status === "succeeded") return jobId;
    expect(["queued", "running"]).toContain(report.status);
    await page.waitForTimeout(500);
  }
  throw new Error(`${recipeId} did not finish within its bounded live-test window`);
}

test("reuses a monitored server and closes its bounded qualification recovery", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_4_CLUSTER_LIVE=1 for the actual 18085 cluster journey");
  test.setTimeout(3 * 60_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");

  await login(page);
  await page.goto("/web/workspace?id=server-administrator.fleet");
  await expect(page.getByRole("heading", { name: "Fleet setup" })).toBeVisible();

  for (const cluster of clusters) await ensureCluster(page, cluster);

  const enrollmentActivity = page.getByRole("region", { name: "Server enrollment activity" });
  const activeEnrollment = enrollmentActivity.getByText(targetLabel, { exact: true });
  await activeEnrollment.waitFor({ state: "visible", timeout: 10_000 }).catch(() => undefined);
  if (await activeEnrollment.count() === 0) {
    await page.getByRole("button", { name: "Add server", exact: true }).click();
    await page.getByLabel("Cluster", { exact: true }).selectOption(clusters[0].id);
    await page.getByRole("button", { name: "Continue to connection", exact: true }).click();

    await expect(page.getByRole("heading", { name: "Use a monitored server" })).toBeVisible();
    const existingServer = page.getByText(targetLabel, { exact: true });
    await expect(existingServer).toBeVisible();
    await page.getByRole("button", { name: `Use ${targetLabel}`, exact: true }).click();

    await expect(page.getByText("The server passed both gates and is available to the cluster."))
      .toBeVisible({ timeout: 2 * 60_000 });
  }

  await expect(enrollmentActivity.getByText(targetLabel, { exact: true })).toBeVisible();
  await expect(enrollmentActivity).toContainText(/Active|Quarantined|Qualifying/);
  const initialEnrollment = await enrollmentActivity.textContent();
  if (initialEnrollment?.includes("Quarantined")) {
    await enrollmentActivity.getByRole("button", { name: "Review details", exact: true }).click();
    await page.getByRole("button", { name: "Request retry", exact: true }).click();
    await expect(page.getByText("The server passed both gates and is available to the cluster."))
      .toBeVisible({ timeout: 2 * 60_000 });
  } else if (!initialEnrollment?.includes("Active")) {
    await enrollmentActivity.getByRole("button", { name: "Resume setup", exact: true }).click();
    await expect(page.getByText("The server passed both gates and is available to the cluster."))
      .toBeVisible({ timeout: 2 * 60_000 });
  }
  await expect(enrollmentActivity).toContainText("Development operations A");
  await expect(enrollmentActivity).toContainText("Active");
  await page.screenshot({
    path: "../../../.gadgetron/r4-4-actual-cluster-enrollment.png",
    fullPage: true,
  });

  if (!recoveryLive) return;
  expect(monitoringMarker, "GADGETRON_R4_4_MONITORING_MARKER is required").not.toBe("");

  const targetsResponse = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(targetsResponse.ok()).toBe(true);
  const targets = await targetsResponse.json() as {
    targets: Array<{ target_id: string; label: string }>;
  };
  const target = targets.targets.find((item) => item.label === targetLabel);
  expect(target, `registered target ${targetLabel} is required`).toBeTruthy();

  let originalMarker: Buffer | null = null;
  let restoreDocument: PolicyDocument | null = null;
  let reviewPolicyRevision = 0;
  let approvalId = "";
  try {
    originalMarker = await readFile(monitoringMarker).catch(() => null);
    const policyResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
    expect(policyResponse.ok()).toBe(true);
    const activePolicy = await policyResponse.json() as {
      policy: { revision: number; document: PolicyDocument };
    };
    restoreDocument = {
      ...activePolicy.policy.document,
      rules: activePolicy.policy.document.rules.filter((rule) => rule.id !== reviewPolicyRuleId),
    };
    const policyUpdate = await page.request.post(
      "/api/v1/web/workbench/admin/policy/revisions",
      {
        data: {
          expected_revision: activePolicy.policy.revision,
          document: {
            ...restoreDocument,
            rules: [{
              id: reviewPolicyRuleId,
              priority: 0,
              enabled: true,
              match: { action_ids: [transitionAction] },
              decision: "review",
              reason: "A Manager reviews quarantined capacity before qualification resumes",
            }, ...restoreDocument.rules],
          },
        },
      },
    );
    expect(policyUpdate.ok(), await policyUpdate.text()).toBe(true);
    reviewPolicyRevision = (await policyUpdate.json() as { policy: { revision: number } }).policy.revision;

    await rm(monitoringMarker, { force: true });
    await page.getByRole("button", { name: "Run qualification", exact: true }).click();
    await expect(page.getByText("A required check failed. The server is isolated from usable cluster capacity."))
      .toBeVisible({ timeout: 2 * 60_000 });
    await expect(page.getByText("Quarantined", { exact: true }).first()).toBeVisible();
    await expect(enrollmentActivity).toContainText("Health · Degraded");
    await expect(enrollmentActivity).toContainText("Compliance · Blocked");

    const repair = await page.request.post(
      `/api/v1/web/workbench/actions/${monitoringRepairAction}`,
      { data: { args: { target_id: target!.target_id } } },
    );
    expect(repair.ok(), await repair.text()).toBe(true);
    expect((await repair.json() as { result: { status: string } }).result.status).toBe("ok");

    await page.getByRole("button", { name: "Request retry", exact: true }).click();
    const reviewLink = page.getByRole("link", { name: "Open this request in Review" });
    await expect(reviewLink).toBeVisible();
    const href = await reviewLink.getAttribute("href");
    approvalId = new URL(href!, page.url()).searchParams.get("approval") ?? "";
    expect(approvalId).not.toBe("");

    const reviewPagePromise = page.context().waitForEvent("page");
    await reviewLink.click();
    const reviewPage = await reviewPagePromise;
    await reviewPage.waitForLoadState("domcontentloaded");
    const detail = reviewPage.getByTestId("approval-detail");
    await expect(detail).toContainText("server.enrollment-transition");
    await expect(detail.getByTestId("approval-arguments")).toContainText("qualifying");
    await detail.getByRole("button", { name: "Approve & run", exact: true }).click();
    await reviewPage.getByRole("dialog").getByRole("button", { name: "Approve & run", exact: true }).click();
    await expect(reviewPage.getByTestId(`approval-row-${approvalId}`)).toHaveCount(0, {
      timeout: 30_000,
    });
    await reviewPage.close();

    await page.bringToFront();
    await expect(page.getByText("The server passed both gates and is available to the cluster."))
      .toBeVisible({ timeout: 2 * 60_000 });
    await expect(enrollmentActivity).toContainText("Active");
    await expect(enrollmentActivity).toContainText("Health · Healthy");
    await expect(enrollmentActivity).toContainText(`Compliance · ${expectedCompliance}`);
    await page.screenshot({
      path: "../../../.gadgetron/r4-4-actual-enrollment-recovery.png",
      fullPage: true,
    });
  } finally {
    if (approvalId) {
      await page.request.post(`/api/v1/web/workbench/approvals/${approvalId}/deny`, {
        data: { reason: "Recovery verification cleanup" },
      }).catch(() => undefined);
    }
    if (restoreDocument && reviewPolicyRevision > 0) {
      const currentResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
      if (currentResponse.ok()) {
        const current = await currentResponse.json() as {
          policy: { revision: number; document: PolicyDocument };
        };
        if (current.policy.document.rules.some((rule) => rule.id === reviewPolicyRuleId)) {
          await page.request.post("/api/v1/web/workbench/admin/policy/revisions", {
            data: { expected_revision: current.policy.revision, document: restoreDocument },
          });
        }
      }
    }
    if (originalMarker) {
      await mkdir(dirname(monitoringMarker), { recursive: true });
      await writeFile(monitoringMarker, originalMarker);
    } else if (monitoringMarker) {
      await rm(monitoringMarker, { force: true });
    }
  }
});

test("reviews an exact profile rollout and returns the actual server to compliant capacity", async ({ page }) => {
  test.skip(!rolloutLive, "set GADGETRON_R4_4_ROLLOUT_LIVE=1 for the actual profile rollout journey");
  test.setTimeout(3 * 60_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");

  await login(page);
  await page.goto("/web/workspace?id=server-administrator.fleet");
  const enrollmentActivity = page.getByRole("region", { name: "Server enrollment activity" });
  await expect(enrollmentActivity.getByText(targetLabel, { exact: true })).toBeVisible();
  await expect(enrollmentActivity).toContainText("Compliance · Drift");

  let restoreDocument: PolicyDocument | null = null;
  let rolloutPolicyRevision = 0;
  let approvalId = "";
  try {
    const policyResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
    expect(policyResponse.ok()).toBe(true);
    const activePolicy = await policyResponse.json() as {
      policy: { revision: number; document: PolicyDocument };
    };
    restoreDocument = {
      ...activePolicy.policy.document,
      rules: activePolicy.policy.document.rules.filter((rule) => rule.id !== rolloutPolicyRuleId),
    };
    const policyUpdate = await page.request.post(
      "/api/v1/web/workbench/admin/policy/revisions",
      {
        data: {
          expected_revision: activePolicy.policy.revision,
          document: {
            ...restoreDocument,
            rules: [{
              id: rolloutPolicyRuleId,
              priority: 0,
              enabled: true,
              match: { action_ids: [rolloutAction] },
              decision: "review",
              reason: "A Manager reviews exact profile impact before usable capacity changes",
            }, ...restoreDocument.rules],
          },
        },
      },
    );
    expect(policyUpdate.ok(), await policyUpdate.text()).toBe(true);
    rolloutPolicyRevision = (await policyUpdate.json() as { policy: { revision: number } }).policy.revision;

    await enrollmentActivity.getByRole("button", { name: "Review profile update", exact: true }).click();
    const rolloutPlan = page.getByRole("region", { name: "Profile update plan" });
    await expect(rolloutPlan).toContainText("No effective setting changed");
    await rolloutPlan.getByRole("button", { name: "Apply & requalify", exact: true }).click();

    const reviewLink = page.getByRole("link", { name: "Open this request in Review" });
    await expect(page.getByText("Profile update waiting in Review", { exact: true })).toBeVisible();
    await expect(reviewLink).toBeVisible();
    const href = await reviewLink.getAttribute("href");
    approvalId = new URL(href!, page.url()).searchParams.get("approval") ?? "";
    expect(approvalId).not.toBe("");

    const reviewPagePromise = page.context().waitForEvent("page");
    await reviewLink.click();
    const reviewPage = await reviewPagePromise;
    await reviewPage.waitForLoadState("domcontentloaded");
    const detail = reviewPage.getByTestId("approval-detail");
    await expect(detail).toContainText("server.enrollment-rollout-apply");
    await expect(detail.getByTestId("approval-arguments")).toContainText("Expected cluster revision");
    await detail.getByRole("button", { name: "Approve & run", exact: true }).click();
    await reviewPage.getByRole("dialog").getByRole("button", { name: "Approve & run", exact: true }).click();
    await expect(reviewPage.getByTestId(`approval-row-${approvalId}`)).toHaveCount(0, {
      timeout: 30_000,
    });
    await reviewPage.close();

    await page.bringToFront();
    await expect(enrollmentActivity).toContainText("Compliance · Compliant", { timeout: 2 * 60_000 });
    await expect(enrollmentActivity).toContainText("Qualification · Passed");
    await expect(enrollmentActivity).toContainText("Active");
    await page.screenshot({
      path: "../../../.gadgetron/r4-4-actual-profile-rollout.png",
      fullPage: true,
    });
  } finally {
    if (approvalId) {
      await page.request.post(`/api/v1/web/workbench/approvals/${approvalId}/deny`, {
        data: { reason: "Profile rollout verification cleanup" },
      }).catch(() => undefined);
    }
    if (restoreDocument && rolloutPolicyRevision > 0) {
      const currentResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
      if (currentResponse.ok()) {
        const current = await currentResponse.json() as {
          policy: { revision: number; document: PolicyDocument };
        };
        if (current.policy.document.rules.some((rule) => rule.id === rolloutPolicyRuleId)) {
          await page.request.post("/api/v1/web/workbench/admin/policy/revisions", {
            data: { expected_revision: current.policy.revision, document: restoreDocument },
          });
        }
      }
    }
  }
});

test("reviews a setup profile change, reapplies it over SSH, and requalifies the server", async ({ page }) => {
  test.skip(
    !setupRolloutLive,
    "set GADGETRON_R4_4_SETUP_ROLLOUT_LIVE=1 for the actual existing-target setup journey",
  );
  test.setTimeout(4 * 60_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");
  expect(sudoPassword, "GADGETRON_R4_4_SUDO_PASSWORD is required").not.toBe("");

  await login(page);
  await page.goto("/web/workspace?id=server-administrator.fleet");
  const enrollmentActivity = page.getByRole("region", { name: "Server enrollment activity" });
  await expect(enrollmentActivity.getByText(targetLabel, { exact: true })).toBeVisible();
  await expect(enrollmentActivity).toContainText("Active");

  const targetResponse = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(targetResponse.ok(), await targetResponse.text()).toBe(true);
  const targetPayload = await targetResponse.json() as {
    targets: Array<{ target_id: string; label: string }>;
  };
  const target = targetPayload.targets.find((candidate) => candidate.label === targetLabel);
  expect(target, `registered target ${targetLabel} is required`).toBeTruthy();

  const clusterPayload = await invokeAction(page, clusterListAction, { status: "active", limit: 100 }) as {
    rows: Array<{
      cluster_id: string;
      label: string;
      environment: string;
      purpose: string;
      base_profile_id: string;
      base_profile_revision: string;
      cluster_profile_id: string;
      cluster_profile_revision: string;
      roles: Array<{
        role_id: string;
        label: string;
        profile: { profile_id: string; revision: string };
      }>;
    }>;
  };
  const cluster = clusterPayload.rows.find((candidate) => candidate.cluster_id === clusters[0].id);
  expect(cluster, `cluster ${clusters[0].id} is required`).toBeTruthy();

  const profilePayload = await invokeAction(page, profileListAction, {
    scope: "platform_base",
    limit: 100,
  }) as {
    rows: Array<{
      profile_id: string;
      revision: string;
      scope: string;
      label: string;
      spec: Record<string, unknown>;
    }>;
  };
  const baseProfile = profilePayload.rows.find((candidate) => (
    candidate.profile_id === cluster!.base_profile_id
      && candidate.revision === cluster!.base_profile_revision
  ));
  expect(baseProfile, "the exact current platform profile is required").toBeTruthy();

  const nextSpec = structuredClone(baseProfile!.spec);
  const setup = nextSpec.setup as { features?: unknown } | undefined;
  expect(setup && Array.isArray(setup.features), "the platform profile must declare setup features")
    .toBeTruthy();
  const currentFeatures = setup!.features as string[];
  const removesDcgm = currentFeatures.includes("nvidia_dcgm");
  setup!.features = removesDcgm
    ? currentFeatures.filter((feature) => feature !== "nvidia_dcgm")
    : [...currentFeatures, "nvidia_dcgm"];

  const createdProfile = await invokeAction(page, profileCreateAction, {
    profile_id: baseProfile!.profile_id,
    scope: baseProfile!.scope,
    label: baseProfile!.label,
    spec: nextSpec,
  }) as { profile_id: string; revision: string };
  expect(createdProfile.profile_id).toBe(baseProfile!.profile_id);
  expect(createdProfile.revision).not.toBe(baseProfile!.revision);

  await invokeAction(page, clusterUpsertAction, {
    cluster_id: cluster!.cluster_id,
    label: cluster!.label,
    environment: cluster!.environment,
    purpose: cluster!.purpose,
    base_profile: {
      profile_id: createdProfile.profile_id,
      revision: createdProfile.revision,
    },
    cluster_profile: {
      profile_id: cluster!.cluster_profile_id,
      revision: cluster!.cluster_profile_revision,
    },
    roles: cluster!.roles,
  });
  await runBundleJob(page, "server-duty-cycle", { target_id: target!.target_id });

  await page.reload();
  await expect(enrollmentActivity).toContainText("Compliance · Drift", { timeout: 30_000 });

  let restoreDocument: PolicyDocument | null = null;
  let rolloutPolicyRevision = 0;
  let approvalId = "";
  try {
    const policyResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
    expect(policyResponse.ok(), await policyResponse.text()).toBe(true);
    const activePolicy = await policyResponse.json() as {
      policy: { revision: number; document: PolicyDocument };
    };
    restoreDocument = {
      ...activePolicy.policy.document,
      rules: activePolicy.policy.document.rules.filter((rule) => rule.id !== rolloutPolicyRuleId),
    };
    const policyUpdate = await page.request.post(
      "/api/v1/web/workbench/admin/policy/revisions",
      {
        data: {
          expected_revision: activePolicy.policy.revision,
          document: {
            ...restoreDocument,
            rules: [{
              id: rolloutPolicyRuleId,
              priority: 0,
              enabled: true,
              match: { action_ids: [rolloutAction] },
              decision: "review",
              reason: "A Manager reviews exact setup impact before server configuration changes",
            }, ...restoreDocument.rules],
          },
        },
      },
    );
    expect(policyUpdate.ok(), await policyUpdate.text()).toBe(true);
    rolloutPolicyRevision = (await policyUpdate.json() as {
      policy: { revision: number };
    }).policy.revision;

    await enrollmentActivity.getByRole("button", { name: "Review profile update", exact: true }).click();
    const rolloutPlan = page.getByRole("region", { name: "Profile update plan" });
    await expect(rolloutPlan).toContainText("configuration and qualification");
    await expect(rolloutPlan).toContainText(removesDcgm ? "Remove: Nvidia dcgm" : "Add: Nvidia dcgm");
    await rolloutPlan.getByRole("button", { name: "Review setup update", exact: true }).click();

    const reviewLink = page.getByRole("link", { name: "Open this request in Review" });
    await expect(page.getByText("Profile update waiting in Review", { exact: true })).toBeVisible();
    await expect(reviewLink).toBeVisible();
    const href = await reviewLink.getAttribute("href");
    approvalId = new URL(href!, page.url()).searchParams.get("approval") ?? "";
    expect(approvalId).not.toBe("");

    const reviewPagePromise = page.context().waitForEvent("page");
    await reviewLink.click();
    const reviewPage = await reviewPagePromise;
    await reviewPage.waitForLoadState("domcontentloaded");
    const detail = reviewPage.getByTestId("approval-detail");
    await expect(detail).toContainText("server.enrollment-rollout-apply");
    await expect(detail.getByTestId("approval-arguments")).toContainText("Expected cluster revision");
    await detail.getByRole("button", { name: "Approve & run", exact: true }).click();
    await reviewPage.getByRole("dialog").getByRole("button", { name: "Approve & run", exact: true }).click();
    await expect(reviewPage.getByTestId(`approval-row-${approvalId}`)).toHaveCount(0, {
      timeout: 30_000,
    });
    await reviewPage.close();

    await page.bringToFront();
    await expect(page.getByRole("heading", { name: "Apply approved server setup" }))
      .toBeVisible({ timeout: 30_000 });
    if (removesDcgm) await expect(page.getByText(/No longer managed: Nvidia dcgm/)).toBeVisible();
    await page.getByLabel("Server administrator password").fill(sudoPassword);
    await page.getByRole("button", { name: "Apply setup & continue", exact: true }).click();
    await expect(enrollmentActivity).toContainText("Active", { timeout: 2 * 60_000 });
    await expect(enrollmentActivity).toContainText("Health · Healthy");
    await expect(enrollmentActivity).toContainText("Compliance · Compliant");
    await expect(enrollmentActivity).toContainText("Qualification · Passed");
    await expect(page.getByLabel("Server administrator password")).toHaveCount(0);
    await page.screenshot({
      path: "../../../.gadgetron/r4-4-actual-setup-profile-rollout.png",
      fullPage: true,
    });
  } finally {
    if (approvalId) {
      await page.request.post(`/api/v1/web/workbench/approvals/${approvalId}/deny`, {
        data: { reason: "Setup profile rollout verification cleanup" },
      }).catch(() => undefined);
    }
    if (restoreDocument && rolloutPolicyRevision > 0) {
      const currentResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
      if (currentResponse.ok()) {
        const current = await currentResponse.json() as {
          policy: { revision: number; document: PolicyDocument };
        };
        if (current.policy.document.rules.some((rule) => rule.id === rolloutPolicyRuleId)) {
          await page.request.post("/api/v1/web/workbench/admin/policy/revisions", {
            data: { expected_revision: current.policy.revision, document: restoreDocument },
          });
        }
      }
    }
  }
});

test("bootstraps a distinct second server and activates it in the B cluster", async ({ page }) => {
  test.skip(
    !secondTargetLive,
    "set GADGETRON_R4_4_SECOND_TARGET_LIVE=1 with a distinct actual Linux server",
  );
  test.setTimeout(20 * 60_000);
  page.setDefaultTimeout(15_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");
  expect(secondTargetAddress, "GADGETRON_R4_4_SECOND_TARGET_ADDRESS is required").not.toBe("");
  expect(secondTargetUser, "GADGETRON_R4_4_SECOND_TARGET_USER is required").not.toBe("");
  expect(Number.isInteger(secondTargetPort) && secondTargetPort > 0 && secondTargetPort <= 65_535)
    .toBe(true);

  await login(page);
  await page.goto("/web/workspace?id=server-administrator.fleet");
  const bCluster = page
    .getByRole("heading", { name: clusters[1].label, exact: true })
    .first()
    .locator("../..");

  const targetsBeforeResponse = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(targetsBeforeResponse.ok(), await targetsBeforeResponse.text()).toBe(true);
  const targetsBefore = await targetsBeforeResponse.json() as {
    targets: Array<{
      target_id: string;
      label: string;
      address: string;
      port: number;
      credential_origin: string;
    }>;
  };
  const firstTarget = targetsBefore.targets.find((target) => target.label === targetLabel);
  expect(firstTarget, `existing A-cluster target ${targetLabel} is required`).toBeTruthy();
  const enrollmentSucceeded = page.getByText(
    "The server passed both gates and is available to the cluster.",
  );
  const bootstrapFailed = page.getByText(/setup stopped$/);
  const enrollmentFailed = page.getByText(
    "A required check failed. The server is isolated from usable cluster capacity.",
  );
  let secondTarget = targetsBefore.targets.find((target) => (
    target.address === secondTargetAddress && target.port === secondTargetPort
  ));

  if (!secondTarget) {
    expect(secondTargetPassword, "GADGETRON_R4_4_SECOND_TARGET_PASSWORD is required").not.toBe("");
    await expect(bCluster).toContainText("0 servers");
    const addServer = page.getByRole("button", { name: "Add server", exact: true });
    const addServerHeading = page.getByRole("heading", { name: "Add server to fleet", exact: true });
    const addServerPanel = page.locator("section").filter({ has: addServerHeading });
    await expect(async () => {
      await addServer.click();
      await expect(addServerHeading).toBeVisible({ timeout: 2_000 });
    }).toPass({ timeout: 15_000 });
    await addServerPanel.locator("select").nth(0).selectOption(clusters[1].id);
    await addServerPanel.locator("select").nth(1).selectOption(clusters[1].roleId);
    await page.getByRole("button", { name: "Continue to connection", exact: true }).click();
    await page.getByLabel("IP address or DNS").fill(secondTargetAddress);
    await page.getByLabel("SSH ID").fill(secondTargetUser);
    await page.getByLabel("Password", { exact: true }).fill(secondTargetPassword);
    if (secondTargetPort !== 22 || secondTargetSudoPassword || secondTargetLabel) {
      await page.getByText("Connection options", { exact: true }).click();
      if (secondTargetLabel) await page.getByLabel("Server name").fill(secondTargetLabel);
      if (secondTargetPort !== 22) {
        await page.getByLabel("SSH port").fill(String(secondTargetPort));
      }
      if (secondTargetSudoPassword) {
        await page.getByLabel("Sudo password").fill(secondTargetSudoPassword);
      }
    }
    await page.getByRole("button", { name: "Set up & register", exact: true }).click();
    await expect(enrollmentSucceeded.or(bootstrapFailed).or(enrollmentFailed))
      .toBeVisible({ timeout: 18 * 60_000 });
    if (await bootstrapFailed.isVisible()) {
      const failureNotice = bootstrapFailed.locator("..");
      await failureNotice.getByRole("button", { name: "Details", exact: true }).click();
      throw new Error(`Server bootstrap failed: ${await failureNotice.locator("pre").innerText()}`);
    }
    if (await enrollmentFailed.isVisible()) {
      await page.getByRole("button", { name: "Request retry", exact: true }).click();
      await expect(enrollmentSucceeded).toBeVisible({ timeout: 3 * 60_000 });
    }
    await expect(page.getByLabel("Password", { exact: true })).toHaveCount(0);
  } else {
    expect(secondTarget.credential_origin).toBe("bootstrap");
    const activity = page.getByRole("region", { name: "Server enrollment activity" });
    const enrollmentRow = activity.getByText(secondTarget.label, { exact: true }).locator("../..");
    await expect(enrollmentRow).toBeVisible();
    const state = await enrollmentRow.textContent();
    if (state?.includes("Quarantined")) {
      await enrollmentRow.getByRole("button", { name: "Review details", exact: true }).click();
      await page.getByRole("button", { name: "Request retry", exact: true }).click();
      await expect(enrollmentSucceeded).toBeVisible({ timeout: 3 * 60_000 });
    } else {
      expect(state).toContain("Active");
    }
  }
  await expect(bCluster).toContainText("1 servers");

  const targetsAfterResponse = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(targetsAfterResponse.ok(), await targetsAfterResponse.text()).toBe(true);
  const targetsAfter = await targetsAfterResponse.json() as typeof targetsBefore;
  secondTarget = targetsAfter.targets.find((target) => (
    target.address === secondTargetAddress
      && target.port === secondTargetPort
      && target.credential_origin === "bootstrap"
  ));
  expect(secondTarget, "the second endpoint must remain as a bootstrap-owned active target")
    .toBeTruthy();
  expect(secondTarget!.target_id).not.toBe(firstTarget!.target_id);

  const serversResponse = await page.request.get(
    "/api/v1/web/workbench/views/server-administrator.servers/data",
  );
  expect(serversResponse.ok(), await serversResponse.text()).toBe(true);
  const servers = await serversResponse.json() as {
    payload: { rows: Array<{ target_id: string; machine_id?: string | null }> };
  };
  const firstServer = servers.payload.rows.find((row) => row.target_id === firstTarget!.target_id);
  const secondServer = servers.payload.rows.find((row) => row.target_id === secondTarget!.target_id);
  expect(firstServer?.machine_id, "the A target needs a collected machine identity").toBeTruthy();
  expect(secondServer?.machine_id, "the B target needs a collected machine identity").toBeTruthy();
  expect(secondServer!.machine_id, "A and B must be distinct actual machines")
    .not.toBe(firstServer!.machine_id);

  const enrollmentPayload = await invokeAction(
    page,
    "server-administrator.fleet.action.server.enrollments-list",
    { cluster_id: clusters[1].id, limit: 100 },
  ) as {
    rows: Array<{
      target_id: string;
      lifecycle_state: string;
      health_status: string;
      compliance_status: string;
      qualification_status: string;
    }>;
  };
  expect(enrollmentPayload.rows).toContainEqual(expect.objectContaining({
    target_id: secondTarget!.target_id,
    lifecycle_state: "active",
    health_status: "healthy",
    compliance_status: "compliant",
    qualification_status: "passed",
  }));
  await page.screenshot({
    path: "../../../.gadgetron/r4-4-actual-second-target-b-cluster.png",
    fullPage: true,
  });
});
