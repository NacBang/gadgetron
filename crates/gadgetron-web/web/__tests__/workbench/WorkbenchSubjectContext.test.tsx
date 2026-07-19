import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";
import {
  getActiveConversationId,
  setActiveConversationId,
} from "../../app/lib/conversation-id";
import {
  buildSubjectDraft,
  parseWorkbenchSubject,
  readConversationSubject,
  startPennyDiscussion,
  useWorkbenchSubject,
  withSubjectContext,
  WorkbenchSubjectProvider,
  writeConversationSubject,
  type WorkbenchSubject,
} from "../../app/lib/workbench-subject-context";
import { LOCALE_STORAGE_KEY } from "../../app/lib/i18n";

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
  id: "article-1",
  kind: "knowledge_article",
  bundle: "knowledge",
  title: "PostgreSQL recovery playbook",
  subtitle: "Operations · reviewed",
  href: "/web/knowledge?q=postgres-recovery",
  summary: "A reviewed recovery sequence for PostgreSQL incidents.",
  facts: {
    topic: "recovery",
    status: "reviewed",
  },
  prompt: "Review this playbook with me and identify missing safeguards.",
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

    const stored = readConversationSubject("conv-related");
    expect(stored?.related?.[0]).toMatchObject({
      id: "page-1",
      kind: "knowledge_page",
      title: "Backup validation checklist",
      status: "ok",
    });
  });

  it("drops malformed related refs without dropping the subject", () => {
    window.localStorage.setItem(
      "gadgetron_subject_conv-bad-related",
      JSON.stringify({
        id: "article-1",
        kind: "knowledge_article",
        bundle: "knowledge",
        title: "Recovery playbook",
        related: [
          { id: 42, title: "bad" },
          { id: "ok", kind: "knowledge_page", title: "OK" },
        ],
      }),
    );

    const stored = readConversationSubject("conv-bad-related");
    expect(stored?.title).toBe("Recovery playbook");
    expect(stored?.related).toEqual([
      { id: "ok", kind: "knowledge_page", title: "OK" },
    ]);
  });

  it("returns null for malformed stored subjects", () => {
    localStorage.setItem("gadgetron_subject_conv-bad", "{bad json");

    expect(readConversationSubject("conv-bad")).toBeNull();
  });

  it("validates a Bundle-projected Penny subject before persistence", () => {
    expect(parseWorkbenchSubject({ id: "edge-one", kind: "server", bundle: "server-administrator", title: "Edge one" })).toMatchObject({ id: "edge-one", kind: "server" });
    expect(parseWorkbenchSubject({ id: "edge-one", kind: "server", title: "Missing owner" })).toBeNull();
  });

  it("builds an English-first draft from structured subject facts", () => {
    const draft = buildSubjectDraft(subject);

    expect(draft).toContain("Review this playbook with me");
    expect(draft).toContain("Subject: PostgreSQL recovery playbook");
    expect(draft).toContain("Bundle: knowledge");
    expect(draft).toContain('"status": "reviewed"');
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
      "PostgreSQL recovery playbook",
    );
    expect(localStorage.getItem("gadgetron_pending_submit_conv-2")).toBe("1");
    expect(assign).toHaveBeenCalledWith("/web");
  });

  it("opens the companion without leaving the current page", () => {
    const opened = vi.fn();
    const assign = vi.fn();
    window.addEventListener("gadgetron:penny-companion-open", opened);

    startPennyDiscussion(subject, {
      conversationId: "conv-companion",
      navigate: assign,
      surface: "companion",
    });

    expect(opened).toHaveBeenCalledTimes(1);
    expect(assign).not.toHaveBeenCalled();
    expect(getActiveConversationId()).toBe("conv-companion");
    window.removeEventListener("gadgetron:penny-companion-open", opened);
  });

  it("refreshes the provider when the active conversation changes", () => {
    writeConversationSubject("conv-1", {
      ...subject,
      id: "article-1",
      title: "First article",
    });
    writeConversationSubject("conv-2", {
      ...subject,
      id: "article-2",
      title: "Second article",
    });
    setActiveConversationId("conv-1");

    render(
      <WorkbenchSubjectProvider>
        <SubjectTitleProbe />
      </WorkbenchSubjectProvider>,
    );

    expect(screen.getByTestId("subject-title").textContent).toBe(
      "First article",
    );

    act(() => {
      setActiveConversationId("conv-2");
    });

    expect(screen.getByTestId("subject-title").textContent).toBe(
      "Second article",
    );
  });
});

describe("withSubjectContext (ISSUE 53)", () => {
  beforeEach(() => {
    localStorageMock.clear();
    sessionStorageMock.clear();
  });

  it("prepends the subject draft to a first message", () => {
    setActiveConversationId("conv-ctx");
    writeConversationSubject("conv-ctx", subject);

    const out = withSubjectContext("이 버그에 대해서 알려줘");

    expect(out).toContain("Subject: PostgreSQL recovery playbook");
    expect(out).toContain('"status": "reviewed"');
    expect(out).toContain("Question: 이 버그에 대해서 알려줘");
  });

  it("uses Korean headings when the saved locale is Korean", () => {
    window.localStorage.setItem(LOCALE_STORAGE_KEY, "ko");
    setActiveConversationId("conv-ko");
    writeConversationSubject("conv-ko", subject);

    const out = withSubjectContext("다음 단계를 알려줘");

    expect(out).toContain("주제: PostgreSQL recovery playbook");
    expect(out).toContain("질문: 다음 단계를 알려줘");
  });

  it("leaves no-subject, slash-command, and already-drafted text alone", () => {
    setActiveConversationId("conv-empty");
    expect(withSubjectContext("hello")).toBe("hello");

    setActiveConversationId("conv-ctx2");
    writeConversationSubject("conv-ctx2", subject);
    expect(withSubjectContext("/help")).toBe("/help");

    const draft = buildSubjectDraft(subject);
    expect(withSubjectContext(draft)).toBe(draft);
  });
});
