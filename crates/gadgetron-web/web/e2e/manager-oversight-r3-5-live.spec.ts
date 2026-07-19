import { createServer, type IncomingHttpHeaders } from "node:http";

import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_R3_5_LIVE === "1";
const email = process.env.GADGETRON_R3_5_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R3_5_PASSWORD ?? "";

interface Delivery {
  headers: IncomingHttpHeaders;
  body: Record<string, unknown>;
}

test("runs the Manager directive and terminal-exception webhook journey", async ({
  page,
}) => {
  test.setTimeout(90_000);
  test.skip(!live, "set GADGETRON_R3_5_LIVE=1 for the 18085 Manager fixture");
  expect(password, "GADGETRON_R3_5_PASSWORD is required").not.toBe("");
  const runId = Date.now().toString(36);
  const escalationSummary = `The bounded correction needs Manager intervention (${runId})`;

  const deliveries: Delivery[] = [];
  const receiver = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      deliveries.push({
        headers: request.headers,
        body: JSON.parse(Buffer.concat(chunks).toString("utf8")) as Record<
          string,
          unknown
        >,
      });
      response.writeHead(204).end();
    });
  });
  await new Promise<void>((resolve) =>
    receiver.listen(0, "127.0.0.1", resolve),
  );
  const address = receiver.address();
  if (!address || typeof address === "string")
    throw new Error("receiver did not bind");
  const webhookUrl = `http://127.0.0.1:${address.port}/manager`;

  try {
    await page.goto("/web/login");
    await page.getByPlaceholder("you@example.com").fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.getByRole("button", { name: "Sign in", exact: true }).click();
    await expect(page).toHaveURL(/\/web\/?$/);

    await page.goto("/web/review");
    await expect(page.getByTestId("review-page-header")).toContainText(
      "Review Center",
    );
    const exceptionsTab = page.getByRole("tab", { name: /Exceptions/ });
    await exceptionsTab.click();
    await expect(exceptionsTab).toHaveAttribute("aria-selected", "true");
    const disable = page.getByRole("button", { name: "Disable", exact: true });
    if (await disable.isVisible()) await disable.click();
    await page.getByLabel("Manager webhook").fill(webhookUrl);
    await page.getByRole("button", { name: "Enable", exact: true }).click();
    await expect(page.getByText(/Enabled · 127\.0\.0\.1/)).toBeVisible();

    await page.getByRole("tab", { name: /Directives/ }).click();
    await page.getByRole("button", { name: "Issue directive" }).first().click();
    const composer = page.getByRole("dialog");
    await composer
      .getByLabel("Directive target", { exact: true })
      .fill(`r3-5-browser-target-${runId}`);
    await composer
      .getByLabel("Directive instruction")
      .fill(
        "Correct the browser journey while preserving its original outcome",
      );
    await composer
      .getByLabel("Directive desired outcome")
      .fill("The browser journey is verified with durable evidence");
    await composer
      .getByRole("button", { name: "Issue directive", exact: true })
      .click();
    await expect(
      page
        .getByText("The browser journey is verified with durable evidence")
        .first(),
    ).toBeVisible();

    const advance = async (
      openLabel: string,
      actionLabel: string,
      stageLabel?: string,
      stageValue?: string,
    ) => {
      await page.getByRole("button", { name: openLabel, exact: true }).click();
      const dialog = page.getByRole("dialog");
      await dialog
        .getByLabel("Directive transition summary")
        .fill(`${actionLabel} from the actual browser journey`);
      if (stageLabel && stageValue)
        await dialog.getByLabel(stageLabel).fill(stageValue);
      await dialog
        .getByRole("button", { name: actionLabel, exact: true })
        .click();
    };

    await advance("Acknowledge directive", "Acknowledge directive");
    await advance(
      "Record plan",
      "Record plan",
      "Plan",
      "Run the bounded correction and compare the result",
    );
    await advance("Start execution", "Start execution");
    await advance(
      "Start verification",
      "Start verification",
      "Execution result",
      "The bounded correction completed",
    );
    await page
      .getByRole("button", { name: "Resolve with evidence", exact: true })
      .click();
    const resolution = page.getByRole("dialog");
    await resolution
      .getByLabel("Directive transition summary")
      .fill("The desired state is verified");
    await resolution
      .getByLabel("Verification result")
      .fill("The actual browser observed the expected state");
    await resolution
      .getByLabel("Directive evidence references")
      .fill("browser:r3-5-manager-journey");
    await resolution
      .getByRole("button", { name: "Resolve with evidence", exact: true })
      .click();
    await expect(page.getByText("Resolved").first()).toBeVisible();

    await page.getByRole("button", { name: "Issue directive" }).first().click();
    const escalationComposer = page.getByRole("dialog");
    await escalationComposer
      .getByLabel("Directive target", { exact: true })
      .fill(`r3-5-terminal-exception-${runId}`);
    await escalationComposer
      .getByLabel("Directive instruction")
      .fill("Stop safely and ask the Manager for a corrected boundary");
    await escalationComposer
      .getByLabel("Directive desired outcome")
      .fill("The exception is visible and delivered without sensitive fields");
    await escalationComposer
      .getByRole("button", { name: "Issue directive", exact: true })
      .click();
    await page.getByRole("button", { name: "Escalate", exact: true }).click();
    const escalation = page.getByRole("dialog");
    await escalation
      .getByLabel("Directive transition summary")
      .fill(escalationSummary);
    await escalation
      .getByRole("button", { name: "Escalate to manager", exact: true })
      .click();

    await expect
      .poll(() => deliveries.length, { timeout: 15_000 })
      .toBeGreaterThan(0);
    const delivery = deliveries.at(-1)!;
    expect(Object.keys(delivery.body).sort()).toEqual([
      "event_id",
      "occurred_at",
      "review_url",
      "severity",
      "summary",
    ]);
    expect(delivery.headers["idempotency-key"]).toMatch(/^manager-exception:/);
    expect(delivery.body.review_url).toMatch(
      /\/web\/review\?tab=exceptions&id=/,
    );

    await page.goto(String(delivery.body.review_url));
    await expect(exceptionsTab).toHaveAttribute("aria-selected", "true");
    await expect(
      page.getByText("Terminal exceptions", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText(escalationSummary, { exact: true }),
    ).toBeVisible();
    await expect(page.getByText(/Sent · 1 attempt/)).toBeVisible({
      timeout: 20_000,
    });
    await page.screenshot({
      path: "../../../.gadgetron/r3-5-manager-review.png",
      fullPage: true,
    });
    const exceptionRow = page
      .getByText(escalationSummary, { exact: true })
      .locator("xpath=ancestor::div[contains(@class, 'lg:grid-cols')][1]");
    await exceptionRow
      .getByRole("button", { name: "Resolve", exact: true })
      .click();
    await expect(
      page.getByText(escalationSummary, { exact: true }),
    ).toBeHidden();
    await page.getByRole("button", { name: "Disable", exact: true }).click();
  } finally {
    try {
      const exceptionsResponse = await page.request.get(
        "/api/v1/web/workbench/admin/exceptions",
      );
      if (exceptionsResponse.ok()) {
        const payload = (await exceptionsResponse.json()) as {
          exceptions: Array<{
            id: string;
            revision: number;
            state: string;
            summary: string;
          }>;
        };
        for (const exception of payload.exceptions) {
          if (
            exception.summary === escalationSummary &&
            exception.state !== "resolved"
          ) {
            await page.request.post(
              `/api/v1/web/workbench/admin/exceptions/${exception.id}/transition`,
              {
                data: {
                  expected_revision: exception.revision,
                  state: "resolved",
                  summary: "Closed by the completed R3.5 browser journey",
                },
              },
            );
          }
        }
      }
      const webhookResponse = await page.request.get(
        "/api/v1/web/workbench/admin/exception-webhook",
      );
      if (webhookResponse.ok()) {
        const settings = (await webhookResponse.json()) as {
          enabled: boolean;
          revision: number;
        };
        if (settings.enabled) {
          await page.request.patch(
            "/api/v1/web/workbench/admin/exception-webhook",
            {
              data: {
                enabled: false,
                destination_url: null,
                review_base_url: null,
                expected_revision: settings.revision,
              },
            },
          );
        }
      }
    } catch {
      // Preserve the original browser failure when cleanup cannot authenticate.
    }
    await new Promise<void>((resolve, reject) =>
      receiver.close((error) => (error ? reject(error) : resolve())),
    );
  }
});
