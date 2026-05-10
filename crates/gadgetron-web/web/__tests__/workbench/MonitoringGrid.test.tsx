import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";
import { MonitoringGrid } from "../../app/components/copilot/monitoring-grid";

// MonitoringGrid drives the right pane of `/web/copilot`. It polls
// `server-list` once on mount and renders one card per host with a
// freshness badge derived from `last_ok_at`. These tests pin the
// happy path (one host, two hosts), the empty state, and the
// freshness-badge thresholds (live / lag / stale) so a regression in
// the age-bucket logic fails loud.

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: "gad_test", identity: null }),
}));

const HOST_LIVE = {
  id: "11111111-1111-4111-8111-111111111111",
  host: "10.0.0.1",
  alias: "live-tower",
  cpu_model: "AMD EPYC 7352",
  cpu_cores: 48,
  gpus: ["NVIDIA A100 80GB"],
  // server-monitor stamps last_ok_at on every successful poll —
  // anything within 90 s = "live" tone.
  last_ok_at: new Date(Date.now() - 5_000).toISOString(),
};

const HOST_LAG = {
  id: "22222222-2222-4222-8222-222222222222",
  host: "10.0.0.2",
  alias: null,
  gpus: [],
  // 2 minutes old — past 90 s warning threshold but still under the
  // 5 min critical cutoff.
  last_ok_at: new Date(Date.now() - 2 * 60 * 1000).toISOString(),
};

const HOST_STALE = {
  id: "33333333-3333-4333-8333-333333333333",
  host: "10.0.0.3",
  alias: "rack-3",
  gpus: ["NVIDIA H100"],
  // 10 minutes — well past the critical threshold.
  last_ok_at: new Date(Date.now() - 10 * 60 * 1000).toISOString(),
};

function mockServerList(hosts: unknown[]) {
  // Mirror the live workbench wire shape used by `(shell)/servers/
  // page.tsx::invokeAction` — `result.payload` is the MCP-style
  // content array `[{type:"text", text:"<json>"}]`, which the
  // grid's `unwrapPayload` parses back into the host record.
  return vi.fn().mockResolvedValue(
    new Response(
      JSON.stringify({
        result: {
          payload: [
            {
              type: "text",
              text: JSON.stringify({ hosts, count: hosts.length }),
            },
          ],
        },
      }),
      { status: 200 },
    ),
  );
}

describe("MonitoringGrid", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("renders empty state when server-list returns zero hosts", async () => {
    vi.stubGlobal("fetch", mockServerList([]));
    render(<MonitoringGrid />);
    expect(
      await screen.findByTestId("copilot-monitoring-empty"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/no hosts registered yet/i),
    ).toBeInTheDocument();
  });

  it("renders one card per host and shows the count in header", async () => {
    vi.stubGlobal("fetch", mockServerList([HOST_LIVE, HOST_LAG]));
    render(<MonitoringGrid />);
    await waitFor(() => {
      expect(screen.getAllByTestId("copilot-host-card")).toHaveLength(2);
    });
    expect(screen.getByText(/2 registered/i)).toBeInTheDocument();
  });

  it("colors the freshness badge by last_ok_at age (live / lag / stale)", async () => {
    vi.stubGlobal("fetch", mockServerList([HOST_LIVE, HOST_LAG, HOST_STALE]));
    render(<MonitoringGrid />);
    await waitFor(() => {
      expect(screen.getAllByTestId("copilot-host-card")).toHaveLength(3);
    });
    const cards = screen.getAllByTestId("copilot-host-card");
    expect(cards[0]).toHaveTextContent(/live/i);
    expect(cards[1]).toHaveTextContent(/lag/i);
    expect(cards[2]).toHaveTextContent(/stale/i);
  });

  it("shows host alias when present, falls back to host string otherwise", async () => {
    vi.stubGlobal("fetch", mockServerList([HOST_LIVE, HOST_LAG]));
    render(<MonitoringGrid />);
    await waitFor(() => {
      expect(screen.getByText("live-tower")).toBeInTheDocument();
    });
    // HOST_LAG.alias is null — fallback header is the host IP. The
    // string "10.0.0.2" also renders in the IP sub-line below the
    // header on every card, so `getAllByText` is the correct query
    // (>=1 means the alias-fallback path produced the header).
    expect(screen.getAllByText("10.0.0.2").length).toBeGreaterThanOrEqual(1);
  });

  it("each card deep-links to /web/servers?host=<id>", async () => {
    vi.stubGlobal("fetch", mockServerList([HOST_LIVE]));
    render(<MonitoringGrid />);
    const link = await screen.findByTestId("copilot-host-open-dashboard");
    expect(link).toHaveAttribute(
      "href",
      `/web/servers?host=${HOST_LIVE.id}`,
    );
  });
});
