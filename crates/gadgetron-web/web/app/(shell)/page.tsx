"use client";

import {
  ThreadPrimitive,
  MessagePrimitive,
  ComposerPrimitive,
  useComposerRuntime,
  useThread,
  useMessage,
} from "@assistant-ui/react";
import { useCallback, useState } from "react";
import {
  SendHorizonal,
  Settings2,
  User,
  CommandIcon,
  Square,
  Copy as CopyIcon,
  Check as CheckIcon,
  ChevronDown,
} from "lucide-react";

import { MarkdownText } from "../components/markdown-text";
import { ReasoningPart } from "../components/reasoning-part";
import { ToolPart } from "../components/tool-part";
import { SlashHelpDialog } from "../components/slash-help-dialog";
import { SlashAutocomplete } from "../components/slash-autocomplete";
import { Button } from "../components/ui/button";
import { Card, CardContent } from "../components/ui/card";
import { Avatar, AvatarFallback, AvatarImage } from "../components/ui/avatar";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "../components/ui/dialog";
import { useAuth } from "../lib/auth-context";

// ---------------------------------------------------------------------------
// /web — Chat page. Runs inside `(shell)/layout.tsx`, which owns the
// `WorkbenchShell` chrome + `AssistantRuntimeProvider`. This component
// only emits the chat-specific header + thread viewport + composer —
// everything above (plugs strip, left rail) and beside (evidence pane)
// is the layout's job.
// ---------------------------------------------------------------------------

export default function Home() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [slashHelpOpen, setSlashHelpOpen] = useState(false);
  const { clearKey } = useAuth();

  return (
    <>
      <SlashHelpDialog open={slashHelpOpen} onOpenChange={setSlashHelpOpen} />
      <ChatHeader
        onOpenSettings={() => setSettingsOpen(true)}
        settingsOpen={settingsOpen}
        setSettingsOpen={setSettingsOpen}
        onClearKey={clearKey}
        onOpenHelp={() => setSlashHelpOpen(true)}
      />

      <ThreadPrimitive.Root className="flex flex-1 flex-col overflow-hidden">
        <div className="relative flex flex-1 flex-col overflow-hidden">
          <ThreadPrimitive.Viewport className="penny-scroll flex-1 overflow-y-auto">
            <div className="mx-auto w-full max-w-3xl px-4 py-6">
              <ThreadPrimitive.Empty>
                <EmptyState />
              </ThreadPrimitive.Empty>
              <ThreadPrimitive.Messages
                components={{
                  UserMessage,
                  AssistantMessage,
                }}
              />
              <ThreadPendingIndicator />
            </div>
          </ThreadPrimitive.Viewport>
          <JumpToLatest />
        </div>

        <div className="border-t border-zinc-800 bg-zinc-950/80 backdrop-blur">
          <div className="mx-auto w-full max-w-3xl p-4">
            <Composer onOpenHelp={() => setSlashHelpOpen(true)} />
            <p className="mt-2 text-center text-[11px] text-zinc-600">
              <kbd className="rounded border border-zinc-800 bg-zinc-900/50 px-1 py-0.5 font-mono text-[10px]">
                /help
              </kbd>{" "}
              to list slash commands. Enter to send, Shift+Enter for newline.
            </p>
          </div>
        </div>
      </ThreadPrimitive.Root>
    </>
  );
}

// ---------------------------------------------------------------------------

function ActiveTaskIndicator() {
  const isRunning = useThread((s) => s.isRunning);
  if (!isRunning) return null;
  return (
    <span className="flex items-center gap-1 rounded border border-blue-900/50 bg-blue-900/20 px-1.5 py-0.5 font-mono text-[10px] text-blue-400 motion-safe:animate-in motion-safe:fade-in duration-200">
      <span className="size-1.5 rounded-full bg-blue-400 motion-safe:animate-pulse" />
      running
    </span>
  );
}

function ChatHeader({
  onOpenSettings,
  settingsOpen,
  setSettingsOpen,
  onClearKey,
  onOpenHelp,
}: {
  onOpenSettings: () => void;
  settingsOpen: boolean;
  setSettingsOpen: (v: boolean) => void;
  onClearKey: () => void;
  onOpenHelp: () => void;
}) {
  return (
    <header
      className="flex h-9 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4"
      data-testid="chat-header"
    >
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-zinc-300">Penny</span>
        <span className="hidden text-[11px] text-zinc-600 md:inline">
          · Gadgetron knowledge workbench
        </span>
      </div>
      <div className="flex items-center gap-1.5">
        <ActiveTaskIndicator />
        <Button
          variant="ghost"
          size="sm"
          onClick={onOpenHelp}
          className="h-6 gap-1 px-2 text-[11px] text-zinc-500 hover:text-zinc-300"
        >
          <CommandIcon className="size-3" />
          Commands
        </Button>
        <Button
          variant="ghost"
          size="sm"
          onClick={onOpenSettings}
          className="h-6 gap-1 px-2 text-[11px] text-zinc-500 hover:text-zinc-300"
        >
          <Settings2 className="size-3" />
          Settings
        </Button>
        <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Settings</DialogTitle>
              <DialogDescription>
                API key is stored in browser localStorage only.
              </DialogDescription>
            </DialogHeader>
            <div className="flex flex-col gap-2 py-4">
              <p className="text-sm text-zinc-400">Current session: signed in</p>
            </div>
            <DialogFooter>
              <Button variant="destructive" onClick={onClearKey}>
                Clear API key (sign out)
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </header>
  );
}

