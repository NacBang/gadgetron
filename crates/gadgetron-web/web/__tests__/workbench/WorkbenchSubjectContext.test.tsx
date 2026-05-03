import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  buildSubjectDraft,
  readConversationSubject,
  startPennyDiscussion,
  writeConversationSubject,
  type WorkbenchSubject,
} from "../../app/lib/workbench-subject-context";

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
Object.defineProperty(window, "sessionStorage", { value: localStorageMock });

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

describe("workbench subject context", () => {
  beforeEach(() => {
    localStorageMock.clear();
    vi.restoreAllMocks();
  });

  it("stores and restores a subject by conversation id", () => {
    writeConversationSubject("conv-1", subject);

    expect(readConversationSubject("conv-1")).toEqual(subject);
    expect(readConversationSubject("conv-missing")).toBeNull();
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
    expect(readConversationSubject("conv-2")).toEqual(subject);
    expect(localStorage.getItem("gadgetron_draft_conv-2")).toContain(
      "SMART pending sectors",
    );
    expect(localStorage.getItem("gadgetron_pending_submit_conv-2")).toBe("1");
    expect(assign).toHaveBeenCalledWith("/web");
  });
});
