"use client";

import {
  ComposerPrimitive,
  MessagePrimitive,
  ThreadPrimitive,
  useComposerRuntime,
  useThread,
} from "@assistant-ui/react";
import { SendHorizontal, Square } from "lucide-react";
import { useCallback, useEffect, type FormEvent } from "react";
import { flushSync } from "react-dom";
import { toast } from "sonner";

import { MarkdownText } from "@/components/markdown-text";
import { ReasoningPart } from "@/components/reasoning-part";
import { ToolPart } from "@/components/tool-part";
import { Button } from "@/components/ui/button";
import { cancelActiveConversationJob } from "@/lib/chat-resume";
import { getActiveConversationId } from "@/lib/conversation-id";
import {
  useWorkbenchSubject,
  withSubjectContext,
} from "@/lib/workbench-subject-context";
import {
  useWorkbenchPageContext,
  withWorkbenchPageContext,
  type WorkbenchPageContextSnapshot,
} from "@/lib/workbench-page-context";
import { PennyAvatar } from "./penny-avatar";
import { ChatAttachmentTray } from "./chat-attachment-tray";

export type PageContextPart =
  | "workspace"
  | "selection"
  | "filters"
  | "timeRange";

function omitPageContextParts(
  snapshot: WorkbenchPageContextSnapshot,
  omitted: PageContextPart[],
): WorkbenchPageContextSnapshot {
  const hidden = new Set(omitted);
  return {
    page: snapshot.page,
    workspace: hidden.has("workspace") ? undefined : snapshot.workspace,
    selection: hidden.has("selection") ? undefined : snapshot.selection,
    filters: hidden.has("filters") ? undefined : snapshot.filters,
    timeRange: hidden.has("timeRange") ? undefined : snapshot.timeRange,
  };
}

function UserText({ text }: { text: string }) {
  const contextStart = text.indexOf("Current screen context:\n");
  const questionMarker = "\n\nQuestion: ";
  const questionStart = text.indexOf(questionMarker, contextStart);
  if (contextStart < 0 || questionStart < 0) {
    return <p>{text}</p>;
  }
  const context = text.slice(contextStart, questionStart);
  const page = context.match(/^- Page: (.+)$/m)?.[1] ?? "Current screen";
  const workspace = context
    .match(/^- Workspace: (.+)$/m)?.[1]
    ?.replace(/\s+\([^)]+\)$/, "");
  return (
    <div className="space-y-1.5">
      <span className="inline-flex rounded border border-[var(--line)] bg-[var(--bg)] px-1.5 py-0.5 text-[11px] font-medium text-[var(--ink-2)]">
        Context · {workspace ?? page}
      </span>
      <p>{text.slice(questionStart + questionMarker.length)}</p>
    </div>
  );
}

function UserMessage() {
  return (
    <div className="mb-4 flex justify-end" data-testid="penny-user-message">
      <div className="max-w-[88%] whitespace-pre-wrap rounded border border-[var(--copper)] bg-[var(--surface-2)] px-3 py-2 text-sm leading-relaxed text-[var(--ink)]">
        <MessagePrimitive.Parts components={{ Text: UserText }} />
      </div>
    </div>
  );
}

function AssistantMessage() {
  return (
    <div className="mb-4 flex items-start gap-2" data-testid="penny-assistant-message">
      <PennyAvatar className="mt-0.5 size-7" />
      <div className="min-w-0 max-w-[88%] rounded border border-[var(--line)] bg-[var(--surface)] px-3 py-2 text-sm text-[var(--ink)]">
        <MessagePrimitive.Parts
          components={{
            Text: MarkdownText,
            Reasoning: ReasoningPart,
            tools: { Fallback: ToolPart },
          }}
        />
      </div>
    </div>
  );
}

