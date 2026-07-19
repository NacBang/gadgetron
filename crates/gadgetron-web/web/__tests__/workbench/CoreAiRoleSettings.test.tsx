import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CoreAiRoleSettings } from "../../app/components/admin/core-ai-role-settings";

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

beforeEach(() => {
  global.fetch = vi.fn(async (input: RequestInfo | URL) => {
    const url = String(input);
    if (url.endsWith("/workbench/admin/knowledge/ai-roles")) {
      return jsonResponse({
        global: {
          backend: "claude_code",
          model: "claude-sonnet-4-6",
          effort: "auto",
          model_source: "default",
        },
        roles: [],
      });
    }
    if (url.endsWith("/workbench/llm/endpoints/available")) {
      return jsonResponse({ models: [] });
    }
    throw new Error(`unexpected fetch: ${url}`);
  }) as typeof fetch;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("Core AI role settings", () => {
  it("treats an empty fixed-role response as a contract failure", async () => {
    render(<CoreAiRoleSettings apiKey={null} canCall />);

    expect(await screen.findByText(
      "AI roles failed to load. Reload the page; if this persists, report it.",
    )).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });
});
