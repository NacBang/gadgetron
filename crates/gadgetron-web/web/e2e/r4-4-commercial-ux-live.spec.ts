import { randomUUID } from "node:crypto";
import { expect, test, type Page } from "@playwright/test";

import {
  expectAccessible,
  expectReadableTextControls,
} from "./support/ui-assertions";

const live = process.env.GADGETRON_R4_4_LIVE === "1";
const email = process.env.GADGETRON_R4_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R4_4_PASSWORD ?? "";
const expectedVersion = process.env.GADGETRON_R4_4_VERSION ?? "0.8.17";
const expectedServerVersion = process.env.GADGETRON_R4_4_SERVER_VERSION ?? "0.4.23";
const reviewPolicyRuleId = "r4-4-live-review";

type PolicyDocument = {
  schema_version: number;
  default_decision: "auto" | "review" | "deny";
  default_reason: string;
  rules: Array<Record<string, unknown> & { id: string }>;
};

async function login(page: Page) {
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

async function expectNoHorizontalOverflow(page: Page) {
  expect(await page.evaluate(
    () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
  )).toBe(false);
}

async function findKnowledgeGraphCenter(page: Page) {
  const spacesResponse = await page.request.get("/api/v1/web/workbench/knowledge/spaces");
  expect(spacesResponse.ok()).toBe(true);
  const spaces = await spacesResponse.json() as {
    spaces: Array<{ id: string; status: string }>;
  };
  for (const space of spaces.spaces.filter((item) => item.status === "active")) {
    const objectsResponse = await page.request.get(
      `/api/v1/web/workbench/knowledge/spaces/${space.id}/objects?canonical_kind=note`,
    );
    if (!objectsResponse.ok()) continue;
    const objects = await objectsResponse.json() as {
      objects: Array<{ id: string; home_bundle_id: string; path: string; title?: string }>;
    };
    const object = objects.objects.find(
      (item) => item.home_bundle_id === "server-operations-intelligence",
    );
    if (object) {
      return {
        spaceId: space.id,
        bundleId: object.home_bundle_id,
        objectId: object.id,
        title: object.title || object.path.replace(/^notes\//, "").replace(/\.md$/, ""),
      };
    }
  }
  throw new Error("R4.4 requires one actor-visible Knowledge note for the actual graph journey");
}

test("closes the actual actor and DB commercial UX journey", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_4_LIVE=1 for the release-built 18085 service");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");

  await login(page);
  await expect(page.getByTestId("version-badge")).toContainText(expectedVersion);
  const brand = page.getByTestId("brand");
  const logo = brand.getByRole("img", { name: "ManyCoreSoft", exact: true });
  const product = brand.getByText("Gadgetron", { exact: true });
  await expect(brand).toBeVisible();
  await expect(logo).toBeVisible();
  await expect(product).toBeVisible();
  await logo.evaluate(async (element) => (element as HTMLImageElement).decode());
  const [logoBox, productBox] = await Promise.all([
    logo.boundingBox(),
    product.boundingBox(),
  ]);
  expect(logoBox).not.toBeNull();
  expect(productBox).not.toBeNull();
  expect(logoBox!.height).toBe(18);
  expect(productBox!.height).toBe(18);
  expect(Math.abs(
    logoBox!.y + logoBox!.height / 2 - (productBox!.y + productBox!.height / 2),
  )).toBeLessThanOrEqual(0.5);
  await brand.screenshot({ path: "../../../.gadgetron/r4-4-brand-lockup.png" });

  const capabilities = await page.request.get("/api/v1/web/workbench/capabilities");
  expect(capabilities.ok()).toBe(true);
  const snapshot = await capabilities.json() as {
    bundles: Array<{ bundle_id: string; bundle_version: string }>;
  };
  expect(snapshot.bundles).toEqual(expect.arrayContaining([
    expect.objectContaining({
      bundle_id: "server-administrator",
      bundle_version: expectedServerVersion,
    }),
  ]));
  for (const bundleId of [
    "travel-planner",
    "travel-intelligence",
    "restaurant-research",
    "news-intelligence",
    "community-intelligence",
    "social-media-intelligence",
  ]) {
    expect(snapshot.bundles.some((bundle) => bundle.bundle_id === bundleId)).toBe(false);
  }
  const bundlesResponse = await page.request.get("/api/v1/web/workbench/admin/bundles");
  expect(bundlesResponse.ok()).toBe(true);
  const installedBundles = await bundlesResponse.json() as {
    bundles: Array<{
      bundle?: { id: string; version: string };
      runtime?: { state: string; health: string };
    }>;
  };
  for (const [bundleId, version] of [
    ["server-administrator", expectedServerVersion],
    ["server-operations-intelligence", "0.1.0"],
  ] as const) {
    expect(installedBundles.bundles).toEqual(expect.arrayContaining([
      expect.objectContaining({
        bundle: expect.objectContaining({ id: bundleId, version }),
        runtime: expect.objectContaining({ state: "enabled", health: "healthy" }),
      }),
    ]));
  }
  for (const endpoint of [
    "/api/v1/web/workbench/views/server-administrator.fleet/data",
    "/api/v1/web/workbench/views/server-administrator.servers/data",
    "/api/v1/web/workbench/knowledge/spaces",
    "/api/v1/web/workbench/approvals/pending",
  ]) {
    const response = await page.request.get(endpoint);
    expect(response.ok(), `${endpoint} returned ${response.status()}`).toBe(true);
  }
  const targetsResponse = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(targetsResponse.ok()).toBe(true);
  const inventory = await targetsResponse.json() as {
    targets: Array<{ target_id: string; label: string }>;
  };
  expect(inventory.targets.length).toBeGreaterThan(0);
  for (const target of inventory.targets) {
    expect(target.label).not.toMatch(/(^r\d)|fixture/i);
  }
  const knowledgeCenter = await findKnowledgeGraphCenter(page);

  await page.goto("/web/workspace?id=server-administrator.fleet");
  await expect(page.getByRole("heading", { name: "Overview", exact: true })).toBeVisible();
  const fleetTabs = page.getByTestId("workspace-tabs");
  for (const label of ["Overview", "Servers", "Incidents", "Metrics", "Topology"]) {
    await expect(fleetTabs.getByRole("link", { name: label, exact: true })).toBeVisible();
  }
  await expect(page.getByText("Clusters", { exact: true })).toBeVisible();
  await expect(page.getByText("Active Servers", { exact: true })).toBeVisible();
  await expect(page.getByText("Open Incidents", { exact: true })).toBeVisible();
  await expect(page.getByText("Cooling", { exact: true })).toHaveCount(0);
  await expectNoHorizontalOverflow(page);

  await page.goto("/web/workspace?id=server-administrator.servers");
  await expect(page.getByRole("heading", { name: "Servers", exact: true })).toBeVisible();
  await expect(page.getByText("Workspace unavailable", { exact: true })).toHaveCount(0);
  await expect(page.getByText("Workspace data unavailable", { exact: true })).toHaveCount(0);
  await expect(page.getByText("SSH registry unavailable", { exact: true })).toHaveCount(0);
  await expectNoHorizontalOverflow(page);
  for (const target of inventory.targets) {
    for (const match of await page.getByText(target.target_id, { exact: true }).all()) {
      await expect(match).toBeHidden();
    }
  }

  await page.getByTestId("penny-companion-launcher").click();
  const companion = page.getByTestId("penny-companion");
  await expect(companion).toBeVisible();
  await expect(companion).toContainText("Servers");
  await page.getByRole("button", { name: "Maximize Penny" }).click();
  await expect(page).toHaveURL(/\/web\/workspace\?id=server-administrator\.servers$/);
  await expectAccessible(page, "[data-testid='penny-companion']");
  await expectReadableTextControls(page, "[data-testid='penny-companion']");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Maximize Penny" })).toBeFocused();
  await page.getByRole("button", { name: "Minimize Penny" }).click();

  await page.setViewportSize({ width: 320, height: 720 });

  await page.goto(
    "/web/knowledge?" + new URLSearchParams({
      workspace: "graph",
      space: knowledgeCenter.spaceId,
      bundle: knowledgeCenter.bundleId,
      center: `note:${knowledgeCenter.objectId}`,
    }).toString(),
  );
  const knowledgeTabs = page.getByTestId("knowledge-workspace-tabs");
  await expect(knowledgeTabs).toBeVisible();
  await expect(knowledgeTabs.getByRole("group", { name: "Understand" })).toBeVisible();
  await expect(page.getByLabel("Knowledge Space")).toHaveValue(knowledgeCenter.spaceId);
  await expect(page.getByLabel("Knowledge Domain")).toHaveValue(knowledgeCenter.bundleId);
  const graphNode = page.getByTestId(`graph-node-note:${knowledgeCenter.objectId}`);
  await expect(graphNode).toBeVisible();
  await graphNode.click();
  const graphInspector = page.getByRole("complementary", { name: "Graph inspector" });
  await expect(graphInspector).toContainText(knowledgeCenter.title);
  await graphInspector.getByRole("button", { name: "Ask Penny" }).click();
  await expect(page.getByTestId("penny-companion")).toContainText(knowledgeCenter.title);
  await page.getByRole("button", { name: "Minimize Penny" }).click();
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page);
  await expectReadableTextControls(page);

  await page.goto("/web/review");
  await expect(page.getByTestId("review-page-header")).toContainText("Review Center");
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page);
  await expectReadableTextControls(page);

  await page.goto("/web/workspace?id=server-administrator.servers");
  await expect(page.getByRole("heading", { name: "Servers", exact: true })).toBeVisible();
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page);
  await expectReadableTextControls(page);

  await page.goto("/web/workspace?id=server-administrator.raw-telemetry");
  await expect(page.getByRole("heading", { name: "Raw telemetry", exact: true })).toBeVisible();
  await expect(page.getByRole("table")).toContainText("cpu.util");
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page);
  await expectReadableTextControls(page);

  await page.goto("/web/admin");
  await page.getByRole("tab", { name: "Bundles", exact: true }).click();
  await expect(page.getByText(/Installed Bundles · \d+/)).toBeVisible();
  const serverBundle = page.getByRole("button", { name: /server-administrator/ });
  await expect(serverBundle).toBeVisible();
  await expect(page.getByRole("button", { name: /server-operations-intelligence/ })).toBeVisible();
  await serverBundle.click();
  await expect(page.getByText(`Version ${expectedServerVersion} · Functions provided by runtime`)).toBeVisible();
  await expect(page.getByText("0 actions · 0 views", { exact: true })).toHaveCount(0);
  await expectNoHorizontalOverflow(page);
  await expectAccessible(page);
  await expectReadableTextControls(page);
});

