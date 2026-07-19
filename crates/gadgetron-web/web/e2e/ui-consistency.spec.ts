import { expect, test } from "@playwright/test";

import {
  expectAccessible,
  expectReadableTextControls,
} from "./support/ui-assertions";

const routes = [
  "/web",
  "/web/knowledge",
  "/web/dashboard",
  "/web/review",
  "/web/admin",
  "/web/workspace?id=server-administrator.servers",
];

const accessibilityRoutes = [
  "/web/dashboard",
  "/web/knowledge",
  "/web/review",
  "/web/workspace?id=server-administrator.servers",
];
const fixtureCapabilityRevision = "1".repeat(64);

function knowledgeObjects(count: number) {
  return Array.from({ length: count }, (_, index) => {
    const suffix = index.toString().padStart(4, "0");
    return {
      id: `perf-${suffix}`,
      vault_id: "vault-performance",
      source_id: null,
      canonical_kind: "note",
      path: `notes/performance-${suffix}.md`,
      status: "active",
      revision: 1,
      created_at: "2026-07-14T00:00:00Z",
      updated_at: "2026-07-14T00:00:00Z",
      space_id: "ui-space",
      home_bundle_id: "server-administrator",
      owner_state: "enabled",
      title: `Performance note ${suffix}`,
    };
  });
}