function Composer({
  contextEnabled,
  omittedContextParts,
}: {
  contextEnabled: boolean;
  omittedContextParts: PageContextPart[];
}) {
  const composer = useComposerRuntime();
  const isRunning = useThread((state) => state.isRunning);
  const messageCount = useThread((state) => state.messages.length);
  const pageContext = useWorkbenchPageContext();
  const { activeConversationId, refreshSubject } = useWorkbenchSubject();

  useEffect(() => {
    if (typeof window === "undefined") return;
    const conversationId = activeConversationId ?? getActiveConversationId();
    if (!conversationId) return;
    refreshSubject();
    const key = `gadgetron_draft_${conversationId}`;
    const saved = window.localStorage.getItem(key);
    if (saved && !composer.getState().text) composer.setText(saved);
    const unsubscribe = composer.subscribe(() => {
      const text = composer.getState().text;
      if (text) window.localStorage.setItem(key, text);
      else window.localStorage.removeItem(key);
    });
    return () => unsubscribe?.();
  }, [activeConversationId, composer, refreshSubject]);

  const stop = useCallback(() => {
    const conversationId = getActiveConversationId();
    if (!conversationId) return;
    void cancelActiveConversationJob(conversationId).catch(() => {
      toast.error("Stop failed", {
        description: "Generation may still be running. Try Stop again.",
      });
    });
  }, []);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const text = composer.getState().text.trim();
    if (!text) return;
    let outgoing = text;
    if (!text.startsWith("/") && contextEnabled) {
      outgoing = withWorkbenchPageContext(
        outgoing,
        omitPageContextParts(pageContext, omittedContextParts),
      );
      if (messageCount === 0) outgoing = withSubjectContext(outgoing);
    }
    if (outgoing !== text) {
      flushSync(() => composer.setText(outgoing));
    }
    const conversationId = getActiveConversationId();
    if (conversationId) {
      window.localStorage.removeItem(`gadgetron_draft_${conversationId}`);
    }
    composer.send();
  };

  return (
    <ComposerPrimitive.Root
      onSubmit={submit}
      className="flex flex-col border-t border-zinc-800 bg-zinc-950 p-3"
    >
      <ChatAttachmentTray conversationId={activeConversationId ?? getActiveConversationId()} />
      <div className="flex items-end gap-2">
        <ComposerPrimitive.Input
          autoFocus
          rows={1}
          placeholder={contextEnabled ? "Ask Penny about this screen" : "Ask Penny"}
          className="max-h-32 min-h-10 flex-1 resize-none rounded border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm text-zinc-100 outline-none focus:border-zinc-500 placeholder:text-zinc-600"
        />
        {isRunning ? (
          <ComposerPrimitive.Cancel asChild>
            <Button
              size="icon"
              variant="destructive"
              className="size-10 shrink-0"
              aria-label="Stop generation"
              onClick={stop}
            >
              <Square className="size-4 fill-current" />
            </Button>
          </ComposerPrimitive.Cancel>
        ) : (
          <Button
            type="submit"
            size="icon"
            className="size-10 shrink-0 bg-[var(--copper)] text-[var(--bg)] hover:bg-[var(--copper-hi)]"
            aria-label="Send"
          >
            <SendHorizontal className="size-4" />
          </Button>
        )}
      </div>
    </ComposerPrimitive.Root>
  );
}

export function PennyCompanionThread({
  contextEnabled,
  omittedContextParts,
}: {
  contextEnabled: boolean;
  omittedContextParts: PageContextPart[];
}) {
  const pageContext = useWorkbenchPageContext();
  const { subject } = useWorkbenchSubject();
  return (
    <ThreadPrimitive.Root className="flex min-h-0 flex-1 flex-col">
      <ThreadPrimitive.Viewport className="penny-scroll min-h-0 flex-1 overflow-y-auto px-4 py-4">
        <ThreadPrimitive.Empty>
          <div className="flex min-h-52 flex-col items-center justify-center gap-2 text-center">
            <PennyAvatar size="lg" />
            <p className="text-sm font-medium text-zinc-200">
              {contextEnabled
                ? `Ask about ${subject?.title ?? pageContext.page.title}`
                : "Start a general conversation"}
            </p>
          </div>
        </ThreadPrimitive.Empty>
        <ThreadPrimitive.Messages
          components={{ UserMessage, AssistantMessage }}
        />
      </ThreadPrimitive.Viewport>
      <Composer
        contextEnabled={contextEnabled}
        omittedContextParts={omittedContextParts}
      />
    </ThreadPrimitive.Root>
  );
}
