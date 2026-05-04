import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { setActiveConversationId } from "../../app/lib/conversation-id";
import { EvidencePane } from "../../app/components/shell/evidence-pane";
import { writeConversationSubject } from "../../app/lib/workbench-subject-context";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
  }),
}));

vi.mock("../../app/lib/evidence-context", () => ({
  useEvidence: () => ({
    items: [],
    wsStatus: "disconnected",
    clear: () => {},
  }),
}));

// ---------------------------------------------------------------------------
// localStorage mock
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

describe("EvidencePane", () => {
  beforeEach(() => {
    localStorageMock.clear();
    window.sessionStorage.clear();
  });

  it("renders the side panel context empty state by default", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    expect(screen.getByRole("button", { name: "Context" })).toBeTruthy();
    expect(screen.getByTestId("context-empty").textContent).toContain(
      "No active context",
    );
  });

  it("renders the active workbench subject in the context tab", () => {
    setActiveConversationId("conv-context");
    writeConversationSubject("conv-context", {
      id: "finding-1",
      kind: "log_finding",
      bundle: "logs",
      title: "SMART pending sectors",
      subtitle: "dg5R-PRO6000-8 · critical",
      href: "/web/findings?host=host-1",
      summary: "smartd reports pending sectors on /dev/sdb.",
      facts: { hostId: "host-1", severity: "critical" },
    });

    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    expect(screen.getByTestId("context-panel").textContent).toContain(
      "SMART pending sectors",
    );
    expect(screen.getByText("Open source").getAttribute("href")).toBe(
      "/web/findings?host=host-1",
    );
    expect(screen.getByTestId("context-panel").textContent).toContain(
      '"severity": "critical"',
    );
  });

  it("renders related context refs in the Context tab", () => {
    setActiveConversationId("conv-related-panel");
    writeConversationSubject("conv-related-panel", {
      id: "server-1",
      kind: "server",
      bundle: "servers",
      title: "dg5R-PRO6000-8",
      related: [
        {
          id: "finding-1",
          kind: "log_finding",
          title: "SMART pending sectors",
          status: "critical",
          href: "/web/findings?host=server-1",
        },
      ],
    });

    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    expect(screen.getByTestId("context-panel").textContent).toContain(
      "Related",
    );
    expect(screen.getByTestId("context-panel").textContent).toContain(
      "SMART pending sectors",
    );
    expect(
      screen.getByRole("link", { name: /SMART pending sectors/i }).getAttribute(
        "href",
      ),
    ).toBe("/web/findings?host=server-1");
  });

  it("does NOT render any mocked citation content", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );
    // No citation headings, source lists, or mocked data
    expect(screen.queryByText(/citation/i)).toBeNull();
    expect(screen.queryByText(/source:/i)).toBeNull();
    expect(screen.queryByTestId("citation-list")).toBeNull();
  });

  it("shows collapsed trigger when open=false", () => {
    render(
      <EvidencePane open={false} onToggle={() => {}} />,
    );
    expect(screen.getByTestId("evidence-pane-collapsed")).toBeTruthy();
    expect(screen.queryByTestId("evidence-pane")).toBeNull();
  });

  it("calls onToggle(true) and writes localStorage when expand button clicked", () => {
    const onToggle = vi.fn();
    render(
      <EvidencePane open={false} onToggle={onToggle} />,
    );
    fireEvent.click(screen.getByTestId("evidence-pane-expand-btn"));
    expect(onToggle).toHaveBeenCalledWith(true);
    expect(localStorageMock.getItem("gadgetron.workbench.evidencePaneOpen")).toBe(
      "true",
    );
  });

  it("calls onToggle(false) and writes localStorage when collapse button clicked", () => {
    const onToggle = vi.fn();
    render(
      <EvidencePane open={true} onToggle={onToggle} />,
    );
    fireEvent.click(screen.getByTestId("evidence-pane-collapse-btn"));
    expect(onToggle).toHaveBeenCalledWith(false);
    expect(localStorageMock.getItem("gadgetron.workbench.evidencePaneOpen")).toBe(
      "false",
    );
  });

  it("applies width style when open", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} width={380} />,
    );
    const pane = screen.getByTestId("evidence-pane");
    expect((pane as HTMLElement).style.width).toBe("380px");
  });
});