function knowledgeGraph(nodeCount: number, edgeCount: number) {
  const nodes = knowledgeObjects(nodeCount).map((object) => ({
    stable_node_id: `note:${object.id}`,
    space_id: object.space_id,
    node_kind: "note",
    canonical_id: object.id,
    canonical_revision: object.revision,
    home_bundle_id: object.home_bundle_id,
    title: object.title,
    status: object.status,
    freshness: "current",
    metadata: {},
  }));
  const edges = Array.from({ length: edgeCount }, (_, index) => ({
    stable_edge_id: `relation-${index}`,
    from_node_id: nodes[index % nodeCount].stable_node_id,
    to_node_id: nodes[(index * 17 + 1) % nodeCount].stable_node_id,
    target_ref: nodes[(index * 17 + 1) % nodeCount].canonical_id,
    relation_kind: "related_to",
    source_space_id: "ui-space",
    target_space_id: "ui-space",
    home_bundle_id: "server-administrator",
    producer_kind: "system",
    producer_revision: 1,
    status: "active",
    evidence: {},
  }));
  return { nodes, edges, truncated: true };
}

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
    if (url.endsWith("/workbench/capabilities")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          revision: fixtureCapabilityRevision,
          bundles: [{ bundle_id: "server-administrator", bundle_version: "0.4.4" }],
          ui_contributions: [
            {
              id: "server-administrator.servers-navigation",
              owner_bundle: "server-administrator",
              kind: "navigation",
              label: "Servers",
              placement: "primary_navigation",
              order_hint: 10,
              icon: "fleet",
              navigation_section: "operations",
              required_scopes: ["management"],
              empty_state: "No servers are registered.",
              error_state: "Servers are unavailable.",
              workspace_id: "server-administrator.servers",
            },
            {
              id: "server-administrator.servers-main",
              owner_bundle: "server-administrator",
              kind: "workspace",
              label: "Servers",
              placement: "main",
              order_hint: 10,
              icon: "fleet",
              target_registry: "ssh",
              target_profile: {
                id: "server",
                label: "Server",
                default: true,
                allowed_operations: ["inventory", "telemetry", "topology", "log-scan"],
                setup_features: ["system_observation", "nvidia_dcgm"],
                bootstrap_input_schema: {
                  type: "object",
                  properties: {},
                  additionalProperties: false,
                },
              },
              required_scopes: ["management"],
              empty_state: "No servers are registered.",
              error_state: "Servers are unavailable.",
              workspace_id: "server-administrator.servers",
            },
          ],
          views: [
            {
              id: "server-administrator.servers",
              title: "Servers",
              owner_bundle: "server-administrator",
              source_kind: "bundle_gadget",
              source_id: "server.workspace",
              placement: "left_rail",
              renderer: "table",
              data_endpoint: "signed",
              refresh_seconds: 30,
              action_ids: [],
            },
          ],
          actions: [],
        }),
      });
      return;
    }

    if (url.endsWith("/workbench/bootstrap")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          gateway_version: "test",
          active_plugs: [],
          degraded_reasons: [],
          knowledge: {
            canonical_ready: true,
            search_ready: true,
            relation_ready: true,
          },
        }),
      });
      return;
    }

    if (url.endsWith("/workbench/jobs/active")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ jobs: [] }),
      });
      return;
    }

    if (url.endsWith("/workbench/approvals/pending")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ approvals: [], count: 0 }),
      });
      return;
    }

    if (url.endsWith("/workbench/knowledge/spaces")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          spaces: [{ id: "ui-space", title: "Operations", kind: "project" }],
        }),
      });
      return;
    }
    if (url.endsWith("/workbench/admin/bundles/server-administrator/ssh/targets")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          targets: [{
            target_id: "server-compute-01",
            target_revision: "revision-1",
            label: "Compute 01",
            address: "10.0.0.10",
            port: 22,
            username: "operator",
            approved_ips: ["10.0.0.10"],
            address_policy: {
              allow_private: true,
              allow_loopback: false,
              allow_link_local: false,
            },
            host_key: {
              algorithm: "ssh-ed25519",
              public_key_base64: "public-key",
              fingerprint: "SHA256:host-key",
            },
            secret_id: "compute-01-key",
            secret_resource: "secret:use:ssh-identity",
            allowed_operations: ["inventory", "telemetry", "topology", "log-scan"],
            target_profile_id: "server",
            lifecycle_state: "active",
            credential_origin: "bootstrap",
            acting_space_id: "ui-space",
            created_at_ms: 1,
            updated_at_ms: 1,
          }],
        }),
      });
      return;
    }
    if (url.endsWith("/workbench/admin/bundles/server-administrator/ssh/secrets")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ secrets: [] }),
      });
      return;
    }
    if (url.endsWith("/workbench/views/server-administrator.servers/data")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          capability_revision: fixtureCapabilityRevision,
          payload: {
            rows: [
              {
                status: "healthy",
                hostname: "compute-01",
                cpu_util_percent: 42,
                memory_used_percent: 58,
                disk_max_used_percent: 63,
              },
            ],
          },
        }),
      });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/vaults")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ vaults: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/sources")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ sources: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/objects")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ objects: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/duplicate-groups")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ groups: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/jobs")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ jobs: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/change-sets")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ change_sets: [] }) });
      return;
    }
    if (url.includes("/workbench/knowledge/spaces/ui-space/experience")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ exchanges: [], outcomes: [] }) });
      return;
    }

    if (url.includes("/workbench/admin/oversight")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ records: [] }) });
      return;
    }
    if (url.includes("/workbench/admin/directives")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ directives: [] }) });
      return;
    }
    if (url.includes("/workbench/admin/exceptions")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ exceptions: [] }) });
      return;
    }
    if (url.includes("/workbench/admin/exception-webhook/deliveries")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ deliveries: [] }) });
      return;
    }
    if (url.endsWith("/workbench/admin/exception-webhook")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          enabled: false,
          configured: false,
          destination_host: null,
          revision: 0,
          updated_at: null,
        }),
      });
      return;
    }
    if (url.includes("/workbench/admin/autonomy/goals")) {
      await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ goals: [] }) });
      return;
    }

    if (url.includes("/workbench/admin/knowledge/ai-roles")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          global: {
            backend: "codex_exec",
            model: "gpt-5.5",
            effort: "auto",
            model_source: "default",
          },
          roles: [],
        }),
      });
      return;
    }
    if (url.endsWith("/workbench/llm/endpoints/available")) {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ models: [] }),
      });
      return;
    }
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

