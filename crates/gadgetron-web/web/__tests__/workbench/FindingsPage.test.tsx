import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import FindingsPage from "../../app/(shell)/findings/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
    identity: {
      user_id: "11111111-1111-1111-1111-111111111111",
      role: "admin",
      display_name: "Local Admin",
      email: "admin@example.local",
    },
  }),
}));

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(JSON.stringify(body)),
  } as Response;
}

function actionPayload(payload: unknown): unknown {
  return {
    result: {
      status: "ok",
      payload: [{ type: "text", text: JSON.stringify(payload) }],
    },
  };
}

describe("FindingsPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("shows a never-scanned empty state instead of claiming there are no findings", async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ count: 0, findings: [] }));
      }
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [{ id: "host-1", host: "10.100.1.110", alias: "dg5R-PRO6000-8" }],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-status")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                host_id: "host-1",
                last_scanned_at: null,
                interval_secs: 120,
                enabled: true,
              },
            ],
          }),
        );
      }
      throw new Error(`unexpected fetch: ${url}`);
    });
    global.fetch = fetchMock;

    render(<FindingsPage />);

    await waitFor(() => {
      expect(screen.getByText(/No log scans have run yet/i)).toBeTruthy();
    });
  });
});