test("keeps one actual Penny job across companion and screen changes", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_4_LIVE=1 for the release-built 18085 service");
  test.setTimeout(240_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");

  const conversationId = randomUUID();
  const replyMarker = `R4.4-CONTEXT-${conversationId.slice(0, 8)}`;
  await page.addInitScript((id) => {
    sessionStorage.setItem("gadgetron_conversation_id", id);
    localStorage.setItem("gadgetron_conversation_id", id);
  }, conversationId);
  await login(page);

  let jobId = "";
  try {
    const profileResponse = await page.request.patch(
      `/api/v1/web/workbench/conversations/${conversationId}/agent-profile`,
      {
        data: {
          backend: "codex_exec",
          model: "",
          effort: "low",
          model_source: "default",
          local_base_url: "",
          local_api_key_env: "",
        },
      },
    );
    if (!profileResponse.ok()) {
      throw new Error(
        `Codex conversation profile returned ${profileResponse.status()}: ${await profileResponse.text()}`,
      );
    }
    const savedProfile = await profileResponse.json() as {
      profile: { backend: string; model: string; effort: string };
    };
    expect(savedProfile.profile).toMatchObject({
      backend: "codex_exec",
      model: "",
      effort: "low",
    });
    await page.evaluate(
      ({ id, profile }) => {
        sessionStorage.setItem(
          `gadgetron_agent_profile:${id}`,
          JSON.stringify(profile),
        );
      },
      { id: conversationId, profile: savedProfile.profile },
    );

    await page.goto("/web/workspace?id=server-administrator.servers");
    await expect(page.getByRole("heading", { name: "Servers", exact: true })).toBeVisible();
    await page.getByTestId("penny-companion-launcher").click();

    const companion = page.getByTestId("penny-companion");
    await expect(companion).toBeVisible();
    await expect(companion).toContainText("Servers");
    const workspaceContext = page.getByRole("button", {
      name: "Remove Servers from current screen context",
      exact: true,
    });
    await expect(workspaceContext).toBeVisible();

    const composer = page.getByPlaceholder("Ask Penny about this screen");
    await composer.fill(`Return this marker verbatim in your first line: ${replyMarker}`);
    await page.getByRole("button", { name: "Send", exact: true }).click();
    const userMessage = companion.getByTestId("penny-user-message").last();
    await expect(userMessage).toContainText(replyMarker);
    await expect(userMessage).toContainText("Servers");

    await expect.poll(async () => {
      const response = await page.request.get(
        `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
      );
      if (!response.ok()) return "";
      const snapshot = await response.json() as { job_id?: string };
      jobId = snapshot.job_id ?? "";
      return jobId;
    }, { timeout: 30_000 }).not.toBe("");

    const before = await companion.boundingBox();
    const moveHandle = await page.getByRole("button", { name: "Move Penny companion" }).boundingBox();
    expect(before).not.toBeNull();
    expect(moveHandle).not.toBeNull();
    await page.mouse.move(moveHandle!.x + 24, moveHandle!.y + 20);
    await page.mouse.down();
    await page.mouse.move(moveHandle!.x - 8, moveHandle!.y + 4);
    await page.mouse.up();
    const moved = await companion.boundingBox();
    expect(moved).not.toBeNull();
    expect(Math.round(moved!.x)).toBe(Math.round(before!.x) - 32);
    expect(Math.round(moved!.y)).toBe(Math.round(before!.y) - 16);

    const resizeHandle = await page.getByRole("button", { name: "Resize Penny companion" }).boundingBox();
    expect(resizeHandle).not.toBeNull();
    await page.mouse.move(resizeHandle!.x + 12, resizeHandle!.y + 12);
    await page.mouse.down();
    await page.mouse.move(resizeHandle!.x + 44, resizeHandle!.y + 36);
    await page.mouse.up();
    const resized = await companion.boundingBox();
    expect(resized).not.toBeNull();
    expect(resized!.width).toBeGreaterThan(moved!.width);
    expect(resized!.height).toBeGreaterThan(moved!.height);

    await page.getByRole("button", { name: "Minimize Penny" }).click();
    const launcher = page.getByTestId("penny-companion-launcher");
    await expect(launcher).toContainText(/Working|New response/);
    await page.getByTestId("nav-tab-wiki").click();
    await expect(page).toHaveURL(/\/web\/knowledge/);

    const afterSwitch = await page.request.get(
      `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
    );
    expect(afterSwitch.ok()).toBe(true);
    expect(await afterSwitch.json()).toMatchObject({ job_id: jobId });
    await expect.poll(async () => {
      const response = await page.request.get(
        `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
      );
      if (!response.ok()) return `HTTP ${response.status()}`;
      return (await response.json() as { status: string }).status;
    }, { timeout: 180_000 }).toBe("complete");
    await expect(launcher).toContainText("New response");

    await launcher.click();
    await expect(companion).toContainText("Knowledge");
    await expect(companion.getByTestId("penny-assistant-message").last()).toContainText(replyMarker);
  } finally {
    if (jobId) {
      const active = await page.request.get(
        `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
      );
      if (active.ok()) {
        const snapshot = await active.json() as { is_finished?: boolean };
        if (!snapshot.is_finished) {
          await page.request.post(`/api/v1/web/workbench/jobs/${jobId}/cancel`);
        }
      }
    }
    await page.request.delete(`/api/v1/web/workbench/conversations/${conversationId}`);
  }
});

test("reviews and withdraws one actual Server request on mobile", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R4_4_LIVE=1 for the release-built 18085 service");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_R4_4_PASSWORD is required").not.toBe("");
  await login(page);

  const actionId = "server-administrator.logs.action.loganalysis.finding-dismiss";
  let approvalId = "";
  let restoreDocument: PolicyDocument | null = null;
  let reviewPolicyRevision = 0;
  try {
    const policyResponse = await page.request.get("/api/v1/web/workbench/admin/policy");
    if (!policyResponse.ok()) {
      throw new Error(`Policy read returned ${policyResponse.status()}: ${await policyResponse.text()}`);
    }
    const activePolicy = await policyResponse.json() as {
      policy: { revision: number; document: PolicyDocument };
    };
    restoreDocument = {
      ...activePolicy.policy.document,
      rules: activePolicy.policy.document.rules.filter((rule) => rule.id !== reviewPolicyRuleId),
    };
    const reviewDocument: PolicyDocument = {
      ...restoreDocument,
      rules: [{
        id: reviewPolicyRuleId,
        priority: 0,
        enabled: true,
        match: { action_ids: [actionId] },
        decision: "review",
        reason: "Verify the Manager Review journey for this exact Server request",
      }, ...restoreDocument.rules],
    };
    const policyUpdate = await page.request.post(
      "/api/v1/web/workbench/admin/policy/revisions",
      { data: { expected_revision: activePolicy.policy.revision, document: reviewDocument } },
    );
    if (!policyUpdate.ok()) {
      throw new Error(`Review policy returned ${policyUpdate.status()}: ${await policyUpdate.text()}`);
    }
    reviewPolicyRevision = (await policyUpdate.json() as {
      policy: { revision: number };
    }).policy.revision;

    const logsResponse = await page.request.get(
      "/api/v1/web/workbench/views/server-administrator.logs/data",
    );
    if (!logsResponse.ok()) {
      throw new Error(`Server logs returned ${logsResponse.status()}: ${await logsResponse.text()}`);
    }
    const logs = await logsResponse.json() as {
      payload: { rows: Array<{ finding_id?: string }> };
    };
    const findingId = logs.payload.rows.find((row) => row.finding_id)?.finding_id;
    expect(findingId, "R4.4 requires one actual Server finding for Review").toBeTruthy();

    const request = await page.request.post(
      `/api/v1/web/workbench/actions/${actionId}`,
      { data: { args: { finding_id: findingId } } },
    );
    if (!request.ok()) {
      throw new Error(`Review request returned ${request.status()}: ${await request.text()}`);
    }
    const result = await request.json() as {
      result: { status: string; approval_id?: string };
    };
    expect(result.result.status).toBe("pending_approval");
    approvalId = result.result.approval_id ?? "";
    expect(approvalId).not.toBe("");

    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto("/web/review?tab=exceptions");
    const row = page.getByTestId(`approval-row-${approvalId}`);
    await expect(row).toBeVisible();
    await row.click();

    const detail = page.getByTestId("approval-detail");
    await expect(detail).toBeVisible();
    await expect(detail).toContainText("Finding triage · server-administrator");
    await expect(detail).toContainText("loganalysis.finding-dismiss");
    await expect(detail.getByTestId("approval-arguments")).toContainText(findingId!);
    await expectAccessible(page);
    await expectReadableTextControls(page);

    await detail.getByRole("button", { name: /Cancel my request|Reject request/ }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByPlaceholder("What should change before this can proceed?").fill(
      "Request withdrawn after reviewing the target and requested change.",
    );
    await dialog.getByRole("button", { name: /Cancel my request|Reject request/ }).click();
    await expect(row).toHaveCount(0);
  } finally {
    if (approvalId) {
      await page.request.post(`/api/v1/web/workbench/approvals/${approvalId}/deny`, {
        data: { reason: "Request withdrawn before execution." },
      });
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
