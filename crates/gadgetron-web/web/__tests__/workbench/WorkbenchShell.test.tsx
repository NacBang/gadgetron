import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { WorkbenchShell } from "../../app/components/shell/workbench-shell";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

// LeftRail derives its active tab from next/navigation's usePathname()
// after ISSUE 29. Vitest has no Next.js router context, so stub the
// hook to "/" (chat tab).
vi.mock("next/navigation", () => ({
  usePathname: () => "/",
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
    saveKey: vi.fn(),
    clearKey: vi.fn(),
    hydrated: true,
    identity: {
      role: "admin",
      display_name: "Local Admin",
      email: "admin@example.local",
    },
    refreshIdentity: vi.fn(),
    viewMode: "admin",
    setViewMode: vi.fn(),
  }),
  useHasAuth: () => true,
  authHeaders: () => ({}),
}));

vi.mock("../../app/components/shell/conversations-pane", () => ({
  ConversationsPane: ({ collapsed }: { collapsed: boolean }) => (
    <div data-testid="conversations-pane" data-collapsed={String(collapsed)} />
  ),
}));

const localStorageMock = (() => {
  let store: Record<string, string> = {};
  return {
    getItem: (key: string) => store[key] ?? null,
    setItem: (key: string, value: string) => {
      store[key] = value;
    },
    removeItem: (key: string) => {
      delete store[key];
    },
    clear: () => {
      store = {};
    },
  };
})();

Object.defineProperty(window, "localStorage", { value: localStorageMock });

function mockFetch(status: number, body: unknown = {}) {
  global.fetch = vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  } as Response);
}

beforeEach(() => {
  localStorageMock.clear();
  vi.restoreAllMocks();
  mockFetch(200, {});
  // navigator.onLine default = true
  Object.defineProperty(navigator, "onLine", {
    value: true,
    writable: true,
    configurable: true,
  });
  Object.defineProperty(window, "innerWidth", {
    value: 1440,
    writable: true,
    configurable: true,
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("WorkbenchShell", () => {
  it("renders 3-panel structure: left rail, chat column, evidence pane", async () => {
    render(
      <WorkbenchShell>
        <div data-testid="chat-content">chat here</div>
      </WorkbenchShell>,
    );
    // Wait for mount effects (prefs load + health check)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.getByTestId("left-rail")).toBeTruthy();
    expect(screen.getByTestId("chat-column")).toBeTruthy();
    // evidence pane defaults collapsed per DEFAULT_PREFS
    expect(screen.getByTestId("evidence-pane-collapsed")).toBeTruthy();
  });

  it("renders the workbench shell wrapper", () => {
    render(
      <WorkbenchShell>
        <div>children</div>
      </WorkbenchShell>,
    );
    expect(screen.getByTestId("workbench-shell")).toBeTruthy();
  });

  it("left rail starts at 240px default width", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    const rail = screen.getByTestId("left-rail");
    expect((rail as HTMLElement).style.width).toBe("240px");
  });

  it("left rail nav targets embedded /web document routes", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    expect(screen.getByTestId("nav-tab-chat").getAttribute("href")).toBe(
      "/web",
    );
    expect(screen.getByTestId("nav-tab-wiki").getAttribute("href")).toBe(
      "/web/wiki",
    );
    expect(screen.getByTestId("nav-tab-dashboard").getAttribute("href")).toBe(
      "/web/dashboard",
    );
    expect(screen.getByTestId("nav-tab-servers").getAttribute("href")).toBe(
      "/web/servers",
    );
  });

  it("marks the active nav link as the current page", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    const chatLink = screen.getByTestId("nav-tab-chat");
    expect(chatLink.getAttribute("aria-current")).toBe("page");
    expect(chatLink.getAttribute("role")).toBeNull();
    expect(chatLink.getAttribute("aria-selected")).toBeNull();
  });

  it("does not render unwired stub nav entries", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    expect(screen.queryByTestId("nav-tab-knowledge")).toBeNull();
    expect(screen.queryByTestId("nav-tab-bundles")).toBeNull();
  });

  it("collapses left rail and hides evidence pane on narrow desktop", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 900,
      writable: true,
      configurable: true,
    });
    window.dispatchEvent(new Event("resize"));

    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });

    const rail = screen.getByTestId("left-rail");
    expect(rail.className).toContain("w-12");
    expect(screen.queryByTestId("evidence-pane-collapsed")).toBeNull();
  });

  it("does not persist a collapsed preference while forced collapsed", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 900,
      writable: true,
      configurable: true,
    });

    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );

    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });

    const button = screen.getByTestId("left-rail-collapse-btn");
    expect(button).toBeDisabled();
    expect(button.getAttribute("aria-label")).toBe(
      "Navigation is collapsed on narrow screens",
    );

    fireEvent.click(button);
    expect(localStorageMock.getItem("gadgetron.workbench.prefs")).toBeNull();
  });

  it("renders caller-supplied right rail on narrow desktop", async () => {
    Object.defineProperty(window, "innerWidth", {
      value: 900,
      writable: true,
      configurable: true,
    });

    render(
      <WorkbenchShell rightRail={<aside data-testid="custom-right-rail" />}>
        <div>chat</div>
      </WorkbenchShell>,
    );

    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });

    expect(screen.getByTestId("custom-right-rail")).toBeTruthy();
    expect(screen.queryByTestId("evidence-pane-collapsed")).toBeNull();
  });

  it("evidence pane defaults collapsed and can be reopened", async () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    // Default: evidence pane is collapsed
    expect(screen.getByTestId("evidence-pane-collapsed")).toBeTruthy();
    // Expand button exists
    expect(screen.getByTestId("evidence-pane-expand-btn")).toBeTruthy();
  });

  it("does NOT show failure overlay when health=healthy", async () => {
    mockFetch(200, {});
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByTestId("failure-panel")).toBeNull();
  });

  it("shows failure overlay when health=blocked (fetch throws)", async () => {
    global.fetch = vi.fn().mockRejectedValue(new Error("net fail"));
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.getByTestId("failure-panel")).toBeTruthy();
  });

  it("does NOT render offline banner when navigator.onLine=true", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    expect(screen.queryByTestId("offline-banner")).toBeNull();
  });

  it("renders children inside chat column", () => {
    render(
      <WorkbenchShell>
        <div data-testid="inner-child">hello workbench</div>
      </WorkbenchShell>,
    );
    const child = screen.getByTestId("inner-child");
    const chatCol = screen.getByTestId("chat-column");
    expect(chatCol.contains(child)).toBe(true);
  });

  it("renders status strip", () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    // StatusStrip renders a status element
    expect(screen.getByRole("status")).toBeTruthy();
  });
});
