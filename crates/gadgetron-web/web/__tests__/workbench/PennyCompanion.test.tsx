import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FormEvent, ReactNode } from "react";

import { PennyCompanion } from "../../app/components/chat/penny-companion";
import {
  PENNY_COMPANION_STORAGE_KEY,
  clampCompanionLayout,
  pennyCompanionStorageKey,
  readStoredCompanionState,
} from "../../app/components/chat/penny-companion-layout";

const navigation = vi.hoisted(() => ({
  pathname: "/dashboard",
  push: vi.fn(),
}));
const composer = vi.hoisted(() => ({
  text: "What needs attention?",
  getState: vi.fn(() => ({ text: "What needs attention?" })),
  setText: vi.fn(),
  send: vi.fn(),
  subscribe: vi.fn(() => () => {}),
}));
const thread = vi.hoisted(() => ({ running: false, messages: [] as unknown[] }));
const workbenchSubject = vi.hoisted(() => ({ activeConversationId: "conv-1" }));
const resume = vi.hoisted(() => ({
  snapshot: null as null | {
    job_id: string;
    conversation_id: string;
    status: "streaming" | "complete" | "error" | "cancelled";
    chunk_count: number;
    is_finished: boolean;
  },
}));

vi.mock("next/navigation", () => ({
  usePathname: () => navigation.pathname,
  useRouter: () => ({ push: navigation.push }),
}));
vi.mock("../../app/lib/auth-context", () => ({
  useAuth: () => ({
    apiKey: null,
    hydrated: true,
    identity: { user_id: "user-1" },
  }),
}));
vi.mock("@assistant-ui/react", () => ({
  ThreadPrimitive: {
    Root: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Viewport: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Empty: ({ children }: { children: ReactNode }) => <div>{children}</div>,
    Messages: () => null,
  },
  MessagePrimitive: { Parts: () => null },
  ComposerPrimitive: {
    Root: ({
      children,
      onSubmit,
    }: {
      children: ReactNode;
      onSubmit?: (event: FormEvent<HTMLFormElement>) => void;
    }) => <form onSubmit={onSubmit}>{children}</form>,
    Input: (props: { placeholder?: string }) => (
      <textarea data-testid="companion-input" placeholder={props.placeholder} />
    ),
    Cancel: ({ children }: { children: ReactNode }) => <>{children}</>,
    Send: ({ children }: { children: ReactNode }) => <>{children}</>,
  },
  useComposerRuntime: () => composer,
  useThread: (selector: (state: { isRunning: boolean; messages: unknown[] }) => unknown) =>
    selector({ isRunning: thread.running, messages: thread.messages }),
}));
vi.mock("../../app/lib/workbench-subject-context", () => ({
  PENNY_COMPANION_EVENT: "gadgetron:penny-companion-open",
  useWorkbenchSubject: () => ({
    activeConversationId: workbenchSubject.activeConversationId,
    subject: { title: "Server fleet", id: "fleet", kind: "server", bundle: "server-administrator" },
    refreshSubject: vi.fn(),
    clearActiveSubject: vi.fn(),
  }),
  withSubjectContext: (text: string) => `Subject context\n\n${text}`,
}));
vi.mock("../../app/lib/workbench-page-context", () => ({
  useWorkbenchPageContext: () => ({
    page: { id: "/dashboard", title: "Dashboard", href: "/web/dashboard" },
    workspace: { id: "fleet", title: "Server operations" },
    selection: { id: "server-1", kind: "server", title: "GPU server 1" },
    filters: { status: "critical" },
    timeRange: "30m",
  }),
  withWorkbenchPageContext: (text: string, snapshot: unknown) =>
    `Current screen context:\n${JSON.stringify(snapshot)}\n\nQuestion: ${text}`,
}));
vi.mock("../../app/lib/chat-resume", () => ({
  cancelActiveConversationJob: vi.fn(),
  isJobRunning: (snapshot: typeof resume.snapshot) =>
    snapshot?.status === "streaming" && !snapshot.is_finished,
  useActiveJob: () => resume.snapshot,
}));
vi.mock("../../app/lib/conversation-id", () => ({
  getActiveConversationId: () => "conv-1",
}));
vi.mock("../../app/components/markdown-text", () => ({ MarkdownText: () => null }));
vi.mock("../../app/components/reasoning-part", () => ({ ReasoningPart: () => null }));
vi.mock("../../app/components/tool-part", () => ({ ToolPart: () => null }));

