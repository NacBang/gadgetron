import { act, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import DashboardPage from "../../app/(shell)/dashboard/page";

const capabilityMock = vi.hoisted(() => ({
  widgets: [] as any[],
  fetchData: vi.fn(),
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: null }),
}));

vi.mock("../../app/lib/capability-context", () => ({
  useCapabilities: () => ({
    snapshot: { revision: "a".repeat(64), bundles: [], views: [], actions: [], ui_contributions: capabilityMock.widgets },
    status: "ready",
    error: null,
  }),
  fetchContributionData: capabilityMock.fetchData,
}));

class MockWebSocket {
  static instances: MockWebSocket[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  constructor() {
    MockWebSocket.instances.push(this);
  }
  close() {}
}

describe("Core DashboardPage", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    capabilityMock.widgets = [];
    capabilityMock.fetchData.mockReset();
    MockWebSocket.instances = [];
    vi.stubGlobal("WebSocket", MockWebSocket);
  });

  it("leads with mission status, attention and domain-neutral vitals", async () => {
    global.fetch = vi.fn(async (input) => {
      const url = String(input);
      const body = url.endsWith("/bootstrap")
        ? {
            gateway_version: "0.5.57",
            active_plugs: [{ id: "wiki", role: "canonical", healthy: true }],
            degraded_reasons: [],
            knowledge: {
              canonical_ready: true,
              search_ready: true,
              relation_ready: false,
            },
          }
        : url.endsWith("/jobs/active")
          ? { jobs: [{ id: "job-1" }] }
          : { count: 2 };
      return { ok: true, status: 200, json: async () => body } as Response;
    });

    render(<DashboardPage />);

    expect(await screen.findByTestId("dashboard-vitals")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "2 decisions need review" })).toBeTruthy();
    expect(screen.getByTestId("vital-core")).toHaveTextContent("Ready");
    expect(screen.getByTestId("vital-knowledge")).toHaveTextContent("2 / 3");
    expect(screen.getByTestId("vital-knowledge")).toHaveTextContent(
      "knowledge features available",
    );
    expect(screen.queryByText("ready planes")).toBeNull();
    expect(screen.getByTestId("vital-jobs")).toHaveTextContent("1");
    expect(screen.getByTestId("vital-review")).toHaveTextContent("2");
    expect(screen.getByRole("link", { name: /2 decisions are waiting.*Open Review/ })).toBeTruthy();
    expect(screen.getByText("No domain overview is enabled.")).toBeTruthy();
  });

  it("renders an isolated signed Bundle dashboard widget", async () => {
    capabilityMock.widgets = [{
      id: "travel.trip-summary",
      owner_bundle: "travel",
      kind: "dashboard_widget",
      label: "Upcoming trips",
      placement: "dashboard",
      order_hint: 2,
      icon: "calendar",
      required_scopes: [],
      empty_state: "No trips",
      error_state: "Trips unavailable",
      gadget_name: "travel.trip-summary",
      renderer: "dashboard",
      refresh_seconds: 30,
    }];
    capabilityMock.fetchData.mockResolvedValue({
      contribution_id: "travel.trip-summary",
      capability_revision: "a".repeat(64),
      payload: { upcoming: 2, next: "Seoul" },
    });
    global.fetch = vi.fn(async (input) => {
      const url = String(input);
      const body = url.endsWith("/bootstrap")
        ? { gateway_version: "0.5.62", active_plugs: [], degraded_reasons: [], knowledge: { canonical_ready: false, search_ready: false, relation_ready: false } }
        : url.endsWith("/jobs/active") ? { jobs: [] } : { count: 0 };
      return { ok: true, status: 200, json: async () => body } as Response;
    });
    render(<DashboardPage />);
    const widget = await screen.findByTestId("bundle-widget-travel.trip-summary");
    await waitFor(() => expect(widget).toHaveTextContent("Upcoming"));
    expect(widget).toHaveTextContent("Seoul");
    expect(widget).not.toHaveTextContent("travel");
  });

  it("guides an empty server tenant to Fleet and hides a complete truncation flag", async () => {
    capabilityMock.widgets = [
      {
        id: "server-administrator.servers-summary",
        owner_bundle: "server-administrator",
        kind: "dashboard_widget",
        label: "Server fleet",
        placement: "dashboard",
        order_hint: 100,
        icon: "dashboard",
        required_scopes: ["management"],
        empty_state: "No current server snapshots",
        error_state: "The server fleet summary is unavailable",
        gadget_name: "server.fleet-summary",
        renderer: "dashboard",
        refresh_seconds: 15,
      },
      {
        id: "server-administrator.cooling-summary",
        owner_bundle: "server-administrator",
        kind: "dashboard_widget",
        label: "Cooling safety",
        placement: "dashboard",
        order_hint: 109,
        icon: "activity",
        required_scopes: ["management"],
        empty_state: "No Gadgetini cooling observations",
        error_state: "Cooling safety summary is unavailable",
        gadget_name: "server.gadgetini-summary",
        renderer: "dashboard",
        refresh_seconds: 15,
      },
    ];
    capabilityMock.fetchData.mockImplementation(async (_apiKey, contributionId) => ({
      contribution_id: contributionId,
      capability_revision: "a".repeat(64),
      payload: contributionId.endsWith("servers-summary")
        ? { summary: { clusters: 0, servers: 0, active_servers: 0 } }
        : { observed: 0, attention: 0, incomplete: 0, truncated: false },
    }));
    global.fetch = vi.fn(async (input) => {
      const url = String(input);
      const body = url.endsWith("/bootstrap")
        ? { gateway_version: "0.8.21", active_plugs: [], degraded_reasons: [], knowledge: { canonical_ready: true, search_ready: true, relation_ready: true } }
        : url.endsWith("/jobs/active") ? { jobs: [] } : { count: 0 };
      return { ok: true, status: 200, json: async () => body } as Response;
    });

    render(<DashboardPage />);

    const fleet = await screen.findByTestId("bundle-widget-server-administrator.servers-summary");
    expect(await within(fleet).findByText("No servers connected yet")).toBeVisible();
    expect(within(fleet).getByText("Fleet status appears after the first server is enrolled.")).toBeVisible();
    expect(within(fleet).getByRole("link", { name: "Start in Fleet" })).toHaveAttribute(
      "href",
      "/workspace?id=server-administrator.fleet",
    );

    const cooling = await screen.findByTestId("bundle-widget-server-administrator.cooling-summary");
    await waitFor(() => expect(cooling).toHaveTextContent("Observed"));
    expect(cooling).not.toHaveTextContent("Truncated");
    expect(cooling).not.toHaveTextContent("false");
  });

  it("does not expose raw snapshot errors", async () => {
    global.fetch = vi.fn(async () => {
      throw new Error("internal credential and upstream details");
    });

    render(<DashboardPage />);
    await waitFor(() => {
      expect(screen.getByText("Core status unavailable")).toBeTruthy();
    });
    expect(screen.queryByText(/internal credential/i)).toBeNull();
  });

  it("turns live events into human activity rows without dumping raw payloads", async () => {
    global.fetch = vi.fn(async (input) => {
      const url = String(input);
      const body = url.endsWith("/bootstrap")
        ? { gateway_version: "0.7.5", active_plugs: [], degraded_reasons: [], knowledge: { canonical_ready: true, search_ready: true, relation_ready: true } }
        : url.endsWith("/jobs/active") ? { jobs: [] } : { count: 0 };
      return { ok: true, status: 200, json: async () => body } as Response;
    });
    render(<DashboardPage />);
    await screen.findByText("Operations are steady");
    act(() => {
      MockWebSocket.instances[0].onmessage?.({
        data: JSON.stringify({
          type: "tool_completed",
          action: "Restore monitoring",
          target_id: "edge-one",
          status: "recovered",
          private_key: "must-not-render",
        }),
      });
    });
    expect(screen.getByText("Tool Completed")).toBeTruthy();
    expect(screen.getByText(/Restore monitoring · edge-one · recovered/)).toBeTruthy();
    expect(screen.queryByText("must-not-render")).toBeNull();
    expect(document.querySelector("#dashboard-live-feed pre")).toBeNull();
  });
});
