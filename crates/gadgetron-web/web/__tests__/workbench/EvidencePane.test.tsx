import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { setActiveConversationId } from "../../app/lib/conversation-id";
import { EvidencePane } from "../../app/components/shell/evidence-pane";
import { InspectorProvider, useRegisterInspectorView } from "../../app/lib/inspector-context";
import { writeConversationSubject } from "../../app/lib/workbench-subject-context";

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
  }),
}));

const { mockUseEvidence } = vi.hoisted(() => ({
  mockUseEvidence: vi.fn(),
}));

vi.mock("../../app/lib/evidence-context", () => ({
  useEvidence: () => mockUseEvidence(),
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

const SCREEN_VIEW = {
  id: "library:item-one",
  title: "Preview",
  content: <div>Cooling runbook preview</div>,
};

function PreviewRegistration() {
  useRegisterInspectorView(SCREEN_VIEW);
  return null;
}

describe("EvidencePane", () => {
  beforeEach(() => {
    localStorageMock.clear();
    window.sessionStorage.clear();
    mockUseEvidence.mockReturnValue({
      items: [],
      wsStatus: "disconnected",
      clear: () => {},
    });
  });

  it("keeps the AI inspector hidden until Penny has activity", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    expect(screen.getByRole("complementary", { name: "Inspector" })).toBeTruthy();
    expect(screen.queryByRole("tab")).toBeNull();
    expect(screen.getByTestId("inspector-empty")).toHaveTextContent(
      "Ask Penny a question and its evidence will appear here",
    );
  });

  it("renders the active workbench subject in the context tab", () => {
    mockUseEvidence.mockReturnValue({
      items: [{ id: "activity-context", name: "assistant.read", kind: "tool_call", outcome: "success", at: Date.now() }],
      wsStatus: "open",
      clear: () => {},
    });
    setActiveConversationId("conv-context");
    writeConversationSubject("conv-context", {
      id: "article-1",
      kind: "knowledge_article",
      bundle: "knowledge",
      title: "PostgreSQL recovery playbook",
      subtitle: "Operations · reviewed",
      href: "/web/knowledge?q=postgres-recovery",
      summary: "A reviewed recovery sequence.",
      facts: { topic: "recovery", status: "reviewed" },
    });

    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    expect(screen.getByRole("tab", { name: "What AI sees" })).toBeTruthy();
    expect(screen.getByTestId("context-panel").textContent).toContain(
      "PostgreSQL recovery playbook",
    );
    expect(screen.getByText("View source").getAttribute("href")).toBe(
      "/web/knowledge?q=postgres-recovery",
    );
    expect(screen.getByTestId("context-panel").textContent).toContain(
      '"status": "reviewed"',
    );
  });

  it("renders related context refs in the Context tab", () => {
    mockUseEvidence.mockReturnValue({
      items: [{ id: "activity-related", name: "assistant.read", kind: "tool_call", outcome: "success", at: Date.now() }],
      wsStatus: "open",
      clear: () => {},
    });
    setActiveConversationId("conv-related-panel");
    writeConversationSubject("conv-related-panel", {
      id: "article-1",
      kind: "knowledge_article",
      bundle: "knowledge",
      title: "PostgreSQL recovery playbook",
      related: [
        {
          id: "page-1",
          kind: "knowledge_page",
          title: "Backup validation checklist",
          status: "ok",
          href: "/web/knowledge?q=backup-validation",
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
      "Backup validation checklist",
    );
    expect(
      screen.getByRole("link", { name: /Backup validation checklist/i }).getAttribute(
        "href",
      ),
    ).toBe("/web/knowledge?q=backup-validation");
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

  it("renders a screen preview first and offers AI activity as a secondary mode", async () => {
    mockUseEvidence.mockReturnValue({
      items: [{ id: "activity-1", name: "wiki.search", kind: "tool_call", outcome: "success", at: Date.now() }],
      wsStatus: "open",
      clear: () => {},
    });
    render(
      <InspectorProvider>
        <PreviewRegistration />
        <EvidencePane open={true} onToggle={() => {}} />
      </InspectorProvider>,
    );

    expect(await screen.findByText("Cooling runbook preview")).toBeVisible();
    expect(screen.getByRole("tab", { name: "Preview" })).toHaveAttribute("aria-selected", "true");
    fireEvent.click(screen.getByRole("tab", { name: "AI activity" }));
    expect(screen.getByRole("tab", { name: "Evidence" })).toBeVisible();
    expect(screen.getByRole("tab", { name: "Activity log" })).toBeVisible();
  });

  it("shows collapsed trigger when open=false", () => {
    render(
      <EvidencePane open={false} onToggle={() => {}} />,
    );
    expect(screen.getByTestId("evidence-pane-collapsed")).toBeTruthy();
    expect(screen.queryByTestId("evidence-pane")).toBeNull();
  });

  it("badges only activity that arrived since the Inspector was last open", () => {
    const firstItem = {
      id: "activity-1",
      name: "wiki.search",
      kind: "tool_call",
      outcome: "success",
      at: Date.now(),
    };
    const { rerender } = render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );

    mockUseEvidence.mockReturnValue({
      items: [firstItem],
      wsStatus: "disconnected",
      clear: () => {},
    });
    rerender(<EvidencePane open={true} onToggle={() => {}} />);
    rerender(<EvidencePane open={false} onToggle={() => {}} />);
    expect(screen.queryByTestId("evidence-pane-activity-badge")).toBeNull();

    mockUseEvidence.mockReturnValue({
      items: [
        firstItem,
        { ...firstItem, id: "activity-2", name: "web.search" },
      ],
      wsStatus: "disconnected",
      clear: () => {},
    });
    rerender(<EvidencePane open={false} onToggle={() => {}} />);

    expect(screen.getByTestId("evidence-pane-activity-badge").textContent).toBe("1");
    expect(screen.getByRole("button", { name: "Open Inspector, 1 new activity item" })).toBeTruthy();
  });

  it("delegates expand state to the shell preference owner", () => {
    const onToggle = vi.fn();
    render(
      <EvidencePane open={false} onToggle={onToggle} />,
    );
    fireEvent.click(screen.getByTestId("evidence-pane-expand-btn"));
    expect(onToggle).toHaveBeenCalledWith(true);
    expect(
      localStorageMock.getItem("gadgetron.workbench.evidencePaneOpen"),
    ).toBeNull();
  });

  it("delegates collapse state to the shell preference owner", () => {
    const onToggle = vi.fn();
    render(
      <EvidencePane open={true} onToggle={onToggle} />,
    );
    fireEvent.click(screen.getByTestId("evidence-pane-collapse-btn"));
    expect(onToggle).toHaveBeenCalledWith(false);
    expect(
      localStorageMock.getItem("gadgetron.workbench.evidencePaneOpen"),
    ).toBeNull();
  });

  it("applies width style when open", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} width={380} />,
    );
    const pane = screen.getByTestId("evidence-pane");
    expect((pane as HTMLElement).style.width).toBe("380px");
  });
});
