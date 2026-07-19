import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { StatusStrip } from "../../app/components/shell/status-strip";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    identity: null,
    viewMode: "user",
    setViewMode: vi.fn(),
    clearKey: vi.fn(),
  }),
}));

// ---------------------------------------------------------------------------
// fetch mock
// ---------------------------------------------------------------------------

function mockFetch(status: number, body: unknown = {}) {
  global.fetch = vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as Response);
}

beforeEach(() => {
  // Use real timers — setInterval in useGatewayHealth is fine for unit tests
  // because we clean up via unmount. Fake timers cause infinite-loop abort
  // when the health hook re-registers intervals.
  vi.restoreAllMocks();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("StatusStrip", () => {
  it("renders the brand while the first health fetch is pending", () => {
    // Never resolves
    global.fetch = vi.fn().mockReturnValue(new Promise(() => {}));
    render(<StatusStrip />);
    expect(screen.getByTestId("brand").textContent).toContain("Gadgetron");
    expect(screen.queryByTestId("health-indicator")).toBeNull();
  });

  it("shows the English/Korean selector in the right-side status controls", () => {
    global.fetch = vi.fn().mockReturnValue(new Promise(() => {}));
    render(<StatusStrip />);
    const toggle = screen.getByTestId("locale-toggle");
    expect(toggle).toHaveAccessibleName("Language");
    expect(screen.getByRole("button", { name: "Eng" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Kor" })).toHaveAttribute("aria-pressed", "false");
  });

  it("keeps healthy state visually quiet on 200 with no degraded_reasons", async () => {
    mockFetch(200, { degraded_reasons: [] });
    render(<StatusStrip />);
    // Wait for the async fetch to resolve and state to update
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByTestId("health-indicator")).toBeNull();
  });

  it("shows degraded state on 200 with degraded_reasons present", async () => {
    mockFetch(200, { degraded_reasons: ["index stale"] });
    render(<StatusStrip />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    const indicator = screen.getByTestId("health-indicator");
    expect(indicator.textContent).toContain("Gateway degraded");
  });

  it("shows degraded state on 503", async () => {
    mockFetch(503, {});
    render(<StatusStrip />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    const indicator = screen.getByTestId("health-indicator");
    expect(indicator.textContent).toContain("Gateway degraded");
  });

  it("shows blocked / unreachable on network error", async () => {
    global.fetch = vi.fn().mockRejectedValue(new Error("Network error"));
    render(<StatusStrip />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    const indicator = screen.getByTestId("health-indicator");
    expect(indicator.textContent).toContain("Gateway unreachable");
  });

  it("does not render the removed legacy knowledge plugs", async () => {
    global.fetch = vi.fn().mockReturnValue(new Promise(() => {}));
    render(<StatusStrip />);
    expect(screen.queryByTestId("knowledge-plugs")).toBeNull();
  });

  it("omits the removed session placeholder when no sessionId/actor provided", async () => {
    global.fetch = vi.fn().mockReturnValue(new Promise(() => {}));
    render(<StatusStrip />);
    expect(screen.queryByTestId("session-placeholder")).toBeNull();
  });

  it("shows sessionId when provided", async () => {
    mockFetch(200, {});
    render(<StatusStrip sessionId="abcd1234efgh5678" />);
    const sessionEl = screen.getByTestId("session-id");
    expect(sessionEl.textContent).toContain("abcd1234");
  });
});
