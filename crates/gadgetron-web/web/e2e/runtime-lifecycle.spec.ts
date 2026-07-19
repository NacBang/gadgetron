import { randomUUID } from "node:crypto";
import { expect, test } from "@playwright/test";

const liveEnabled = process.env.GADGETRON_LIVE_RUNTIME_E2E === "1";
const adminPassword = process.env.GADGETRON_ADMIN_PW ?? "";
const localHost = process.env.GADGETRON_LOCAL_LLM_HOST ?? "127.0.0.1";
const localPort = Number(process.env.GADGETRON_LOCAL_LLM_PORT ?? "11434");
const localModel = process.env.GADGETRON_LOCAL_LLM_MODEL ?? "qwen3:1.7b";

test.describe("live agent runtime lifecycle", () => {
  test.skip(
    !liveEnabled,
    "set GADGETRON_LIVE_RUNTIME_E2E=1 to run against the development service",
  );

  test("route switch, replay, and browser cancel preserve one background job", async ({
    page,
  }) => {
    test.setTimeout(240_000);
    expect(adminPassword, "GADGETRON_ADMIN_PW is required").not.toBe("");

    const conversationId = randomUUID();
    let endpointId = "";
    let jobId = "";

    const login = await page.request.post("/api/v1/auth/login", {
      data: { email: "admin@example.com", password: adminPassword },
    });
    expect(login.ok(), await login.text()).toBeTruthy();

    try {
      const detectedResponse = await page.request.post(
        "/api/v1/web/workbench/admin/llm/endpoints/autodetect",
        {
          data: {
            host: localHost,
            port: localPort,
            alias: `r0-browser-${conversationId.slice(0, 8)}`,
            model_id: localModel,
          },
        },
      );
      expect(detectedResponse.ok(), await detectedResponse.text()).toBeTruthy();
      const detected = await detectedResponse.json();
      endpointId = detected.endpoint?.id ?? "";
      expect(detected).toMatchObject({
        ok: true,
        endpoint: {
          protocol: "openai_responses",
          runtime_compatibility: "codex_exec",
          tool_status: "passed",
          tool_model_id: localModel,
        },
      });
      expect(endpointId).not.toBe("");
      console.log("[runtime-lifecycle] endpoint ready");

      const profile = {
        backend: "codex_exec",
        model: localModel,
        effort: "low",
        model_source: "local",
        llm_endpoint_id: endpointId,
        local_base_url: "",
        local_api_key_env: "",
      };
      const profileResponse = await page.request.patch(
        `/api/v1/web/workbench/conversations/${conversationId}/agent-profile`,
        { data: profile },
      );
      expect(profileResponse.ok(), await profileResponse.text()).toBeTruthy();

      await page.addInitScript(
        ({ id, cachedProfile }) => {
          sessionStorage.setItem("gadgetron_conversation_id", id);
          localStorage.setItem("gadgetron_conversation_id", id);
          sessionStorage.setItem(
            `gadgetron_agent_profile:${id}`,
            JSON.stringify(cachedProfile),
          );
        },
        { id: conversationId, cachedProfile: profile },
      );

      await page.goto("/web");
      await expect(page.getByTestId("chat-column")).toBeVisible();
      console.log("[runtime-lifecycle] chat ready");

      const composer = page.locator("textarea, [role='textbox']").first();
      await composer.fill(
        "Use the wiki.list Gadget exactly once, then summarize briefly. /no_think",
      );
      await composer.press("Enter");
      await expect
        .poll(
          async () => {
            const response = await page.request.get(
              `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
            );
            if (!response.ok()) return "";
            const snapshot = await response.json();
            if (snapshot.status !== "streaming") return "";
            jobId = snapshot.job_id ?? "";
            return jobId;
          },
          { timeout: 30_000 },
        )
        .not.toBe("");
      console.log("[runtime-lifecycle] job streaming", jobId);
      await expect(
        page.getByRole("button", { name: "Stop generation" }),
      ).toBeVisible();
      console.log("[runtime-lifecycle] stop visible");

      await page.getByTestId("nav-tab-dashboard").click();
      await page.waitForURL(/\/dashboard/);
      console.log("[runtime-lifecycle] dashboard route");
      const backgroundResponse = await page.request.get(
        `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
      );
      expect(backgroundResponse.ok()).toBeTruthy();
      expect(await backgroundResponse.json()).toMatchObject({
        job_id: jobId,
        status: "streaming",
        is_finished: false,
      });
      console.log("[runtime-lifecycle] background job preserved");

      await page.getByTestId("nav-tab-chat").click();
      await page.waitForURL(/\/web\/?$/);
      console.log("[runtime-lifecycle] chat route restored");
      const replay = page.waitForRequest(
        (request) =>
          request.method() === "GET" &&
          request.url().includes(`/workbench/jobs/${jobId}/sync?since=0`),
      );
      await page.reload();
      await replay;
      console.log("[runtime-lifecycle] replay requested");
      const replayedJob = await page.request.get(
        `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
      );
      expect(replayedJob.ok()).toBeTruthy();
      expect(await replayedJob.json()).toMatchObject({
        job_id: jobId,
        status: "streaming",
        is_finished: false,
      });
      await expect(
        page.getByRole("button", { name: "Stop generation" }),
      ).toBeVisible();
      console.log("[runtime-lifecycle] stop visible after reload");

      const cancelled = page.waitForResponse(
        (response) =>
          response.request().method() === "POST" &&
          response.url().endsWith(`/workbench/jobs/${jobId}/cancel`),
      );
      await page.getByRole("button", { name: "Stop generation" }).click();
      expect((await cancelled).ok()).toBeTruthy();
      console.log("[runtime-lifecycle] browser cancel posted");

      await expect
        .poll(async () => {
          const response = await page.request.get(
            `/api/v1/web/workbench/conversations/${conversationId}/active-job`,
          );
          if (!response.ok()) return `HTTP ${response.status()}`;
          return (await response.json()).status;
        })
        .toBe("cancelled");
    } finally {
      if (jobId) {
        await page.request.post(`/api/v1/web/workbench/jobs/${jobId}/cancel`);
      }
      await page.request.delete(
        `/api/v1/web/workbench/conversations/${conversationId}`,
      );
      if (endpointId) {
        await page.request.delete(
          `/api/v1/web/workbench/admin/llm/endpoints/${endpointId}`,
        );
      }
    }
  });
});
