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
    expect(screen.getByText("Live feed and usage summary")).toBeTruthy();
    expect(screen.getByText("Live feed disconnected")).toBeTruthy();

    await waitFor(() => {
      expect(screen.getByText("Usage summary request failed")).toBeTruthy();
    });
    expect(screen.queryByText(/upstream refused/i)).toBeNull();
  });
});
