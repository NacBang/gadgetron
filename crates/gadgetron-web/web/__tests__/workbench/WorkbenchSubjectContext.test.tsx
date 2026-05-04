import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import {
  getActiveConversationId,
  setActiveConversationId,
} from "../../app/lib/conversation-id";
import {
  buildSubjectDraft,
  readConversationSubject,
  startPennyDiscussion,
  useWorkbenchSubject,
  WorkbenchSubjectProvider,
  writeConversationSubject,
  type WorkbenchSubject,
} from "../../app/lib/workbench-subject-context";

const createStorageMock = () => {
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
};

const localStorageMock = createStorageMock();
const sessionStorageMock = createStorageMock();

Object.defineProperty(window, "localStorage", { value: localStorageMock });
Object.defineProperty(window, "sessionStorage", { value: sessionStorageMock });

const subject: WorkbenchSubject = {
  id: "finding-1",
  kind: "log_finding",
  bundle: "logs",
  title: "SMART pending sectors",
  subtitle: "dg5R-PRO6000-8 · critical",
  href: "/web/findings?host=host-1",
  summary: "smartd reports 6 pending sectors on /dev/sdb.",
  facts: {
    hostId: "host-1",
    severity: "critical",
    category: "storage",
  },
  prompt:
    "Review this log finding with me and recommend the next operational step.",
  createdAt: "2026-05-03T10:00:00.000Z",
};

function SubjectTitleProbe() {
  const { subject: activeSubject } = useWorkbenchSubject();
  return (
    <div data-testid="subject-title">{activeSubject?.title ?? "none"}</div>
  );
}

describe("workbench subject context", () => {
  beforeEach(() => {
    localStorageMock.clear();
    sessionStorageMock.clear();
    vi.restoreAllMocks();
  });

  it("stores and restores a subject by conversation id", () => {
    writeConversationSubject("conv-1", subject);

    expect(readConversationSubject("conv-1")).toEqual(subject);
    expect(readConversationSubject("conv-missing")).toBeNull();
  });

  it("stores and restores compact related refs", () => {
    writeConversationSubject("conv-related", {
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

    const stored = readConversationSubject("conv-related");
    expect(stored?.related?.[0]).toMatchObject({
      id: "finding-1",
      kind: "log_finding",
      title: "SMART pending sectors",
      status: "critical",
    });
  });

  it("drops malformed related refs without dropping the subject", () => {
    window.localStorage.setItem(
      "gadgetron_subject_conv-bad-related",
      JSON.stringify({
        id: "server-1",
        kind: "server",
        bundle: "servers",
        title: "dg5R-PRO6000-8",
        related: [
          { id: 42, title: "bad" },
          { id: "ok", kind: "server", title: "OK" },
        ],
      }),
    );

    const stored = readConversationSubject("conv-bad-related");
    expect(stored?.title).toBe("dg5R-PRO6000-8");
    expect(stored?.related).toEqual([
      { id: "ok", kind: "server", title: "OK" },
    ]);
  });

  it("returns null for malformed stored subjects", () => {
    localStorage.setItem("gadgetron_subject_conv-bad", "{bad json");

    expect(readConversationSubject("conv-bad")).toBeNull();
  });

  it("builds an English-first draft from structured subject facts", () => {
    const draft = buildSubjectDraft(subject);

    expect(draft).toContain("Review this log finding with me");
    expect(draft).toContain("Subject: SMART pending sectors");
    expect(draft).toContain("Bundle: logs");
    expect(draft).toContain('"severity": "critical"');
  });

  it("starts a Penny discussion with draft, subject, and pending submit flag", () => {
    const assign = vi.fn();

    const convId = startPennyDiscussion(subject, {
      conversationId: "conv-2",
      autoSubmit: true,
      navigateTo: "/web",
      navigate: assign,
    });

    expect(convId).toBe("conv-2");
    expect(getActiveConversationId()).toBe("conv-2");
    expect(readConversationSubject("conv-2")).toEqual(subject);
    expect(localStorage.getItem("gadgetron_draft_conv-2")).toContain(
      "SMART pending sectors",
    );
    expect(localStorage.getItem("gadgetron_pending_submit_conv-2")).toBe("1");
    expect(assign).toHaveBeenCalledWith("/web");
  });

  it("refreshes the provider when the active conversation changes", () => {
    writeConversationSubject("conv-1", {
      ...subject,
      id: "finding-1",
      title: "First finding",
    });
    writeConversationSubject("conv-2", {
      ...subject,
      id: "finding-2",
      title: "Second finding",
    });
    setActiveConversationId("conv-1");

    render(
      <WorkbenchSubjectProvider>
        <SubjectTitleProbe />
      </WorkbenchSubjectProvider>,
    );

    expect(screen.getByTestId("subject-title").textContent).toBe(
      "First finding",
    );

    act(() => {
      setActiveConversationId("conv-2");
    });

    expect(screen.getByTestId("subject-title").textContent).toBe(
      "Second finding",
    );
  });
});
