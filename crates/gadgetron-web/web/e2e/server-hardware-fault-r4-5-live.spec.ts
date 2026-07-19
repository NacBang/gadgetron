import { expect, test, type Page } from "@playwright/test";

import { unwrapPayload, type ActionResponse } from "../app/lib/workbench-client";
import { isPhysicalHardwareFaultEvidence } from "./physical-hardware-fault-contract";

const live = process.env.GADGETRON_R4_5_HARDWARE_FAULT_LIVE === "1";
const recoveryLive = process.env.GADGETRON_R4_5_HARDWARE_RECOVERY_LIVE === "1";
const email = process.env.GADGETRON_R4_5_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R4_5_PASSWORD ?? "";
const targetAddress = process.env.GADGETRON_R4_4_SECOND_TARGET_ADDRESS ?? "";
const targetPort = Number.parseInt(process.env.GADGETRON_R4_4_SECOND_TARGET_PORT ?? "22", 10);
const expectedRule = process.env.GADGETRON_R4_5_HARDWARE_RULE ?? "";
const expectedMetric = process.env.GADGETRON_R4_5_HARDWARE_METRIC ?? "";
const bClusterId = process.env.GADGETRON_R4_5_HARDWARE_CLUSTER ?? "gpu-research-b";
const bundleId = "server-administrator";
const transitionAction = `${bundleId}.fleet.action.server.enrollment-transition`;

type Target = {
  target_id: string;
  label: string;
  address: string;
  port: number;
};

type Incident = {
  incident_id: string;
  title: string;
  summary: string;
  severity: string;
  status: string;
  target_id: string;
  rule_key: string;
  next_action: string;
};

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

async function invokeAction(page: Page, actionId: string, args: Record<string, unknown>) {
  const response = await page.request.post(
    `/api/v1/web/workbench/actions/${actionId}`,
    { data: { args } },
  );
  expect(response.ok(), await response.text()).toBe(true);
  let body = await response.json() as ActionResponse;
  if (body.result?.status === "pending_approval") {
    const approvalId = body.result.approval_id;
    expect(approvalId, `${actionId} returned Review without an approval id`).toBeTruthy();
    const approval = await page.request.post(
      `/api/v1/web/workbench/approvals/${approvalId}/approve`,
    );
    expect(approval.ok(), await approval.text()).toBe(true);
    body = await approval.json() as ActionResponse;
  }
  expect(body.result?.status).toBe("ok");
  return unwrapPayload(body);
}

async function runBundleJob(
  page: Page,
  recipeId: "server-duty-cycle" | "server-enrollment",
  parameters: Record<string, unknown>,
) {
  let jobId = "";
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await page.request.post(
      `/api/v1/web/workbench/admin/bundles/${bundleId}/job-recipes/${recipeId}/start`,
      { data: { parameters } },
    );
    if (response.status() === 409) {
      await page.waitForTimeout(500);
      continue;
    }
    expect(response.ok(), await response.text()).toBe(true);
    jobId = (await response.json() as { job_id: string }).job_id;
    break;
  }
  expect(jobId, `${recipeId} target remained busy beyond the bounded retry window`).not.toBe("");

  await expect.poll(async () => {
    const response = await page.request.get(
      `/api/v1/web/workbench/admin/bundles/${bundleId}/jobs/${jobId}`,
    );
    expect(response.ok(), await response.text()).toBe(true);
    return (await response.json() as { status: string }).status;
  }, { timeout: recipeId === "server-enrollment" ? 3 * 60_000 : 2 * 60_000 }).toBe("succeeded");
}

async function runDutyCycle(page: Page, targetId: string) {
  await runBundleJob(page, "server-duty-cycle", { target_id: targetId });
}

