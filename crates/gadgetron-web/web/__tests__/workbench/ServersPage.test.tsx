import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import ServersPage from "../../app/(shell)/servers/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
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

describe("ServersPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("keeps host actions below the title so long aliases can use the full card width", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return jsonResponse(
          actionPayload({
            hosts: [
              {
                id: "host-1",
                host: "10.100.1.110",
                ssh_user: "root",
                ssh_port: 22,
                created_at: "2026-05-03T10:00:00Z",
                last_ok_at: null,
                alias: "dg5R-PRO6000-8",
                machine_id: null,
                cpu_model: "AMD EPYC 7763 64-Core Processor",
                cpu_cores: 64,
                gpus: ["NVIDIA RTX PRO 6000 Blackwell Server Edition"],
              },
            ],
          }),
        );
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      if (url.includes("/workbench/actions/server-stats")) {
        return jsonResponse(
          actionPayload({
            cpu: null,
            mem: null,
            disks: [],
            temps: [],
            gpus: [],
            power: null,
            network: [],
            uptime_secs: null,
            fetched_at: "2026-05-03T10:00:00Z",
            warnings: [],
          }),
        );
      }
      if (url.includes("/workbench/servers/host-1/metrics")) {
        return jsonResponse({
          metric: "cpu.util",
          unit: null,
          resolution: "auto",
          points: [],
          refresh_lag_seconds: 0,
          dropped_frames: 0,
        });
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    const titleRow = await screen.findByTestId("host-card-title-row");
    const actions = screen.getByTestId("host-card-actions-host-1");

    expect(titleRow.textContent).toContain("dg5R-PRO6000-8");
    expect(titleRow.parentElement).toHaveClass("flex-col");
    expect(actions).toHaveClass("justify-end");
    expect(actions.textContent).toContain("detail");
    expect(actions.compareDocumentPosition(titleRow)).toBe(
      Node.DOCUMENT_POSITION_PRECEDING,
    );
    await waitFor(() => {
      expect(global.fetch).toHaveBeenCalled();
    });
  });

  it("shows server inventory errors as shared notices with hidden details", async () => {
    global.fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/actions/server-list")) {
        return {
          ok: false,
          status: 401,
          text: () => Promise.resolve("raw invalid api key"),
        } as Response;
      }
      if (url.includes("/workbench/actions/loganalysis-list")) {
        return jsonResponse(actionPayload({ findings: [] }));
      }
      throw new Error(`unexpected fetch: ${url}`);
    });

    render(<ServersPage />);

    expect(await screen.findByTestId("servers-header")).toBeTruthy();
    expect(
      await screen.findByText("Server inventory request failed"),
    ).toBeTruthy();
    expect(screen.queryByText(/raw invalid api key/i)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Details" }));

    expect(screen.getByText(/raw invalid api key/i)).toBeTruthy();
  });
});
