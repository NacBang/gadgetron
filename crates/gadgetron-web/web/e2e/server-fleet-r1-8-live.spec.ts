import { expect, test, type APIRequestContext } from "@playwright/test";

const live = process.env.GADGETRON_R1_8_LIVE === "1";
const email = process.env.GADGETRON_R1_8_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_8_PASSWORD ?? "";
const bundleId = "server-administrator";
const targetId = "r1-failure-probe";

async function checkedJson<T>(response: Awaited<ReturnType<APIRequestContext["get"]>>): Promise<T> {
  if (!response.ok()) throw new Error(`HTTP ${response.status()}: ${await response.text()}`);
  return response.json() as Promise<T>;
}

async function targetRows(request: APIRequestContext) {
  const body = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
    await request.get("/api/v1/web/workbench/views/server-administrator.servers/data"),
  );
  return body.payload.rows;
}

async function firingAlerts(request: APIRequestContext): Promise<number> {
  const body = await checkedJson<{ payload: { firing: number } }>(
    await request.get(
      "/api/v1/web/workbench/contributions/server-administrator.alerts-summary/data",
    ),
  );
  return body.payload.firing;
}

async function runFailedCycle(request: APIRequestContext): Promise<void> {
  let accepted: { job_id: string } | null = null;
  for (let attempt = 0; attempt < 30 && !accepted; attempt += 1) {
    const response = await request.post(
      `/api/v1/web/workbench/admin/bundles/${bundleId}/job-recipes/server-duty-cycle/start`,
      { data: { parameters: { target_id: targetId } } },
    );
    if (response.status() === 409) {
      await new Promise((resolve) => setTimeout(resolve, 500));
      continue;
    }
    accepted = await checkedJson<{ job_id: string }>(response);
  }
  expect(accepted, "target remained busy beyond its bounded cycle").not.toBeNull();
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const report = await checkedJson<{ status: string }>(
      await request.get(
        `/api/v1/web/workbench/admin/bundles/${bundleId}/jobs/${accepted!.job_id}`,
      ),
    );
    if (["succeeded", "failed", "cancelled"].includes(report.status)) {
      expect(report.status).toBe("failed");
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("failed target cycle did not reach a terminal state");
}

test("registers, monitors, alerts, retires, and preserves the Server workspace", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R1_8_LIVE=1 for the signed 18085 fleet fixture");
  test.setTimeout(90_000);
  expect(password, "GADGETRON_R1_8_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
  await expect(page.getByTestId("version-badge")).toContainText("0.6.8");

  const request = page.request;
  const inventory = await checkedJson<{
    targets: Array<{
      target_id: string;
      host_key: { algorithm: string; public_key_base64: string };
      secret_id: string;
      secret_resource: string;
    }>;
  }>(await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`));
  const fixture = inventory.targets.find((target) => target.target_id === "r1-fixture");
  expect(fixture, "the actual OpenSSH fixture must remain registered").toBeTruthy();
  const baselineAlerts = await firingAlerts(request);

  const targetBody = {
    label: "R1.8 failure probe",
    address: "127.0.0.1",
    port: 22999,
    username: "jungho",
    host_key_algorithm: fixture!.host_key.algorithm,
    host_public_key_base64: fixture!.host_key.public_key_base64,
    secret_id: fixture!.secret_id,
    secret_resource: fixture!.secret_resource,
    allowed_operations: ["inventory", "telemetry", "topology", "log-scan"],
    address_policy: {
      allow_private: true,
      allow_loopback: true,
      allow_link_local: false,
    },
  };

  try {
    await checkedJson(
      await request.put(
        `/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets/${targetId}`,
        { data: targetBody },
      ),
    );

    await page.goto("/web/workspace?id=server-administrator.servers");
    await expect(page.getByRole("heading", { name: "Server registration" })).toBeVisible();
    await expect(page.getByText("R1.8 failure probe", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Edit R1.8 failure probe" }).click();
    await expect(page.getByLabel("Stable target ID")).toBeDisabled();
    await page.getByLabel("Display label").fill("R1.8 failure probe revised");
    await page.getByRole("button", { name: "Save new revision" }).click();
    await expect(page.getByText("R1.8 failure probe revised", { exact: true })).toBeVisible();

    for (let attempt = 0; attempt < 3; attempt += 1) {
      const row = (await targetRows(request)).find((item) => item.target_id === targetId);
      if (Number(row?.consecutive_failures ?? 0) >= 3) break;
      await runFailedCycle(request);
    }
    await expect.poll(async () => {
      const row = (await targetRows(request)).find((item) => item.target_id === targetId);
      return { status: row?.health_status, failures: row?.consecutive_failures };
    }).toEqual({ status: "unreachable", failures: 3 });
    await expect.poll(() => firingAlerts(request)).toBe(baselineAlerts + 1);

    await page.getByRole("button", { name: "Remove R1.8 failure probe revised" }).click();
    await page.getByTestId("confirm-accept").click();
    await expect(page.getByText("R1.8 failure probe revised", { exact: true })).toHaveCount(0);
    await expect.poll(async () => {
      const targets = await checkedJson<{ targets: Array<{ target_id: string }> }>(
        await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`),
      );
      return targets.targets.some((target) => target.target_id === targetId);
    }).toBe(false);
    await expect.poll(() => firingAlerts(request)).toBe(baselineAlerts);
    expect((await targetRows(request)).some((row) => row.target_id === targetId)).toBe(false);
  } finally {
    const targets = await checkedJson<{ targets: Array<{ target_id: string }> }>(
      await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`),
    );
    if (targets.targets.some((target) => target.target_id === targetId)) {
      await request.delete(
        `/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets/${targetId}`,
      );
    }
  }
});