describe("Penny companion", () => {
  beforeEach(() => {
    navigation.pathname = "/dashboard";
    navigation.push.mockClear();
    composer.setText.mockClear();
    composer.send.mockClear();
    composer.getState.mockReturnValue({ text: "What needs attention?" });
    composer.subscribe.mockReturnValue(() => {});
    thread.running = false;
    thread.messages = [];
    workbenchSubject.activeConversationId = "conv-1";
    resume.snapshot = null;
    window.localStorage.clear();
    Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 900, configurable: true });
  });

  it("clamps restored geometry inside the viewport", () => {
    expect(
      clampCompanionLayout(
        { x: -100, y: 2_000, width: 2_000, height: 100 },
        { width: 1_000, height: 700 },
      ),
    ).toEqual({ x: 16, y: 324, width: 968, height: 360 });
  });

  it("opens over the current page and maximizes in place without changing context", async () => {
    render(<PennyCompanion />);
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));

    let panel = await screen.findByTestId("penny-companion");
    expect(panel.textContent).toContain("Dashboard");
    expect(panel.textContent).toContain("Server fleet");
    fireEvent.click(screen.getByRole("button", { name: "Maximize Penny" }));
    panel = await screen.findByTestId("penny-companion");
    expect(panel.textContent).toContain("Dashboard");
    expect(screen.getByRole("button", { name: "Restore Penny window" })).toBeTruthy();
    expect(navigation.push).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Restore Penny window" }));
    expect(await screen.findByRole("button", { name: "Maximize Penny" })).toBeTruthy();
  });

  it("restores a persisted maximized mode with bounded medium geometry", () => {
    window.localStorage.setItem(PENNY_COMPANION_STORAGE_KEY, JSON.stringify({
      mode: "maximized",
      layout: { x: 900, y: 300, width: 440, height: 584 },
    }));
    const restored = readStoredCompanionState({ width: 1_000, height: 700 });
    expect(restored.mode).toBe("maximized");
    expect(restored.layout).toEqual({ x: 544, y: 100, width: 440, height: 584 });
  });

  it("keeps companion geometry separate for each signed-in user", async () => {
    const userOneKey = pennyCompanionStorageKey("user:user-1");
    const userTwoKey = pennyCompanionStorageKey("user:user-2");
    window.localStorage.setItem(userTwoKey, JSON.stringify({
      mode: "maximized",
      layout: { x: 24, y: 24, width: 700, height: 640 },
    }));

    render(<PennyCompanion />);
    await waitFor(() => expect(window.localStorage.getItem(userOneKey)).not.toBeNull());

    expect(window.localStorage.getItem(userOneKey)).not.toBe(
      window.localStorage.getItem(userTwoKey),
    );
    expect(window.localStorage.getItem(PENNY_COMPANION_STORAGE_KEY)).toBeNull();
  });

  it("snapshots current page context when sending", async () => {
    render(<PennyCompanion />);
    window.dispatchEvent(new Event("gadgetron:penny-companion-open"));
    await screen.findByTestId("companion-input");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => {
      expect(composer.setText).toHaveBeenCalledWith(
        expect.stringContaining("Current screen context"),
      );
    });
    expect(composer.setText).toHaveBeenCalledWith(
      expect.stringContaining("Subject context"),
    );
    expect(composer.send).toHaveBeenCalledOnce();
  });

  it("removes a context part before the next message", async () => {
    render(<PennyCompanion />);
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));
    fireEvent.click(
      screen.getByRole("button", { name: "Remove Filters · 1 from current screen context" }),
    );
    const input = screen.getByTestId("companion-input");
    fireEvent.submit(input.closest("form")!);

    const outgoing = composer.setText.mock.calls.at(-1)?.[0] as string;
    expect(outgoing).toContain("Current screen context");
    expect(outgoing).not.toContain("critical");
  });

  it("preserves the user's context choice across minimize and restore", async () => {
    render(<PennyCompanion />);
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));
    fireEvent.click(screen.getByRole("button", { name: "Dashboard" }));
    expect(screen.getByPlaceholderText("Ask Penny")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Minimize Penny" }));
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));

    expect(screen.getByRole("button", { name: "General chat" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    expect(screen.getByPlaceholderText("Ask Penny")).toBeTruthy();
  });

  it("restores current screen context for an explicit Ask Penny action", async () => {
    render(<PennyCompanion />);
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));
    fireEvent.click(screen.getByRole("button", { name: "Dashboard" }));
    fireEvent.click(screen.getByRole("button", { name: "Minimize Penny" }));

    window.dispatchEvent(new Event("gadgetron:penny-companion-open"));

    expect(await screen.findByRole("button", { name: "Dashboard" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByPlaceholderText("Ask Penny about this screen")).toBeTruthy();
  });

  it("supports keyboard move, resize, and Escape minimize", async () => {
    render(<PennyCompanion />);
    fireEvent.click(await screen.findByTestId("penny-companion-launcher"));
    const panel = await screen.findByTestId("penny-companion");
    const initialLeft = Number.parseInt(panel.style.left, 10);
    fireEvent.keyDown(screen.getByRole("button", { name: "Move Penny companion" }), {
      key: "ArrowLeft",
    });
    expect(Number.parseInt(panel.style.left, 10)).toBe(initialLeft - 16);

    fireEvent.keyDown(screen.getByRole("button", { name: "Resize Penny companion" }), {
      key: "Home",
    });
    expect(panel.style.width).toBe("360px");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByTestId("penny-companion-launcher")).toBeTruthy();
  });

  it("shows a new response after background generation completes", async () => {
    thread.running = true;
    const view = render(<PennyCompanion />);
    expect(
      await screen.findByRole("button", { name: "Open Penny, response in progress" }),
    ).toHaveTextContent("Working");

    thread.running = false;
    view.rerender(<PennyCompanion />);
    expect(
      await screen.findByRole("button", { name: "Open Penny, new response" }),
    ).toHaveTextContent("New response");
  });

  it("shows a background generation failure", async () => {
    resume.snapshot = {
      job_id: "job-1",
      conversation_id: "conv-1",
      status: "streaming",
      chunk_count: 1,
      is_finished: false,
    };
    const view = render(<PennyCompanion />);
    expect(
      await screen.findByRole("button", { name: "Open Penny, response in progress" }),
    ).toHaveTextContent("Working");

    resume.snapshot = { ...resume.snapshot, status: "error", is_finished: true };
    view.rerender(<PennyCompanion />);
    expect(
      await screen.findByRole("button", { name: "Open Penny, response failed" }),
    ).toHaveTextContent("Failed");
  });

  it("does not announce a stale failure loaded on initial mount", async () => {
    resume.snapshot = {
      job_id: "old-job",
      conversation_id: "conv-1",
      status: "error",
      chunk_count: 2,
      is_finished: true,
    };
    render(<PennyCompanion />);

    expect(await screen.findByRole("button", { name: "Open Penny" })).not.toHaveTextContent(
      "Failed",
    );
  });

  it("ignores the previous conversation job while switching chats", async () => {
    resume.snapshot = {
      job_id: "job-1",
      conversation_id: "conv-1",
      status: "streaming",
      chunk_count: 1,
      is_finished: false,
    };
    const view = render(<PennyCompanion />);
    expect(
      await screen.findByRole("button", { name: "Open Penny, response in progress" }),
    ).toHaveTextContent("Working");

    workbenchSubject.activeConversationId = "conv-2";
    view.rerender(<PennyCompanion />);
    expect(await screen.findByRole("button", { name: "Open Penny" })).not.toHaveTextContent(
      "Working",
    );
  });

  it("does not duplicate the full chat surface", () => {
    navigation.pathname = "/";
    render(<PennyCompanion />);
    expect(screen.queryByTestId("penny-companion-launcher")).toBeNull();
    expect(screen.queryByTestId("penny-companion")).toBeNull();
  });
});
