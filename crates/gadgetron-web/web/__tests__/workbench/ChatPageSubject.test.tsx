import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FormEvent, ReactNode } from "react";
import { setActiveConversationId } from "../../app/lib/conversation-id";
import Home from "../../app/(shell)/page";

const composerMocks = vi.hoisted(() => ({
  getState: vi.fn(() => ({ text: "" })),
  setText: vi.fn(),
  send: vi.fn(),
  subscribe: vi.fn(() => () => {}),
}));

const subjectHook = vi.hoisted(() => ({
  value: {
    activeConversationId: null as string | null,
    subject: null as {
      id: string;
      kind: string;
      bundle: string;
      title: string;
      href?: string;
    } | null,
    refresh: vi.fn(),
    refreshSubject: vi.fn(),
    clearActiveSubject: vi.fn(),
  },
}));

vi.mock("@assistant-ui/react", () => ({
  ThreadPrimitive: {
    Root: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Viewport: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Empty: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Messages: () => null,
    ScrollToBottom: ({ children }: { children: ReactNode }) => <>{children}</>,
  },
  MessagePrimitive: {
    Parts: () => null,
  },
  ComposerPrimitive: {
    Root: ({
      children,
      onSubmit,
    }: {
      children: ReactNode;
      onSubmit?: (event: FormEvent<HTMLFormElement>) => void;
    }) => <form onSubmit={onSubmit}>{children}</form>,
    Input: (props: { placeholder?: string }) => (
      <textarea data-testid="composer-input" placeholder={props.placeholder} />
    ),
    Cancel: ({ children }: { children: ReactNode }) => <>{children}</>,
    Send: ({ children }: { children: ReactNode }) => <>{children}</>,
  },
  useComposerRuntime: () => composerMocks,
  useThread: (selector: (state: { isRunning: boolean; messages: unknown[] }) => unknown) =>
    selector({ isRunning: false, messages: [] }),
  useMessage: (selector: (state: { content: unknown[]; status: null }) => unknown) =>
    selector({ content: [], status: null }),
  useThreadViewport: (selector: (state: { isAtBottom: boolean }) => unknown) =>
    selector({ isAtBottom: true }),
}));

vi.mock("../../app/components/slash-autocomplete", () => ({
  SlashAutocomplete: () => <div data-testid="slash-autocomplete" />,
}));

vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    clearKey: vi.fn(),
  }),
}));

vi.mock("../../app/lib/workbench-subject-context", () => ({
  useWorkbenchSubject: () => subjectHook.value,
}));

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

describe("Chat page subject context", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  beforeEach(() => {
    localStorageMock.clear();
    sessionStorageMock.clear();
    composerMocks.getState.mockReset();
    composerMocks.getState.mockReturnValue({ text: "" });
    composerMocks.setText.mockClear();
    composerMocks.send.mockClear();
    composerMocks.subscribe.mockReset();
    composerMocks.subscribe.mockReturnValue(() => {});
    subjectHook.value = {
      activeConversationId: null,
      subject: null,
      refresh: vi.fn(),
      refreshSubject: vi.fn(),
      clearActiveSubject: vi.fn(),
    };
  });

  it("uses English-first composer copy", () => {
    render(<Home />);

    expect(screen.getByTestId("composer-input").getAttribute("placeholder")).toBe(
      "Ask Penny or type /command",
    );
  });

  it("renders the active subject banner with source link", () => {
    subjectHook.value = {
      ...subjectHook.value,
      activeConversationId: "conv-1",
      subject: {
        id: "finding-1",
        kind: "log_finding",
        bundle: "logs",
        title: "SMART pending sectors",
        href: "/web/findings?host=host-1",
      },
    };

    render(<Home />);

    expect(screen.getByTestId("active-subject-banner").textContent).toContain(
      "Talking about",
    );
    expect(screen.getByTestId("active-subject-banner").textContent).toContain(
      "SMART pending sectors",
    );
    expect(screen.getByText("Open source").getAttribute("href")).toBe(
      "/web/findings?host=host-1",
    );
  });

  it("refreshes subject context when the composer hydrates a seeded draft", async () => {
    const refreshSubject = vi.fn();
    subjectHook.value = {
      ...subjectHook.value,
      refreshSubject,
    };
    setActiveConversationId("conv-chat");
    localStorage.setItem("gadgetron_draft_conv-chat", "seeded draft");

    render(<Home />);

    await waitFor(() => {
      expect(refreshSubject).toHaveBeenCalled();
    });
    expect(composerMocks.setText).toHaveBeenCalledWith("seeded draft");
  });

  it("refetches conversation history when the chat route regains focus", async () => {
    setActiveConversationId("conv-return");
    let historyAvailable = false;
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/workbench/conversations/conv-return/messages")) {
        return {
          ok: true,
          json: async () => ({
            messages: historyAvailable
              ? [
                  {
                    role: "assistant",
                    content: "Penny returned with the stored answer.",
                    ts: "2026-05-04T04:00:00Z",
                  },
                ]
              : [],
          }),
        } as Response;
      }
      if (url.endsWith("/api/v1/web/workbench/conversations")) {
        return {
          ok: true,
          json: async () => ({ conversations: [] }),
        } as Response;
      }
      return {
        ok: true,
        json: async () => ({}),
      } as Response;
    });
    vi.stubGlobal("fetch", fetchMock);

    render(<Home />);

    await waitFor(() => {
      expect(fetchMock).toHaveBeenCalledWith(
        expect.stringContaining(
          "/api/v1/web/workbench/conversations/conv-return/messages",
        ),
        expect.anything(),
      );
    });
    expect(
      screen.queryByText("Penny returned with the stored answer."),
    ).toBeNull();

    historyAvailable = true;
    window.dispatchEvent(new Event("focus"));

    await waitFor(() => {
      expect(
        screen.getByText("Penny returned with the stored answer."),
      ).toBeTruthy();
    });
  });
});