test("projects a collector-observed second-target hardware fault into a safe incident context", async ({ page }) => {
  test.skip(
    !live,
    "set GADGETRON_R4_5_HARDWARE_FAULT_LIVE=1 only while an actual second-target fault is observable",
  );
  test.setTimeout(4 * 60_000);
  expect(password, "GADGETRON_R4_5_PASSWORD is required").not.toBe("");
  expect(targetAddress, "GADGETRON_R4_4_SECOND_TARGET_ADDRESS is required").not.toBe("");
  expect(Number.isInteger(targetPort) && targetPort > 0 && targetPort <= 65_535).toBe(true);
  expect(
    isPhysicalHardwareFaultEvidence(expectedRule, expectedMetric),
    "hardware rule and metric must identify the same invariant or a critical physical sensor",
  ).toBe(true);

  await login(page);

  const targetsResponse = await page.request.get(
    `/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`,
  );
  expect(targetsResponse.ok(), await targetsResponse.text()).toBe(true);
  const targets = (await targetsResponse.json() as { targets: Target[] }).targets;
  const target = targets.find((item) => item.address === targetAddress && item.port === targetPort);
  expect(target, "the declared second endpoint must already be registered").toBeTruthy();

  const serversResponse = await page.request.get(
    `/api/v1/web/workbench/views/${bundleId}.servers/data`,
  );
  expect(serversResponse.ok(), await serversResponse.text()).toBe(true);
  const servers = (await serversResponse.json() as {
    payload: { rows: Array<{ target_id: string; machine_id?: string | null }> };
  }).payload.rows;
  const server = servers.find((row) => row.target_id === target!.target_id);
  expect(server?.machine_id, "the fault target needs a collected machine identity").toBeTruthy();
  expect(
    servers.some((row) => row.target_id !== target!.target_id
      && row.machine_id
      && row.machine_id !== server!.machine_id),
    "the hardware matrix requires another collected physical machine",
  ).toBe(true);

  const enrollment = await invokeAction(
    page,
    `${bundleId}.fleet.action.server.enrollments-list`,
    { cluster_id: bClusterId, limit: 100 },
  ) as { rows: Array<{ enrollment_id: string; target_id: string; lifecycle_state: string }> };
  expect(enrollment.rows).toContainEqual(expect.objectContaining({
    target_id: target!.target_id,
    lifecycle_state: "active",
  }));
  const targetEnrollment = enrollment.rows.find((row) => row.target_id === target!.target_id)!;

  await runDutyCycle(page, target!.target_id);

  const metricsResponse = await page.request.get(
    `/api/v1/web/workbench/views/${bundleId}.metrics/data`,
  );
  expect(metricsResponse.ok(), await metricsResponse.text()).toBe(true);
  const metrics = (await metricsResponse.json() as {
    payload: { rows: Array<{ target_id: string; metric: string }> };
  }).payload.rows;
  expect(metrics).toContainEqual(expect.objectContaining({
    target_id: target!.target_id,
    metric: expectedMetric,
  }));

  const incidentsResponse = await page.request.get(
    `/api/v1/web/workbench/views/${bundleId}.alerts/data`,
  );
  expect(incidentsResponse.ok(), await incidentsResponse.text()).toBe(true);
  const incidents = (await incidentsResponse.json() as { payload: { rows: Incident[] } }).payload.rows;
  const incident = incidents.find((row) => (
    row.target_id === target!.target_id
      && row.rule_key === expectedRule
      && row.status === "firing"
  ));
  expect(
    incident,
    "no collector-observed firing hardware incident matched the declared target and rule",
  ).toBeTruthy();
  expect(incident!.severity).toBe("critical");

  const context = await invokeAction(
    page,
    `${bundleId}.alerts.action.server.incident-context`,
    { incident_id: incident!.incident_id, target_id: target!.target_id },
  ) as {
    id: string;
    title: string;
    facts: {
      target_id: string;
      health_revision: string;
      rule: string;
      status: string;
      lifecycle_status: string;
      linked_operation_outcomes: number;
      learning_handoffs: number;
      recommended_next_action: string;
      signals: Array<{ state: string; summary: string }>;
      timeline: Array<{ kind: string; summary: string }>;
    };
    prompt: string;
  };
  expect(context.id).toBe(incident!.incident_id);
  expect(context.facts).toMatchObject({
    target_id: target!.target_id,
    rule: expectedRule,
    status: "firing",
  });
  expect(context.facts.recommended_next_action).not.toBe("");
  expect(context.facts.signals).toEqual(expect.arrayContaining([
    expect.objectContaining({ state: "firing", summary: expect.stringContaining(expectedMetric) }),
  ]));
  expect(context.prompt).toContain("safe stop");

  const safeStop = await invokeAction(page, transitionAction, {
    enrollment_id: targetEnrollment.enrollment_id,
    to: "quarantined",
    reason: `${expectedMetric} remains a collector-observed critical physical fault`,
    incident_id: incident!.incident_id,
  }) as {
    target_id: string;
    to: string;
    incident_id: string;
    operation_id: string;
    after: { lifecycle_state: string; capacity: string };
    experience: { state: string; outcome_tracking: { state: string } };
  };
  expect(safeStop).toMatchObject({
    target_id: target!.target_id,
    to: "quarantined",
    incident_id: incident!.incident_id,
    after: { lifecycle_state: "quarantined", capacity: "isolated" },
    experience: { state: "recorded", outcome_tracking: { state: "linked" } },
  });
  expect(safeStop.operation_id).not.toBe("");

  const quarantined = await invokeAction(
    page,
    `${bundleId}.fleet.action.server.enrollments-list`,
    { cluster_id: bClusterId, limit: 100 },
  ) as { rows: Array<{ enrollment_id: string; lifecycle_state: string }> };
  expect(quarantined.rows).toContainEqual(expect.objectContaining({
    enrollment_id: targetEnrollment.enrollment_id,
    lifecycle_state: "quarantined",
  }));

  const safeContext = await invokeAction(
    page,
    `${bundleId}.alerts.action.server.incident-context`,
    { incident_id: incident!.incident_id, target_id: target!.target_id },
  ) as typeof context;
  expect(safeContext.facts.lifecycle_status).toBe("quarantined");
  expect(safeContext.facts.linked_operation_outcomes).toBeGreaterThanOrEqual(1);
  expect(safeContext.facts.learning_handoffs).toBeGreaterThanOrEqual(1);
  expect(safeContext.facts.timeline).toEqual(expect.arrayContaining([
    expect.objectContaining({ kind: "action_succeeded" }),
    expect.objectContaining({ kind: "experience_recorded" }),
  ]));

  await page.goto(`/web/workspace?id=${bundleId}.alerts`);
  await expect(page.getByRole("heading", { name: "Incidents", exact: true })).toBeVisible();
  const card = page.getByText(incident!.summary, { exact: true }).locator("..");
  await expect(card).toContainText(incident!.next_action);
  await card.getByRole("button", { name: /Ask Penny/ }).click();
  await expect(page.getByTestId("penny-companion")).toContainText(context.title);
  await page.screenshot({
    path: "../../../.gadgetron/r4-5-actual-hardware-fault.png",
    fullPage: true,
  });
});

