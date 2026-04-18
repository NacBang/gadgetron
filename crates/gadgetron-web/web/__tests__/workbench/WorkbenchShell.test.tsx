import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { WorkbenchShell } from "../../app/components/shell/workbench-shell";

// ---------------------------------------------------------------------------
// Mocks
// ---------------------------------------------------------------------------

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
    // evidence pane defaults open per DEFAULT_PREFS
    expect(screen.getByTestId("evidence-pane")).toBeTruthy();
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

  it("evidence pane is collapsible and defaults open", async () => {
    render(
      <WorkbenchShell>
        <div>chat</div>
      </WorkbenchShell>,
    );
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    // Default: evidence pane is open
    expect(screen.getByTestId("evidence-pane")).toBeTruthy();
    // Collapse button exists
    expect(screen.getByTestId("evidence-pane-collapse-btn")).toBeTruthy();
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