for (const route of accessibilityRoutes) {
  test(`${route} has no automated WCAG A or AA violations`, async ({ page }) => {
    await page.goto(route);
    await expect(page.getByTestId("workbench-shell")).toBeVisible();
    if (route.includes("server-administrator.servers")) {
      await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
      await expect(page.getByRole("cell", { name: "compute-01", exact: true })).toBeVisible();
    }

    await expectAccessible(page);
    await expectReadableTextControls(page);
  });
}

for (const [legacy, expectedQuery] of [
  ["/web/wiki", null],
  ["/web/wiki?q=thermal%20runbook", "thermal runbook"],
  ["/web/wiki?page=ops%2Frecovery", "ops/recovery"],
] as const) {
  test(`legacy Wiki bookmark ${legacy} redirects into Knowledge`, async ({ page }) => {
    await page.goto(legacy);
    await expect.poll(() => new URL(page.url()).pathname).toBe("/web/knowledge");
    await expect.poll(() => new URL(page.url()).searchParams.get("q")).toBe(expectedQuery);
    await expect(page.getByLabel("Search knowledge")).toHaveValue(expectedQuery ?? "");
  });
}

test("a new tenant enters Knowledge through the composed Library", async ({ page }) => {
  await page.goto("/web/knowledge");

  const library = page.getByTestId("knowledge-library-landing");
  await expect(library.getByRole("heading", { name: "Library" })).toBeVisible();
  await expect(library.getByText("Your library is ready for its first material")).toBeVisible();
  await expect(page).not.toHaveURL(/workspace=/);
  await expect(page.getByLabel("Knowledge Space")).toHaveCount(0);
  await expect(page.getByLabel("Knowledge Domain")).toHaveCount(0);
  await expect(page.getByRole("navigation", { name: "Knowledge workspaces" })).toHaveCount(0);

  await page.getByRole("button", { name: "Show Knowledge tools" }).click();
  const navigation = page.getByRole("navigation", { name: "Knowledge workspaces" });
  await expect(navigation).toBeVisible();
  for (const name of ["Overview", "Materials", "Topics", "Knowledge", "Review"]) {
    await expect(navigation.getByRole("button", { name, exact: true })).toBeVisible();
  }
  for (const name of ["Cleanup", "Graph explorer", "Use & learn", "Automation"]) {
    await expect(navigation.getByRole("button", { name, exact: true })).toHaveCount(0);
  }
});

test("Knowledge search shows a server match from note body text", async ({ page }) => {
  await page.route("**/workbench/actions/knowledge-search", async (route) => {
    const request = route.request().postDataJSON() as { args?: { query?: string } };
    const hits = request.args?.query === "body-only safeguard"
      ? [{
          page_name: "notes/cooling-runbook.md",
          section: "Cooling Runbook",
          snippet: "Check the body-only safeguard before declaring recovery.",
          score: 0.91,
        }]
      : [];
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ result: { status: "ok", payload: { hits } } }),
    });
  });

  await page.goto("/web/knowledge");
  await page.getByLabel("Search knowledge").fill("body-only safeguard");

  const result = page.getByTestId("knowledge-full-text-result");
  await expect(result).toContainText("Cooling Runbook");
  await expect(result).toContainText("Check the body-only safeguard");
  await expect(result).toContainText("Full text");
});

test("open contextual Penny has no automated WCAG A or AA violations", async ({
  page,
}) => {
  await page.goto("/web/knowledge");
  await page.getByTestId("penny-companion-launcher").click();
  await expect(page.getByTestId("penny-companion")).toBeVisible();

  await expectAccessible(page, "[data-testid='penny-companion']");
  await expectReadableTextControls(page, "[data-testid='penny-companion']");
});

