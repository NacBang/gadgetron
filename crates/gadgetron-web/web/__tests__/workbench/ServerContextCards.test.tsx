import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import {
  ServerContextCardStack,
} from "../../app/components/shell/server-context-cards";
import type { ServerContextItem } from "../../app/lib/evidence-context";

// ServerContextCardStack reads `useEvidence().serverContext` and
// fetches host info from the workbench `server-info` action when a
// card expands. Both the auth and evidence contexts are hoisted out
// of the React tree in tests so we can drive the component with
// hand-rolled fixtures and assert the collapsed → expanded → fetch
// → render lifecycle without booting the live WebSocket.

let stubItems: ServerContextItem[] = [];

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({ apiKey: "gad_test_key" }),
}));

vi.mock("../../app/lib/evidence-context", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("../../app/lib/evidence-context")>();
  return {
    ...actual,
    useEvidence: () => ({
      items: [],
      serverContext: stubItems,
      wsStatus: "open",
      clear: () => {},
    }),
  };
});

const HOST_A = "11111111-1111-4111-8111-111111111111";
const HOST_B = "22222222-2222-4222-8222-222222222222";

function makeItem(
  hostId: string,
  toolName: string,
  ageSeconds: number,
  mentions = 1,
): ServerContextItem {
  return {
    hostId,
    lastToolName: toolName,
    lastSeenAt: Date.now() - ageSeconds * 1000,
    mentionCount: mentions,
  };
}

describe("ServerContextCardStack", () => {
  beforeEach(() => {
    stubItems = [];
    vi.restoreAllMocks();
  });

  it("renders nothing when no server has been touched", () => {
    stubItems = [];
    const { container } = render(<ServerContextCardStack />);
    expect(container.firstChild).toBeNull();
  });

  it("renders one collapsed card per host with last-tool + age", () => {
    stubItems = [
      makeItem(HOST_A, "server.stats", 5),
      makeItem(HOST_B, "server.journal", 90),
    ];
    render(<ServerContextCardStack />);
    const cards = screen.getAllByTestId("server-context-card");
    expect(cards).toHaveLength(2);
    // Header includes the abbreviated tool name (without `server.` prefix).
    expect(cards[0]).toHaveTextContent("stats");
    expect(cards[1]).toHaveTextContent("journal");
    // Collapsed by default — no expanded inner content.
    expect(
      screen.queryByTestId("server-context-card-open-drawer"),
    ).toBeNull();
  });

  it("expands a card on click and fetches host snapshot", async () => {
    stubItems = [makeItem(HOST_A, "server.info", 5)];
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          result: {
            payload: [
              {
                type: "text",
                text: JSON.stringify({
                  id: HOST_A,
                  host: "10.0.0.42",
                  alias: "tower-a",
                  cpu_model: "AMD EPYC 7352",
                  gpus: ["NVIDIA A100 80GB"],
                  last_ok_at: "2026-05-06T10:00:00Z",
                }),
              },
            ],
          },
        }),
        { status: 200 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(<ServerContextCardStack />);
    const toggle = screen.getByTestId("server-context-card-toggle");
    await act(async () => {
      fireEvent.click(toggle);
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toContain("/workbench/actions/server-info");
    expect((init as RequestInit).method).toBe("POST");
    expect((init as RequestInit).body).toContain(HOST_A);

    // Expanded body shows the fetched fields.
    expect(await screen.findByText("tower-a")).toBeInTheDocument();
    expect(screen.getByText("10.0.0.42")).toBeInTheDocument();
    expect(screen.getByText("AMD EPYC 7352")).toBeInTheDocument();
    expect(screen.getByText("NVIDIA A100 80GB")).toBeInTheDocument();
    // Open-dashboard deep link is present when expanded.
    expect(
      screen.getByTestId("server-context-card-open-drawer"),
    ).toHaveAttribute("href", `/web/servers?host=${HOST_A}`);
  });

  it("collapses again on a second click without re-fetching", async () => {
    stubItems = [makeItem(HOST_A, "server.info", 5)];
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          result: {
            payload: [
              {
                type: "text",
                text: JSON.stringify({
                  id: HOST_A,
                  host: "10.0.0.42",
                  alias: null,
                  gpus: [],
                }),
              },
            ],
          },
        }),
        { status: 200 },
      ),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(<ServerContextCardStack />);
    const toggle = screen.getByTestId("server-context-card-toggle");
    await act(async () => {
      fireEvent.click(toggle);
    });
    // Wait for the expanded panel — assert via the open-drawer link
    // testid (unique per card) rather than the host string, which
    // also appears in the collapsed header when alias is null.
    expect(
      await screen.findByTestId("server-context-card-open-drawer"),
    ).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(toggle);
    });
    // Snapshot was already fetched; no second call.
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(
      screen.queryByTestId("server-context-card-open-drawer"),
    ).toBeNull();
  });

  it("surfaces a host-info error message when the action 4xx's", async () => {
    stubItems = [makeItem(HOST_A, "server.stats", 1)];
    const fetchMock = vi.fn().mockResolvedValue(
      new Response("not found", { status: 404 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    render(<ServerContextCardStack />);
    const toggle = screen.getByTestId("server-context-card-toggle");
    await act(async () => {
      fireEvent.click(toggle);
    });
    expect(await screen.findByText(/host info unavailable/i))
      .toBeInTheDocument();
  });
});