const SUGGESTIONS = [
  "매니코어소프트가 어떤 회사인지 조사해서 위키에 정리해줘",
  "위키에 저장된 페이지 전체 목록을 보여줘",
  "운영자가 참고할 troubleshooting 런북을 알려줘",
];

function EmptyState() {
  const composer = useComposerRuntime();

  const pick = useCallback(
    (text: string) => {
      composer.setText(text);
      setTimeout(() => composer.send(), 0);
    },
    [composer],
  );

  return (
    <div
      className="mx-auto flex h-[70vh] max-w-xl flex-col items-center justify-center gap-6"
      data-testid="chat-empty-state"
    >
      <div className="flex flex-col gap-1.5">
        <h1 className="text-sm font-medium text-zinc-300">Ready</h1>
        <p className="text-xs text-zinc-500">
          Send a message to Penny, or import a wiki page via{" "}
          <code className="rounded bg-zinc-900 px-1 py-0.5 font-mono text-[11px] text-zinc-400">
            wiki.import
          </code>{" "}
          gadget.
        </p>
      </div>
      <div className="flex w-full flex-col gap-1.5">
        {SUGGESTIONS.map((s) => (
          <button
            key={s}
            onClick={() => pick(s)}
            className="group flex items-center gap-2 rounded border border-zinc-800 bg-zinc-900/50 px-3 py-2.5 text-left text-xs text-zinc-400 transition-colors hover:border-zinc-700 hover:bg-zinc-900 hover:text-zinc-300"
          >
            <span className="flex-1">{s}</span>
            <span className="text-[11px] text-zinc-600 opacity-0 transition-opacity group-hover:opacity-100">
              Enter ↵
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function JumpToLatest() {
  return (
    <ThreadPrimitive.ScrollToBottom asChild>
      <button
        type="button"
        aria-label="최신 메시지로 이동"
        className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-border/60 bg-background/85 px-3 py-1.5 text-xs font-medium text-foreground/90 shadow-lg backdrop-blur transition-all hover:bg-background hover:text-foreground motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-1 duration-200"
      >
        <ChevronDown className="size-3.5" />
        최신으로
      </button>
    </ThreadPrimitive.ScrollToBottom>
  );
}

function ThreadPendingIndicator() {
  const isRunning = useThread((s) => s.isRunning);
  const lastIsUser = useThread(
    (s) => s.messages[s.messages.length - 1]?.role === "user",
  );
  if (!isRunning || !lastIsUser) return null;
  return (
    <div className="mb-6 flex items-start gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-1 duration-200">
      <Avatar className="size-7 shrink-0">
        <AvatarFallback className="bg-zinc-800 text-zinc-400 text-[10px] font-bold">
          P
        </AvatarFallback>
      </Avatar>
      <div className="flex items-center gap-1 rounded border border-zinc-800 bg-zinc-900 px-3 py-2.5">
        <span className="size-1.5 rounded-full bg-zinc-600 motion-safe:animate-bounce [animation-delay:-0.3s]" />
        <span className="size-1.5 rounded-full bg-zinc-600 motion-safe:animate-bounce [animation-delay:-0.15s]" />
        <span className="size-1.5 rounded-full bg-zinc-600 motion-safe:animate-bounce" />
      </div>
    </div>
  );
}

function extractMessageText(content: unknown): string {
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const p of content as Array<{ type: string; text?: string }>) {
    if (p?.type === "text" && typeof p.text === "string") parts.push(p.text);
  }
  return parts.join("");
}

function CopyMessageButton() {
  const content = useMessage((s) => s.content);
  const [copied, setCopied] = useState(false);
  const onCopy = async () => {
    const text = extractMessageText(content);
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API blocked by insecure context or permissions — swallow.
    }
  };
  return (
    <button
      type="button"
      aria-label="메시지 복사"
      onClick={onCopy}
      className="absolute right-2 top-2 flex size-7 items-center justify-center rounded-md border border-border/40 bg-background/80 text-muted-foreground opacity-0 shadow-sm transition-opacity hover:text-foreground group-hover:opacity-100"
    >
      {copied ? (
        <CheckIcon className="size-3.5 text-emerald-400" />
      ) : (
        <CopyIcon className="size-3.5" />
      )}
    </button>
  );
}

function UserMessage() {
  return (
    <div className="group mb-6 flex items-start justify-end gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300">
      <div className="flex max-w-[85%] flex-col items-end gap-1">
        <span className="text-xs text-muted-foreground">You</span>
        <Card className="relative bg-primary text-primary-foreground shadow-sm">
          <CardContent className="px-4 py-2.5 text-sm leading-relaxed whitespace-pre-wrap">
            <MessagePrimitive.Parts />
          </CardContent>
        </Card>
      </div>
      <Avatar className="size-8 shrink-0">
        <AvatarFallback>
          <User className="size-4" />
        </AvatarFallback>
      </Avatar>
    </div>
  );
}

function AssistantMessage() {
  return (
    <div className="group mb-6 flex items-start gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300">
      <Avatar className="size-7 shrink-0">
        {/* Drop a real Penny portrait into
         * `crates/gadgetron-web/web/public/brand/penny.png` (or .svg)
         * to override the placeholder. The fallback renders a 'P' if
         * the file is missing or the request fails. */}
        <AvatarImage src="/web/brand/penny.svg" alt="Penny" />
        <AvatarFallback className="bg-zinc-800 text-zinc-400 text-[10px] font-bold">
          P
        </AvatarFallback>
      </Avatar>
      <div className="flex max-w-[85%] flex-col gap-1">
        <div className="flex items-center gap-2">
          <span className="text-[11px] text-zinc-500">Penny</span>
          <AssistantStatusBadge />
        </div>
        <Card className="relative border-zinc-800 bg-zinc-900/70 shadow-none transition-colors hover:bg-zinc-900">
          <CopyMessageButton />
          <CardContent className="px-4 py-2.5 text-sm text-zinc-200">
            <MessagePrimitive.Parts
              components={{
                Text: MarkdownText,
                Reasoning: ReasoningPart,
                tools: {
                  Fallback: ToolPart,
                },
              }}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function AssistantStatusBadge() {
  const status = useMessage((s) => s.status);
  if (!status || status.type !== "incomplete") return null;
  const reason = status.reason;
  const labelMap: Record<string, { text: string; tint: string }> = {
    cancelled: {
      text: "중지됨",
      tint: "text-amber-300/90 border-amber-400/30 bg-amber-400/10",
    },
    length: {
      text: "길이 제한",
      tint: "text-sky-300/90 border-sky-400/30 bg-sky-400/10",
    },
    "content-filter": {
      text: "필터 차단",
      tint: "text-red-300/90 border-red-400/30 bg-red-400/10",
    },
    "tool-calls": {
      text: "도구 보류",
      tint: "text-blue-300/90 border-blue-400/30 bg-blue-400/10",
    },
    error: {
      text: "오류 종료",
      tint: "text-red-300/90 border-red-400/30 bg-red-400/10",
    },
    other: {
      text: "조기 종료",
      tint: "text-muted-foreground border-border/60 bg-muted/40",
    },
  };
  const info = labelMap[reason] ?? labelMap.other;
  return (
    <span
      className={`rounded-full border px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider ${info.tint}`}
      title={`message.status: incomplete (${reason})`}
    >
      {info.text}
    </span>
  );
}

function Composer({ onOpenHelp }: { onOpenHelp: () => void }) {
  const composer = useComposerRuntime();
  const isRunning = useThread((s) => s.isRunning);

  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    const state = composer.getState();
    const text = state.text.trim();
    if (text === "/help") {
      e.preventDefault();
      e.stopPropagation();
      composer.setText("");
      onOpenHelp();
      return;
    }
    if (text === "/clear") {
      e.preventDefault();
      e.stopPropagation();
      composer.setText("");
      if (typeof location !== "undefined") location.reload();
      return;
    }
  };

  const executeLocalCommand = (cmd: "/help" | "/clear") => {
    if (cmd === "/help") {
      onOpenHelp();
    } else if (cmd === "/clear") {
      if (typeof location !== "undefined") location.reload();
    }
  };

  return (
    <ComposerPrimitive.Root
      onSubmit={handleSubmit}
      className="relative flex items-end gap-2 rounded border border-zinc-700 bg-zinc-900 p-2 transition-colors focus-within:border-zinc-600 focus-within:bg-zinc-900"
    >
      <SlashAutocomplete onLocalExecute={executeLocalCommand} />
      <ComposerPrimitive.Input
        placeholder="질문하거나 /command 를 입력하세요"
        rows={1}
        autoFocus
        className="max-h-40 min-h-[2.5rem] flex-1 resize-none bg-transparent px-2 py-1.5 text-sm text-zinc-200 outline-none placeholder:text-zinc-600"
      />
      {isRunning ? (
        <ComposerPrimitive.Cancel asChild>
          <Button
            size="icon"
            variant="destructive"
            className="size-8 shrink-0"
            aria-label="Stop generation"
          >
            <Square className="size-3.5 fill-current" />
          </Button>
        </ComposerPrimitive.Cancel>
      ) : (
        <ComposerPrimitive.Send asChild>
          <Button
            size="icon"
            className="size-8 shrink-0 bg-blue-600 text-white hover:bg-blue-500"
            aria-label="Send"
          >
            <SendHorizonal className="size-4" />
          </Button>
        </ComposerPrimitive.Send>
      )}
    </ComposerPrimitive.Root>
  );
}