test("Server workspace keeps page-level reflow at 320px", async ({ page }) => {
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto("/web/workspace?id=server-administrator.servers");
  await expect(page.getByRole("heading", { name: "Servers" })).toBeVisible();
  await expect(page.getByRole("cell", { name: "compute-01", exact: true })).toBeVisible();

  const horizontalOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
  );
  expect(horizontalOverflow).toBe(false);
});

test("Knowledge filters and selects one note from 1,000 bounded candidates", async ({
  page,
}, testInfo) => {
  const objects = knowledgeObjects(1_000);
  await page.route("**/workbench/knowledge/spaces/ui-space/objects**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ objects }),
    });
  });
  await page.route("**/workbench/knowledge/objects/perf-0999/note", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        object_id: "perf-0999",
        source_id: null,
        revision: 1,
        content_hash: "a".repeat(64),
        git_revision: "performance-revision",
        frontmatter_format: "yaml",
        properties: { title: "Performance note 0999" },
        body: "# Performance note 0999",
        external_edit_reconciled: false,
      }),
    });
  });

  await page.goto("/web/knowledge?space=ui-space");
  const startedAt = Date.now();
  await page.getByLabel("Search Knowledge").fill("Performance note 0999");
  const result = page.getByRole("button", { name: /Performance note 0999/ });
  await expect(result).toHaveCount(1);
  await result.click();
  await expect(page.getByRole("heading", { name: "Performance note 0999", level: 2 })).toBeVisible();
  const elapsedMs = Date.now() - startedAt;
  testInfo.annotations.push({
    type: "performance",
    description: `1,000 candidate filter and select: ${elapsedMs} ms`,
  });

  await testInfo.attach("knowledge-1000-filter-select.json", {
    body: JSON.stringify({ candidates: objects.length, result_cap: 12, elapsed_ms: elapsedMs }),
    contentType: "application/json",
  });
});

test("Knowledge renders and selects within the 200 node and 500 relation cap", async ({
  page,
}, testInfo) => {
  const graph = knowledgeGraph(200, 500);
  await page.route("**/workbench/knowledge/spaces/ui-space/objects**", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ objects: knowledgeObjects(200) }),
    });
  });
  await page.route("**/workbench/knowledge/graph/neighborhood", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(graph),
    });
  });
  await page.route("**/workbench/knowledge/objects/**/shares", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ shares: [] }),
    });
  });

  const startedAt = Date.now();
  await page.goto(
    "/web/knowledge?workspace=graph&space=ui-space&center=note%3Aperf-0000",
  );
  await expect(
    page.getByRole("img", { name: "200 topology nodes and 500 relations" }),
  ).toBeVisible();
  await expect(page.getByText("Showing 200 nodes and 500 connections.")).toBeVisible();
  await page.getByTestId("graph-node-note:perf-0199").click();
  await expect(page.getByRole("heading", { name: "Performance note 0199" })).toBeVisible();
  const elapsedMs = Date.now() - startedAt;
  testInfo.annotations.push({
    type: "performance",
    description: `200 node and 500 relation first selection: ${elapsedMs} ms`,
  });

  await testInfo.attach("knowledge-graph-200-500.json", {
    body: JSON.stringify({ nodes: graph.nodes.length, edges: graph.edges.length, elapsed_ms: elapsedMs }),
    contentType: "application/json",
  });
});

