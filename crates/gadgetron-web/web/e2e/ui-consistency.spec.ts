import { expect, test } from "@playwright/test";

const routes = [
  "/web",
  "/web/wiki",
  "/web/dashboard",
  "/web/servers",
  "/web/findings",
  "/web/admin",
];

const viewports = [
  { width: 1440, height: 900, name: "desktop" },
  { width: 900, height: 768, name: "narrow-desktop" },
];

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("gadgetron_api_key", "gad_live_test_key");
  });

  await page.route("**/health", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ status: "ok", degraded_reasons: [] }),
    });
  });

  await page.route("**/models", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        object: "list",
        data: [{ id: "penny", object: "model" }],
      }),
    });
  });

  await page.route("**/workbench/**", async (route) => {
    const url = route.request().url();
    if (url.includes("/workbench/usage/summary")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          window_hours: 24,
          chat: {
            requests: 0,
            errors: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_cents: 0,
            avg_latency_ms: 0,
          },
          actions: {
            total: 0,
            success: 0,
            error: 0,
            pending_approval: 0,
            avg_elapsed_ms: 0,
          },
          tools: { total: 0, errors: 0 },
        }),
      });
      return;
    }

    if (url.includes("/workbench/admin/users")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ users: [], returned: 0 }),
      });
      return;
    }

    if (url.includes("/workbench/admin/agent/brain")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          mode: "claude_max",
          external_base_url: "",
          model: "",
          external_auth_token_env: "",
          custom_model_option: false,
          source: "defaults",
        }),
      });
      return;
    }

    if (url.includes("/workbench/admin/llm/endpoints")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ endpoints: [] }),
      });
      return;
    }

    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        result: {
          payload: {
            pages: [],
            hosts: [],
            findings: [],
            endpoints: [],
            users: [],
          },
        },
      }),
    });
  });
});

for (const viewport of viewports) {
  test.describe(`UI consistency ${viewport.name}`, () => {
    test.use({ viewport });

    for (const route of routes) {
      test(`${route} renders shared shell without horizontal clipping`, async ({
        page,
      }) => {
        await page.goto(route);
        await expect(page.getByTestId("workbench-shell")).toBeVisible();
        await expect(page.getByTestId("chat-column")).toBeVisible();

        const bodyBox = await page.locator("body").boundingBox();
        const shellBox = await page.getByTestId("workbench-shell").boundingBox();
        expect(bodyBox).not.toBeNull();
        expect(shellBox).not.toBeNull();
        expect(Math.ceil(shellBox!.width)).toBeLessThanOrEqual(
          Math.ceil(bodyBox!.width),
        );

        const horizontalOverflow = await page.evaluate(() => {
          return (
            document.documentElement.scrollWidth >
            document.documentElement.clientWidth + 1
          );
        });
        expect(horizontalOverflow).toBe(false);
      });
    }
  });
}
