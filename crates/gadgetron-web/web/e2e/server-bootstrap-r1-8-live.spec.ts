import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { expect, test, type APIRequestContext, type Page } from "@playwright/test";

const live = process.env.GADGETRON_R1_8_BOOTSTRAP_LIVE === "1";
const email = process.env.GADGETRON_R1_8_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_8_PASSWORD ?? "";
const sshPassword = process.env.GADGETRON_R1_8_SSH_PASSWORD ?? "";
const sshUser = process.env.GADGETRON_R1_8_SSH_USER ?? process.env.USER ?? "";
const sshAddress = process.env.GADGETRON_R1_8_SSH_ADDRESS ?? "";
const sshPort = Number.parseInt(process.env.GADGETRON_R1_8_SSH_PORT ?? "22", 10);
const bundleId = "server-administrator";

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

test("bootstraps a real server from IP, ID and password, verifies monitoring, then removes the generated key", async ({ page }) => {
  test.skip(!live, "set GADGETRON_R1_8_BOOTSTRAP_LIVE=1 for the destructive local bootstrap fixture");
  test.setTimeout(20 * 60_000);
  expect(password, "GADGETRON_R1_8_PASSWORD is required").not.toBe("");
  expect(sshPassword, "GADGETRON_R1_8_SSH_PASSWORD is required").not.toBe("");
  expect(sshUser, "GADGETRON_R1_8_SSH_USER is required").not.toBe("");
  expect(sshAddress, "GADGETRON_R1_8_SSH_ADDRESS is required").not.toBe("");

  await login(page);
  const request = page.request;
  let targetId: string | null = null;
  try {
    await page.goto(`/web/workspace?id=${bundleId}.servers`);
    if (sshPort === 22) {
      await page.getByLabel("IP address or DNS").fill(sshAddress);
      await page.getByLabel("SSH ID").fill(sshUser);
      await page.getByLabel("Password").fill(sshPassword);
      await page.getByRole("button", { name: "Set up & register" }).click();
      await expect(page.getByText(/registered$/)).toBeVisible({ timeout: 15 * 60_000 });
      await expect(page.getByText(/Verified initial inventory, telemetry, topology and log collection/)).toBeVisible();
      await expect(page.getByLabel("Password")).toHaveValue("");
    } else {
      const result = await checkedJson<{
        target: { target_id: string };
        first_collection_verified: boolean;
        stages: Array<{ id: string }>;
      }>(await request.post(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`, {
        data: {
          address: sshAddress,
          port: sshPort,
          username: sshUser,
          password: sshPassword,
          address_policy: {
            allow_private: true,
            allow_loopback: sshAddress === "127.0.0.1" || sshAddress === "::1",
            allow_link_local: false,
          },
        },
        timeout: 15 * 60_000,
      }));
      expect(result.first_collection_verified).toBe(true);
      expect(result.stages.map((stage) => stage.id)).toContain("first-collection");
      targetId = result.target.target_id;
      await page.reload();
    }

    const targets = await checkedJson<{
      targets: Array<{
        target_id: string;
        address: string;
        lifecycle_state: string;
        credential_origin: string;
      }>;
    }>(await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`));
    const target = targets.targets.find((item) => item.address === sshAddress && item.credential_origin === "bootstrap");
    expect(target).toBeTruthy();
    expect(target!.lifecycle_state).toBe("active");
    expect(targetId ?? target!.target_id).toBe(target!.target_id);
    targetId = target!.target_id;

    const view = await checkedJson<{ payload: { rows: Array<Record<string, unknown>> } }>(
      await request.get(`/api/v1/web/workbench/views/${bundleId}.servers/data`),
    );
    const row = view.payload.rows.find((item) => item.target_id === targetId);
    expect(row?.health_status).toBe("healthy");

    const secrets = await checkedJson<{ secrets: Array<{ secret_id: string }> }>(
      await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/secrets`),
    );
    expect(secrets.secrets.some((item) => item.secret_id === targetId)).toBe(true);

    await page.getByTestId(`ssh-target-${targetId}`).getByRole("button", { name: /Remove/ }).click();
    await page.getByTestId("confirm-accept").click();
    await expect.poll(async () => {
      const current = await checkedJson<{ targets: Array<{ target_id: string }> }>(
        await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets`),
      );
      return current.targets.some((item) => item.target_id === targetId);
    }).toBe(false);

    const currentSecrets = await checkedJson<{ secrets: Array<{ secret_id: string }> }>(
      await request.get(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/secrets`),
    );
    expect(currentSecrets.secrets.some((item) => item.secret_id === targetId)).toBe(false);
    const authorizedKeys = path.join(os.homedir(), ".ssh", "authorized_keys");
    if (fs.existsSync(authorizedKeys)) {
      expect(fs.readFileSync(authorizedKeys, "utf8")).not.toContain(`gadgetron-monitor:${targetId}`);
    }
    targetId = null;
  } finally {
    if (targetId) {
      await request.delete(`/api/v1/web/workbench/admin/bundles/${bundleId}/ssh/targets/${targetId}`);
    }
  }
});