test("contextual Penny stays on the current page and reuses one chat runtime", async ({
  page,
}) => {
  await page.goto("/web/knowledge");
  await page.getByTestId("penny-companion-launcher").click();

  const companion = page.getByTestId("penny-companion");
  await expect(companion).toBeVisible();
  await expect(companion).toContainText("Knowledge");
  await expect(companion).toContainText("Ask about Knowledge");

  const before = await companion.boundingBox();
  const moveHandle = await page
    .getByRole("button", { name: "Move Penny companion" })
    .boundingBox();
  expect(before).not.toBeNull();
  expect(moveHandle).not.toBeNull();
  await page.mouse.move(moveHandle!.x + 24, moveHandle!.y + 20);
  await page.mouse.down();
  await page.mouse.move(moveHandle!.x - 8, moveHandle!.y + 4);
  await page.mouse.up();
  const dragged = await companion.boundingBox();
  expect(dragged).not.toBeNull();
  expect(Math.round(dragged!.x)).toBe(Math.round(before!.x) - 32);
  expect(Math.round(dragged!.y)).toBe(Math.round(before!.y) - 16);

  await page.getByRole("button", { name: "Move Penny companion" }).press("ArrowLeft");
  const moved = await companion.boundingBox();
  expect(moved).not.toBeNull();
  expect(Math.round(moved!.x)).toBe(Math.round(before!.x) - 48);

  const resizeHandle = await page
    .getByRole("button", { name: "Resize Penny companion" })
    .boundingBox();
  expect(resizeHandle).not.toBeNull();
  await page.mouse.move(resizeHandle!.x + 12, resizeHandle!.y + 12);
  await page.mouse.down();
  await page.mouse.move(resizeHandle!.x + 44, resizeHandle!.y + 36);
  await page.mouse.up();
  const resized = await companion.boundingBox();
  expect(resized).not.toBeNull();
  expect(resized!.width).toBeGreaterThan(moved!.width);
  expect(resized!.height).toBeGreaterThan(moved!.height);
  await page.getByRole("button", { name: "Resize Penny companion" }).press("Home");
  await expect(companion).toHaveCSS("width", "360px");
  const stored = await companion.boundingBox();
  await page.waitForFunction(() => {
    const raw = localStorage.getItem("gadgetron.penny.companion.v1:api-key");
    return raw !== null && JSON.parse(raw).layout?.width === 360;
  });
  await page.reload();
  await expect(companion).toBeVisible();
  const restored = await companion.boundingBox();
  expect(restored).not.toBeNull();
  expect(Math.round(restored!.x)).toBe(Math.round(stored!.x));
  expect(Math.round(restored!.width)).toBe(360);
  await page.getByRole("button", { name: "Minimize Penny" }).click();
  await expect(page.getByTestId("penny-companion-launcher")).toBeVisible();

  await page.getByTestId("penny-companion-launcher").click();
  const draft = page.getByPlaceholder("Ask Penny about this screen");
  await draft.fill("Draft survives Penny window changes");
  const mediumBeforeMaximize = await companion.boundingBox();
  await page.getByRole("button", { name: "Maximize Penny" }).click();
  await expect(page).toHaveURL(/\/web\/knowledge/);
  await expect(companion).toContainText("Knowledge");
  await expect(draft).toHaveValue("Draft survives Penny window changes");
  const maximized = await companion.boundingBox();
  const viewport = page.viewportSize();
  expect(maximized).not.toBeNull();
  expect(viewport).not.toBeNull();
  expect(Math.round(maximized!.x)).toBe(0);
  expect(Math.round(maximized!.y)).toBe(0);
  expect(Math.round(maximized!.width)).toBe(viewport!.width);
  expect(Math.round(maximized!.height)).toBe(viewport!.height);
  await expectAccessible(page, "[data-testid='penny-companion']");
  await expectReadableTextControls(page, "[data-testid='penny-companion']");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("button", { name: "Maximize Penny" })).toBeFocused();
  await expect(draft).toHaveValue("Draft survives Penny window changes");
  const mediumAfterRestore = await companion.boundingBox();
  expect(Math.round(mediumAfterRestore!.x)).toBe(Math.round(mediumBeforeMaximize!.x));
  expect(Math.round(mediumAfterRestore!.width)).toBe(Math.round(mediumBeforeMaximize!.width));
});

