import { expect, test } from "@playwright/test";

const live = process.env.GADGETRON_K0_CHAT_ATTACHMENT_LIVE === "1";
const email = process.env.GADGETRON_R1_4_EMAIL ?? "admin@example.com";
const password = process.env.GADGETRON_R1_4_PASSWORD ?? process.env.GADGETRON_ADMIN_PW ?? "";

test("attaches a chat-only Source and receives a locator-citing answer", async ({ page }) => {
  test.skip(!live, "set GADGETRON_K0_CHAT_ATTACHMENT_LIVE=1 for the 18085 Penny journey");
  test.setTimeout(120_000);
  expect(password, "GADGETRON_ADMIN_PW or GADGETRON_R1_4_PASSWORD is required").not.toBe("");

  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);

  let conversationId: string | null = null;
  try {
    const attachmentTray = page.getByTestId("chat-attachment-tray");
    await attachmentTray.getByRole("button", { name: "Attach", exact: true }).click();
    await expect(attachmentTray.getByRole("button", { name: "This chat only" })).toHaveClass(
      /bg-zinc-700/,
    );
    const attachment = Buffer.from(
      "# K0.3 browser fixture\n\nThe bounded verification code is COPPER-LOCATOR-731.\n",
    );
    const uploadResponse = page.waitForResponse((response) =>
      response.request().method() === "POST" && response.url().includes("/attachments/upload"),
    );
    await attachmentTray.locator('input[type="file"]').setInputFiles({
      name: "k03-browser-fixture.md",
      mimeType: "text/markdown",
      buffer: attachment,
    });
    const upload = await uploadResponse;
    expect(upload.ok(), await upload.text()).toBeTruthy();
    const uploadBody = await upload.json() as { object: { path: string } };
    const expectedLocator = uploadBody.object.path;
    await expect(page.getByText("Citation-ready", { exact: true })).toBeVisible();

    await page.getByPlaceholder("Ask Penny").fill(
      "Read the attached source. Reply with its verification code and cite the exact notes/ locator pinned to this chat.",
    );
    await page.getByRole("button", { name: "Send" }).click();
    const answer = page.getByTestId("penny-assistant-message").last();
    await expect(answer).toContainText("COPPER-LOCATOR-731", { timeout: 90_000 });
    await expect(answer).toContainText(expectedLocator);
    await page.screenshot({
      path: "../../../.gadgetron/k03-chat-attachment-locator.png",
      fullPage: true,
    });
  } finally {
    conversationId = await page.evaluate(() =>
      window.sessionStorage.getItem("gadgetron_conversation_id"),
    );
    if (conversationId) {
      const purge = await page.request.delete(
        `/api/v1/web/workbench/knowledge/conversations/${conversationId}/attachments`,
      );
      expect(purge.ok(), await purge.text()).toBeTruthy();
    }
  }
  expect(conversationId).toBeTruthy();
});
