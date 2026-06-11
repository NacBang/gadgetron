import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import DashboardPage from "../../app/(shell)/dashboard/page";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
  }),
}));

class MockWebSocket {
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;

  close() {
    this.onclose?.();
  }
}

const FLEET = {
  generated_at: "2026-06-11T00:00:00Z",
  servers: { total: 2, online: 1, offline: 1 },
  gpus: { count: 4, avg_util_pct: 50, max_temp_c: 70, total_power_w: 400 },
  cpu: { avg_util_pct: 40 },
  mem: { used_bytes: 1024 ** 3, total_bytes: 4 * 1024 ** 3 },
  warnings: 1,
  hosts: [
    {
      id: "h1",
      host: "10.0.0.5",
      alias: "node01",
      online: true,
      cpu_util_pct: 40,
      gpu_count: 2,
      gpu_avg_util_pct: 50,
      gpu_max_temp_c: 70,
      warnings: 1,
    },
    {
      id: "h2",
      host: "10.0.0.6",
      alias: null,
      online: false,
      cpu_util_pct: null,
      gpu_count: 2,
      gpu_avg_util_pct: null,
      gpu_max_temp_c: null,
      warnings: 0,
    },
  ],
};

describe("DashboardPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  it("renders the shared workbench frame and hides raw summary errors", async () => {
    global.fetch = vi.fn(async () => {
      return {
        ok: false,
        status: 503,
        text: () => Promise.resolve("upstream refused the summary request"),
      } as Response;
    });

    render(<DashboardPage />);

    expect(await screen.findByTestId("dashboard-header")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Dashboard" })).toBeTruthy();
    expect(screen.getByText("Fleet status and live feed")).toBeTruthy();
    expect(screen.getByText("Live feed disconnected")).toBeTruthy();

    await waitFor(() => {
      expect(screen.getByText("Fleet summary request failed")).toBeTruthy();
    });
    expect(screen.queryByText(/upstream refused/i)).toBeNull();
  });

  it("renders fleet tiles and one status dot per host", async () => {
    global.fetch = vi.fn(async () => {
      return {
        ok: true,
        status: 200,
        json: () =>
          Promise.resolve({
            result: {
              payload: [{ type: "text", text: JSON.stringify(FLEET) }],
            },
          }),
      } as Response;
    });

    render(<DashboardPage />);

    expect(await screen.findByTestId("dashboard-tiles")).toBeTruthy();
    expect(screen.getByTestId("tile-servers")).toHaveTextContent("1/2");
    expect(screen.getByTestId("tile-gpus")).toHaveTextContent("4");
    expect(screen.getByTestId("tile-gpus")).toHaveTextContent("70°C");
    expect(screen.getByTestId("tile-resources")).toHaveTextContent("40%");
    expect(screen.getByTestId("tile-warnings")).toHaveTextContent("1");
    expect(screen.getAllByTestId("dashboard-host-dot")).toHaveLength(2);
    expect(screen.getByTestId("dashboard-window-label")).toHaveTextContent(
      "1/2 online",
    );
  });
});