test("Penny uses a bottom sheet on a small screen", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/web/knowledge");
  await page.getByTestId("penny-companion-launcher").click();

  const companion = page.getByTestId("penny-companion");
  await expect(companion).toBeVisible();
  await expect(companion).toHaveCSS("width", "390px");
  await expect(page.getByRole("button", { name: "Resize Penny companion" })).toHaveCount(0);
  await page.getByRole("button", { name: "Maximize Penny" }).click();
  await expect(companion).toHaveCSS("height", "844px");
  await expect(page.getByRole("button", { name: "Restore Penny window" })).toBeVisible();
  await page.getByRole("button", { name: "Restore Penny window" }).click();
  await expect.poll(async () => {
    const box = await companion.boundingBox();
    return box ? Math.round(box.height) : 0;
  }).toBe(Math.round(Math.min(844 * 0.76, 680)));
  await page.getByRole("button", { name: "Minimize Penny" }).click();
  await expect(page.getByTestId("penny-companion-launcher")).toBeFocused();
});

test("mobile shell exposes navigation and Inspector drawers without clipping", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto("/web/knowledge");
  await expect(page.getByTestId("left-rail")).toHaveCount(0);

  await page.getByRole("button", { name: "Open navigation" }).click();
  await expect(page.getByTestId("navigation-drawer")).toBeVisible();
  await expect(page.getByTestId("product-navigation-list")).toBeVisible();
  await page.getByTestId("nav-tab-dashboard").click();
  await expect(page).toHaveURL(/\/web\/dashboard$/);
  await expect(page.getByTestId("navigation-drawer")).toHaveCount(0);

  const inspectorTrigger = page.getByRole("button", { name: "Open inspector" });
  await inspectorTrigger.click();
  await expect(page.getByTestId("inspector-drawer")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("inspector-drawer")).toHaveCount(0);
  await expect(inspectorTrigger).toBeFocused();

  const horizontalOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
  );
  expect(horizontalOverflow).toBe(false);
});

test("Knowledge keeps grouped workspaces in the central frame on mobile", async ({
  page,
}) => {
  await page.setViewportSize({ width: 320, height: 720 });
  await page.goto("/web/knowledge");

  const tabs = page.getByTestId("knowledge-workspace-tabs");
  await expect(tabs).toBeVisible();
  for (const group of ["Collect", "Curate", "Understand", "Automate"]) {
    await expect(tabs.getByRole("group", { name: group })).toBeVisible();
  }
  expect(
    await tabs.evaluate((element) => element.scrollWidth > element.clientWidth),
  ).toBe(true);
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
    ),
  ).toBe(false);
});

test("Review switches between the exception inbox and detail on mobile", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.route("**/workbench/approvals/pending", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        approvals: [{
          id: "mobile-approval",
          action_id: "inspect-server",
          gadget_name: "server.inspect",
          args: { target: "Edge operations node", depth: 1 },
          requested_by_user_id: "manager-one",
          tenant_id: "tenant-one",
          state: "pending",
          created_at: "2026-07-14T04:00:00Z",
          context: {
            subject_title: "Edge operations node",
            reason: "Monitoring needs a bounded inspection.",
            risk: "medium",
          },
        }],
        count: 1,
      }),
    });
  });
  await page.goto("/web/review?tab=exceptions");

  const inbox = page.getByRole("region", { name: "Pending exceptions" });
  const detail = page.getByTestId("approval-detail");
  await expect(inbox).toBeVisible();
  await expect(detail).not.toBeVisible();
  await page.getByTestId("approval-row-mobile-approval").click();
  await expect(inbox).not.toBeVisible();
  await expect(detail).toBeVisible();
  await expect(detail).toContainText("What will run");
  await expectAccessible(page);
  await expectReadableTextControls(page);
  await page.getByRole("button", { name: "Back to requests" }).click();
  await expect(inbox).toBeVisible();
  await expect(detail).not.toBeVisible();

  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth > document.documentElement.clientWidth + 1,
    ),
  ).toBe(false);
});
