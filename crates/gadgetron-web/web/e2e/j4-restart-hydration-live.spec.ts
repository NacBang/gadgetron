import { expect, test, type Page } from "@playwright/test";


const live = process.env.GADGETRON_J4_BROWSER_LIVE === "1";
const conversationId = process.env.GADGETRON_J4_CONVERSATION_ID ?? "";
const originalMarker = process.env.GADGETRON_J4_ORIGINAL_MARKER ?? "";
const expectedVersion = process.env.GADGETRON_J4_EXPECTED_VERSION ?? "";
const email = process.env.GADGETRON_J4_EMAIL ?? "admin@example.com";
const apiKeyEnv = process.env.GADGETRON_J4_API_KEY_ENV ?? "GADGETRON_RUNTIME_VERIFY_API_KEY";
const passwordEnv = process.env.GADGETRON_J4_PASSWORD_ENV ?? "GADGETRON_ADMIN_PW";
const apiKey = process.env[apiKeyEnv] ?? "";
const password = process.env[passwordEnv] ?? "";
const restartNotice =
  "Generation stopped because Gadgetron restarted. Your request and completed " +
  "messages were preserved; send it again to continue.";

async function authenticate(page: Page) {
  if (apiKey) {
    await page.addInitScript((key) => {
      localStorage.setItem("gadgetron_api_key", key);
    }, apiKey);
    return;
  }
  expect(password, `${passwordEnv} is required when no API key is configured`).not.toBe("");
  await page.goto("/web/login");
  await page.getByPlaceholder("you@example.com").fill(email);
  await page.locator('input[type="password"]').fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await expect(page).toHaveURL(/\/web\/?$/);
}

test("hydrates one interrupted conversation after the Core process restart", async ({ page }) => {
  test.skip(!live, "the J4 process verifier owns this live browser journey");
  test.setTimeout(60_000);
  expect(conversationId, "GADGETRON_J4_CONVERSATION_ID is required").not.toBe("");
  expect(originalMarker, "GADGETRON_J4_ORIGINAL_MARKER is required").not.toBe("");
  expect(expectedVersion, "GADGETRON_J4_EXPECTED_VERSION is required").not.toBe("");

  await page.addInitScript((id) => {
    sessionStorage.setItem("gadgetron_conversation_id", id);
    localStorage.setItem("gadgetron_conversation_id", id);
  }, conversationId);
  await authenticate(page);
  await page.goto("/web");

  await expect(page.getByTestId("version-badge")).toContainText(expectedVersion);
  const history = page.getByTestId("past-messages");
  await expect(history).toBeVisible();
  await expect(history).toContainText(originalMarker);
  await expect(history.getByText(restartNotice, { exact: true })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Stop generation" })).toHaveCount(0);
  const composer = page.getByPlaceholder("Ask Penny");
  await expect(composer).toBeEditable();
  await composer.fill("Retry is available");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
});
