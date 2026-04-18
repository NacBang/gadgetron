import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { EvidencePane } from "../../app/components/shell/evidence-pane";

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
  });

  it("renders empty state with P2B roadmap copy when open", () => {
    render(
      <EvidencePane open={true} onToggle={() => {}} />,
    );
    const copy = screen.getByTestId("evidence-empty-copy");
    expect(copy.textContent).toContain(
      "Knowledge sources will appear here when Penny cites them",
    );
    expect(copy.textContent).toContain("P2B per ADR");
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