test("returns the remediated hardware-fault target through fresh validation to usable capacity", async ({ page }) => {
  test.skip(
    !recoveryLive,
    "set GADGETRON_R4_5_HARDWARE_RECOVERY_LIVE=1 only after the observed physical fault is remediated",
  );
  test.setTimeout(7 * 60_000);
  expect(password, "GADGETRON_R4_5_PASSWORD is required").not.toBe("");
  expect(targetAddress, "GADGETRON_R4_4_SECOND_TARGET_ADDRESS is required").not.toBe("");
  expect(Number.isInteger(targetPort) && targetPort > 0 && targetPort <= 65_535).toBe(true);
  expect(
    isPhysicalHardwareFaultEvidence(expectedRule, expectedMetric),
    "hardware recovery must use the same collector invariant as the safe-stop phase",
  ).toBe(true);

  await login(page);

  const targetsResponse = await page.request.get(
    `/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`,
  );
  expect(targetsResponse.ok(), await targetsResponse.text()).toBe(true);
  const targets = (await targetsResponse.json() as { targets: Target[] }).targets;
  const target = targets.find((item) => item.address === targetAddress && item.port === targetPort);
  expect(target, "the remediated endpoint must be the registered safe-stop target").toBeTruthy();

  const before = await invokeAction(
    page,
    `${bundleId}.fleet.action.server.enrollments-list`,
    { cluster_id: bClusterId, limit: 100 },
  ) as {
    rows: Array<{
      enrollment_id: string;
      target_id: string;
      lifecycle_state: string;
    }>;
  };
  const enrollment = before.rows.find((row) => row.target_id === target!.target_id);
  expect(enrollment, "the hardware-fault target must still belong to its declared cluster").toBeTruthy();
  expect(
    enrollment!.lifecycle_state,
    "recovery evidence starts from incident-isolated capacity",
  ).toBe("quarantined");

  await runDutyCycle(page, target!.target_id);

  const incidentsResponse = await page.request.get(
    `/api/v1/web/workbench/views/${bundleId}.alerts/data`,
  );
  expect(incidentsResponse.ok(), await incidentsResponse.text()).toBe(true);
  const incidents = (await incidentsResponse.json() as { payload: { rows: Incident[] } }).payload.rows;
  const incident = incidents.find((row) => (
    row.target_id === target!.target_id
      && row.rule_key === expectedRule
      && row.status === "closed"
  ));
  expect(
    incident,
    "the exact collector-observed hardware episode must be closed after physical remediation",
  ).toBeTruthy();
  expect(incidents).not.toContainEqual(expect.objectContaining({
    target_id: target!.target_id,
    rule_key: expectedRule,
    status: "firing",
  }));

  const release = await invokeAction(page, transitionAction, {
    enrollment_id: enrollment!.enrollment_id,
    to: "commissioning",
    reason: `${expectedMetric} cleared; start a fresh physical-fault recovery validation cycle`,
  }) as { target_id: string; from: string; to: string };
  expect(release).toMatchObject({
    target_id: target!.target_id,
    from: "quarantined",
    to: "commissioning",
  });

  await runBundleJob(page, "server-enrollment", {
    target_id: target!.target_id,
    enrollment_id: enrollment!.enrollment_id,
  });

  const after = await invokeAction(
    page,
    `${bundleId}.fleet.action.server.enrollments-list`,
    { cluster_id: bClusterId, limit: 100 },
  ) as {
    rows: Array<{
      enrollment_id: string;
      lifecycle_state: string;
      health_status: string;
      compliance_status: string;
      commissioning_status: string;
      qualification_status: string;
    }>;
  };
  expect(after.rows).toContainEqual(expect.objectContaining({
    enrollment_id: enrollment!.enrollment_id,
    lifecycle_state: "active",
    health_status: "healthy",
    compliance_status: "compliant",
    commissioning_status: "passed",
    qualification_status: "passed",
  }));

  const context = await invokeAction(
    page,
    `${bundleId}.alerts.action.server.incident-context`,
    { incident_id: incident!.incident_id, target_id: target!.target_id },
  ) as {
    title: string;
    facts: {
      status: string;
      lifecycle_status: string;
      linked_operation_outcomes: number;
      learning_handoffs: number;
      active_signal_count: number;
      timeline: Array<{ kind: string; summary: string; details: { action?: string } }>;
    };
  };
  expect(context.facts).toMatchObject({
    status: "closed",
    lifecycle_status: "active",
    active_signal_count: 0,
  });
  expect(context.facts.linked_operation_outcomes).toBeGreaterThanOrEqual(2);
  expect(context.facts.learning_handoffs).toBeGreaterThanOrEqual(2);
  expect(context.facts.timeline).toEqual(expect.arrayContaining([
    expect.objectContaining({
      kind: "action_succeeded",
      details: expect.objectContaining({ action: "incident-recovery" }),
    }),
  ]));

  await page.goto(`/web/workspace?id=${bundleId}.alerts`);
  await expect(page.getByRole("heading", { name: "Incidents", exact: true })).toBeVisible();
  const card = page.getByText(incident!.summary, { exact: true }).locator("..");
  await expect(card).toContainText(/closed/i);
  await card.getByRole("button", { name: /Ask Penny/ }).click();
  await expect(page.getByTestId("penny-companion")).toContainText(context.title);
  await page.screenshot({
    path: "../../../.gadgetron/r4-5-actual-hardware-recovery.png",
    fullPage: true,
  });
});
