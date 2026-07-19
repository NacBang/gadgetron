import { expect, test, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_4_LIVE === "1";
const pennyLive = process.env.GADGETRON_R1_8_PENNY_LIVE === "1";
const pennyBackend = process.env.GADGETRON_R1_8_BACKEND ?? "codex_exec";
const email = process.env.GADGETRON_R1_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_4_PASSWORD ?? "";
const expectedVersion = process.env.GADGETRON_R1_4_VERSION ?? "0.8.5";

async function firstMetricLabel(page: Page): Promise<string> {
  const response = await page.request.get(
    "/api/v1/web/workbench/views/server-administrator.metrics/data",
  );
  expect(response.ok()).toBe(true);
  const body = await response.json() as {
    payload: { rows: Array<{ target_label?: string; hostname?: string }> };
  };
  const first = body.payload.rows[0];
  const label = first?.target_label ?? first?.hostname ?? "";
  expect(label).not.toBe("");
  expect(label).not.toMatch(/(^r\d)|fixture/i);
  return label;
}

async function firstRegisteredServerLabel(page: Page): Promise<string> {
  const response = await page.request.get(
    "/api/v1/web/workbench/admin/bundles/server-administrator/ssh/targets",
  );
  expect(response.ok()).toBe(true);
  const body = await response.json() as {
    targets: Array<{ label?: string }>;
  };
  const label = body.targets[0]?.label ?? "";
  expect(label).not.toBe("");
  expect(label).not.toMatch(/(^r\d)|fixture/i);
  return label;
}

test.describe("Server Administrator live product projection", () => {
  test.skip(!live, "set GADGETRON_R1_4_LIVE=1 for the signed 18085 fixture");

  test("human telemetry stays in Monitoring while raw metrics stay in Diagnostics", async ({ page }) => {
    test.setTimeout(45_000);
    expect(password, "GADGETRON_R1_4_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    const serverLabel = await firstMetricLabel(page);

    await expect(page.getByText("Monitoring", { exact: true })).toBeVisible();
    await expect(page.getByText("Diagnostics", { exact: true })).toBeVisible();

    await page.getByTestId("nav-workspace-server-administrator-fleet").click();
    await page.getByTestId("workspace-tabs").getByRole("link", { name: "Metrics" }).click();
    await expect(page.getByRole("heading", { name: "Metrics" })).toBeVisible();
    await expect(page.getByText(/server\.metric-catalog/)).toHaveCount(0);
    await expect(page.getByText(/telemetry · rev/)).toHaveCount(0);
    await expect(page.getByRole("heading", { name: serverLabel })).toBeVisible();
    await expect(page.getByRole("progressbar").first()).toBeVisible();
    await expect(page.getByRole("img", { name: /temperature:/i }).first()).toBeVisible();
    await expect(page.getByText("cpu.util", { exact: true })).toHaveCount(0);
    await page.screenshot({ path: "../../../.gadgetron/r1-8-human-telemetry.png", fullPage: true });

    await page.goto("/web/workspace?id=server-administrator.raw-telemetry");
    await expect(page.getByRole("heading", { name: "Raw telemetry" })).toBeVisible();
    await expect(page.getByRole("table")).toContainText("cpu.util");
  });

  test("Servers, Logs, Dashboard, and Penny context share one live Bundle", async ({ page }) => {
    test.setTimeout(60_000);
    expect(password, "GADGETRON_R1_4_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    await expect(page.getByTestId("version-badge")).toContainText(expectedVersion);
    const registeredLabel = await firstRegisteredServerLabel(page);
    const metricLabel = await firstMetricLabel(page);

    const fleet = page.getByTestId("nav-workspace-server-administrator-fleet");
    const logs = page.getByTestId("nav-workspace-server-administrator-logs");
    await expect(fleet).toBeVisible();
    await expect(logs).toBeVisible();

    await fleet.click();
    await page.getByTestId("workspace-tabs").getByRole("link", { name: "Servers" }).click();
    await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
    await expect(page.getByText(registeredLabel, { exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Ask Penny for row 1", exact: true }).click();
    await expect(page.getByTestId("penny-companion")).toContainText(metricLabel);
    await page.getByRole("button", { name: "Minimize Penny", exact: true }).click();

    await page.getByTestId("nav-tab-dashboard").click();
    await expect(page.getByText("Server fleet", { exact: true })).toBeVisible();
    await expect(page.getByText("Server alerts", { exact: true })).toBeVisible();

    await logs.click();
    await expect(page.getByRole("heading", { name: "Logs" })).toBeVisible();
    await expect(page.getByText("Service or device failure detected", { exact: true }).first()).toBeVisible();
    await page.getByRole("button", { name: "Ask Penny for row 1", exact: true }).click();
    await expect(page.getByTestId("penny-companion")).toContainText(/Operational warning|Service or device failure/);

    const projection = await page.evaluate(async () => {
      const response = await fetch("/api/v1/web/workbench/capabilities", { credentials: "include" });
      return response.json() as Promise<{ ui_contributions: Array<{ kind: string; gadget_name?: string }> }>;
    });
    expect(projection.ui_contributions).toEqual(expect.arrayContaining([
      expect.objectContaining({ kind: "tool_result", gadget_name: "loganalysis.findings-list" }),
      expect.objectContaining({ kind: "job_presentation" }),
      expect.objectContaining({ kind: "review_presentation", gadget_name: "loganalysis.finding-dismiss" }),
    ]));
  });

  test("selected server drives one audited agent diagnosis turn", async ({
    page,
  }) => {
    test.skip(
      !pennyLive,
      "set GADGETRON_R1_8_PENNY_LIVE=1 to exercise the logged-in Codex runtime",
    );
    test.setTimeout(240_000);
    expect(password, "GADGETRON_R1_4_PASSWORD is required").not.toBe("");

    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);
    const serverLabel = await firstMetricLabel(page);

    await page
      .getByTestId("nav-workspace-server-administrator-fleet")
      .click();
    await page.getByTestId("workspace-tabs").getByRole("link", { name: "Servers" }).click();
    await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
    await page.getByRole("button", { name: "Ask Penny for row 1", exact: true }).click();
    await expect(page.getByTestId("penny-companion")).toContainText(
      serverLabel,
    );

    const pinned = await page.evaluate(() => {
      const conversationId =
        sessionStorage.getItem("gadgetron_conversation_id") ?? "";
      const raw = localStorage.getItem(`gadgetron_subject_${conversationId}`);
      const subject = raw
        ? (JSON.parse(raw) as { id?: string; title?: string })
        : {};
      return {
        conversationId,
        targetId: subject.id ?? "",
        title: subject.title ?? "",
      };
    });
    expect(pinned.conversationId).not.toBe("");
    expect(pinned.targetId).not.toBe("");
    expect(pinned.title).toBe(serverLabel);

    try {
      const profile = await page.request.patch(
        `/api/v1/web/workbench/conversations/${pinned.conversationId}/agent-profile`,
        {
          data: {
            backend: pennyBackend,
            model: "",
            effort: "low",
            model_source: "default",
            local_base_url: "",
            local_api_key_env: "",
          },
        },
      );
      expect(profile.ok(), await profile.text()).toBeTruthy();
      const savedProfile = (await profile.json()) as { profile: unknown };
      await page.evaluate(
        ({ conversationId, value }) => {
          sessionStorage.setItem(
            `gadgetron_agent_profile:${conversationId}`,
            JSON.stringify(value),
          );
        },
        { conversationId: pinned.conversationId, value: savedProfile.profile },
      );

      const composer = page.locator("textarea, [role='textbox']").first();
      await composer.fill(
        `Before answering, you MUST call server.subject-context exactly once with target_id "${pinned.targetId}". ` +
          "Do not use the pinned Facts as a substitute for the tool call. Then return one short JSON object " +
          "using only that tool output with keys target_id, health, firing_alerts, and open_findings. " +
          "Do not call or propose any write action.",
      );
      await composer.press("Enter");

      await expect
        .poll(
          async () => {
            const response = await page.request.get(
              `/api/v1/web/workbench/conversations/${pinned.conversationId}/active-job`,
            );
            if (!response.ok()) return `HTTP ${response.status()}`;
            return ((await response.json()) as { status: string }).status;
          },
          { timeout: 180_000, intervals: [1_000, 2_000, 5_000] },
        )
        .toBe("complete");

      const transcript = await page.request.get(
        `/api/v1/web/workbench/conversations/${pinned.conversationId}/messages`,
      );
      expect(transcript.ok(), await transcript.text()).toBeTruthy();
      const history = (await transcript.json()) as {
        messages: Array<{ role: string; content: string }>;
      };
      const user =
        history.messages.find((message) => message.role === "user")?.content ??
        "";
      const assistant =
        history.messages.find((message) => message.role === "assistant")
          ?.content ?? "";
      expect(user).toContain(`Subject: ${serverLabel}`);
      expect(user).toContain(`target_id \"${pinned.targetId}\"`);
      expect(assistant).toMatch(/healthy|정상/i);
      expect(assistant).toMatch(/firing_alerts|alert|알림/i);
      expect(assistant).toMatch(/open_findings|finding|발견/i);

      await expect
        .poll(
          async () => {
            const response = await page.request.get(
              "/api/v1/web/workbench/audit/tool-events?tool_name=server.subject-context&limit=100",
            );
            if (!response.ok()) return [];
            const body = (await response.json()) as {
              events: Array<{
                conversation_id: string | null;
                outcome: string;
                tier: string;
              }>;
            };
            return body.events.filter(
              (event) => event.conversation_id === pinned.conversationId,
            );
          },
          { timeout: 15_000, intervals: [250, 500, 1_000] },
        )
        .toEqual([
          expect.objectContaining({ outcome: "success", tier: "read" }),
        ]);
    } finally {
      await page.request.delete(
        `/api/v1/web/workbench/conversations/${pinned.conversationId}`,
      );
    }
  });
});
