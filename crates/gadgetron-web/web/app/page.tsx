"use client";

import {
  AssistantRuntimeProvider,
  ThreadPrimitive,
  MessagePrimitive,
  ComposerPrimitive,
  useComposerRuntime,
  useThread,
  useMessage,
} from "@assistant-ui/react";
import { useChatRuntime } from "@assistant-ui/react-ai-sdk";
import { useCallback, useEffect, useMemo, useState } from "react";
import {
  SendHorizonal,
  Settings2,
  User,
  CommandIcon,
  Square,
  Copy as CopyIcon,
  Check as CheckIcon,
  Sparkles,
  ChevronDown,
} from "lucide-react";

import { OpenAIChatTransport } from "./openai-transport";
import { MarkdownText } from "./components/markdown-text";
import { ReasoningPart } from "./components/reasoning-part";
import { ToolPart } from "./components/tool-part";
import { SlashHelpDialog } from "./components/slash-help-dialog";
import { SlashAutocomplete } from "./components/slash-autocomplete";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { Card, CardContent } from "./components/ui/card";
import { Avatar, AvatarFallback } from "./components/ui/avatar";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "./components/ui/dialog";

function getApiBase(): string {
  if (typeof document === "undefined") return "/v1";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  return meta?.content || "/v1";
}

function getHealthPath(): string {
  // `/health` is unauthenticated and lives at the gateway root — strip the
  // `/v1` (or whatever the operator customized) suffix from the API base.
  const base = getApiBase();
  return base.replace(/\/v\d+$/, "") + "/health";
}

export default function Home() {
  const [apiKey, setApiKey] = useState<string | null>(null);
  const [keyInput, setKeyInput] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [slashHelpOpen, setSlashHelpOpen] = useState(false);

  useEffect(() => {
    const stored = localStorage.getItem("gadgetron_api_key");
    if (stored) setApiKey(stored);
  }, []);

  const saveKey = () => {
    const k = keyInput.trim();
    if (!k) return;
    localStorage.setItem("gadgetron_api_key", k);
    setApiKey(k);
    setSettingsOpen(false);
    setKeyInput("");
  };

  const clearKey = () => {
    localStorage.removeItem("gadgetron_api_key");
    setApiKey(null);
    setKeyInput("");
  };

  const transport = useMemo(
    () =>
      new OpenAIChatTransport({
        api: `${getApiBase()}/chat/completions`,
        model: "kairos",
        headers: (): Record<string, string> => {
          const key =
            typeof localStorage !== "undefined"
              ? localStorage.getItem("gadgetron_api_key")
              : null;
          return key ? { Authorization: `Bearer ${key}` } : {};
        },
      }),
    [],
  );
  const runtime = useChatRuntime({ transport });

  if (!apiKey) {
    return (
      <div className="flex h-screen flex-col bg-background text-foreground">
        <AppHeader />
        <div className="flex flex-1 items-center justify-center p-6">
          <Card className="w-full max-w-md">
            <CardContent className="flex flex-col gap-4 p-6">
              <div>
                <h2 className="text-lg font-semibold">Gadgetron API 키</h2>
                <p className="mt-1 text-sm text-muted-foreground">
                  <code className="rounded bg-muted px-1 py-0.5 text-xs">
                    gadgetron key create
                  </code>
                  로 발급한 키를 입력하세요. (localStorage에 저장됨)
                </p>
              </div>
              <Input
                type="password"
                value={keyInput}
                onChange={(e) => setKeyInput(e.target.value)}
                placeholder="gad_live_..."
                onKeyDown={(e) => e.key === "Enter" && saveKey()}
                className="font-mono"
              />
              <Button onClick={saveKey} className="w-full">
                시작
              </Button>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  }

  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <SlashHelpDialog open={slashHelpOpen} onOpenChange={setSlashHelpOpen} />
      <div className="flex h-screen flex-col bg-background text-foreground">
        <AppHeader
          onOpenSettings={() => setSettingsOpen(true)}
          settingsOpen={settingsOpen}
          setSettingsOpen={setSettingsOpen}
          onClearKey={clearKey}
          onOpenHelp={() => setSlashHelpOpen(true)}
          insideRuntime
        />

        <ThreadPrimitive.Root className="flex flex-1 flex-col overflow-hidden">
          <div className="relative flex flex-1 flex-col overflow-hidden">
            <ThreadPrimitive.Viewport className="kairos-scroll flex-1 overflow-y-auto">
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

          <div className="border-t border-border/50 bg-background/80 backdrop-blur">
            <div className="mx-auto w-full max-w-3xl p-4">
              <Composer onOpenHelp={() => setSlashHelpOpen(true)} />
              <p className="mt-2 text-center text-[11px] text-muted-foreground">
                <kbd className="rounded border border-border/40 bg-muted/30 px-1 py-0.5 font-mono text-[10px]">
                  /help
                </kbd>{" "}
                를 입력하면 슬래시 명령 목록이 열립니다. Enter로 보내기,
                Shift+Enter로 줄바꿈.
              </p>
            </div>
          </div>
        </ThreadPrimitive.Root>
      </div>
    </AssistantRuntimeProvider>
  );
}

// ---------------------------------------------------------------------------

/**
 * Live pulse shown in the header while the thread is streaming. Bridges
 * the gap until we thread real tokens-per-second metrics through the
 * stream (part of the C8 structured-stream refactor). Today it answers
 * the simpler question of "is the agent actually working right now?"
 * from anywhere in the viewport — no need to scroll back to see the
 * per-message caret.
 */
function ActiveTaskIndicator() {
  const isRunning = useThread((s) => s.isRunning);
  if (!isRunning) return null;
  return (
    <span className="flex items-center gap-1.5 rounded-full border border-blue-400/30 bg-blue-500/10 px-2 py-0.5 text-[10px] uppercase tracking-wider text-blue-300 motion-safe:animate-in motion-safe:fade-in duration-200">
      <span className="size-1.5 rounded-full bg-blue-400 motion-safe:animate-pulse" />
      응답 중
    </span>
  );
}

function ServerStatus() {
  const [healthy, setHealthy] = useState<boolean | null>(null);
  useEffect(() => {
    let cancelled = false;
    const check = async () => {
      try {
        const r = await fetch(getHealthPath(), { cache: "no-store" });
        if (!cancelled) setHealthy(r.ok);
      } catch {
        if (!cancelled) setHealthy(false);
      }
    };
    void check();
    const iv = setInterval(check, 5000);
    return () => {
      cancelled = true;
      clearInterval(iv);
    };
  }, []);

  const color =
    healthy === null
      ? "bg-muted-foreground/50"
      : healthy
        ? "bg-emerald-500 motion-safe:animate-pulse"
        : "bg-red-500";
  const label =
    healthy === null ? "확인 중" : healthy ? "연결됨" : "연결 끊김";
  return (
    <span
      className="flex items-center gap-1.5 rounded-full border border-border/40 bg-muted/20 px-2 py-0.5 text-[10px] uppercase tracking-wider text-muted-foreground"
      title={`게이트웨이 상태: ${label}`}
    >
      <span className={`size-1.5 rounded-full ${color}`} />
      <span>{label}</span>
    </span>
  );
}

function AppHeader({
  onOpenSettings,
  settingsOpen,
  setSettingsOpen,
  onClearKey,
  onOpenHelp,
  insideRuntime = false,
}: {
  onOpenSettings?: () => void;
  settingsOpen?: boolean;
  setSettingsOpen?: (v: boolean) => void;
  onClearKey?: () => void;
  onOpenHelp?: () => void;
  /** True when this header is rendered inside an `AssistantRuntimeProvider`.
   * Gates hooks like `useThread` that require the runtime to be present —
   * the login screen renders the header OUTSIDE the provider for brand
   * consistency and calling `useThread` there aborts SSG. */
  insideRuntime?: boolean;
}) {
  const showSettings = !!onOpenSettings;
  return (
    <header className="flex h-14 shrink-0 items-center justify-between border-b border-border/50 bg-background/80 px-6 backdrop-blur">
      <div className="flex items-center gap-2">
        <div className="size-7 rounded-lg bg-gradient-to-br from-blue-500 to-purple-500 shadow-[0_0_20px_-6px_rgba(139,92,246,0.5)]" />
        <span className="font-semibold tracking-tight">Kairos</span>
        <span className="hidden text-xs text-muted-foreground md:inline">
          · Gadgetron의 AI 에이전트
        </span>
      </div>
      <div className="flex items-center gap-2">
        {insideRuntime && <ActiveTaskIndicator />}
        <ServerStatus />
        {onOpenHelp && (
          <Button variant="ghost" size="sm" onClick={onOpenHelp}>
            <CommandIcon className="size-4" />
            명령
          </Button>
        )}
        {showSettings && (
          <>
            <Button variant="ghost" size="sm" onClick={onOpenSettings}>
              <Settings2 className="size-4" />
              설정
            </Button>
            <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
              <DialogContent>
                <DialogHeader>
                  <DialogTitle>설정</DialogTitle>
                  <DialogDescription>
                    API 키는 브라우저의 localStorage에만 저장됩니다.
                  </DialogDescription>
                </DialogHeader>
                <div className="flex flex-col gap-2 py-4">
                  <p className="text-sm">현재 세션: 로그인됨</p>
                </div>
                <DialogFooter>
                  <Button variant="destructive" onClick={onClearKey}>
                    API 키 지우기 (로그아웃)
                  </Button>
                </DialogFooter>
              </DialogContent>
            </Dialog>
          </>
        )}
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
      // Tiny delay lets the input update before we fire send() — avoids
      // a race where send() reads a stale empty state.
      setTimeout(() => composer.send(), 0);
    },
    [composer],
  );

  return (
    <div className="mx-auto flex h-[70vh] max-w-xl flex-col items-center justify-center gap-6 text-center">
      <div className="relative">
        <div className="size-14 rounded-2xl bg-gradient-to-br from-blue-500 to-purple-500 shadow-[0_0_40px_-10px_rgba(139,92,246,0.6)]" />
        <Sparkles className="absolute -right-2 -top-2 size-5 text-amber-300/90 motion-safe:animate-pulse" />
      </div>
      <div className="flex flex-col gap-1.5">
        <h1 className="text-2xl font-semibold tracking-tight">
          무엇을 도와드릴까요?
        </h1>
        <p className="max-w-md text-sm text-muted-foreground">
          저장·검색·조사는 모두 Kairos가 알아서 합니다. 대화는 내부 위키에
          자동으로 커밋됩니다.
        </p>
      </div>
      <div className="flex w-full flex-col gap-2 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300">
        {SUGGESTIONS.map((s) => (
          <button
            key={s}
            onClick={() => pick(s)}
            className="group flex items-center gap-2 rounded-xl border border-border/50 bg-card/60 px-4 py-3 text-left text-sm transition-all hover:border-blue-500/40 hover:bg-card hover:shadow-[0_0_20px_-10px_rgba(59,130,246,0.4)]"
          >
            <span className="flex-1">{s}</span>
            <span className="text-xs text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100">
              Enter ↵
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

/**
 * Floating "↓ 최신으로" button shown only when the user has scrolled
 * away from the tail of the transcript. `ThreadPrimitive.ScrollToBottom`
 * handles the "at bottom" detection + click wiring for us — we just
 * style the host element and let the primitive decide whether to render.
 */
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
  // The Thread layer already renders a live assistant bubble while streaming,
  // so this bubble only needs to bridge the gap between the user hitting
  // Enter and the assistant's first `text-start` chunk. We gate on
  // `isRunning` and the last message being from the user.
  const lastIsUser = useThread(
    (s) => s.messages[s.messages.length - 1]?.role === "user",
  );
  if (!isRunning || !lastIsUser) return null;
  return (
    <div className="mb-6 flex items-start gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-1 duration-200">
      <Avatar className="size-8 shrink-0 ring-2 ring-blue-500/30 motion-safe:animate-pulse">
        <AvatarFallback className="bg-gradient-to-br from-blue-500 to-purple-500 text-white text-xs font-bold">
          K
        </AvatarFallback>
      </Avatar>
      <Card className="bg-card/80">
        <CardContent className="flex items-center gap-1.5 px-4 py-3">
          <span className="size-1.5 rounded-full bg-muted-foreground/70 motion-safe:animate-bounce [animation-delay:-0.3s]" />
          <span className="size-1.5 rounded-full bg-muted-foreground/70 motion-safe:animate-bounce [animation-delay:-0.15s]" />
          <span className="size-1.5 rounded-full bg-muted-foreground/70 motion-safe:animate-bounce" />
        </CardContent>
      </Card>
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
      <Avatar className="size-8 shrink-0">
        <AvatarFallback className="bg-gradient-to-br from-blue-500 to-purple-500 text-white text-xs font-bold">
          K
        </AvatarFallback>
      </Avatar>
      <div className="flex max-w-[85%] flex-col gap-1">
        <div className="flex items-center gap-2">
          <span className="text-xs text-muted-foreground">Kairos</span>
          <AssistantStatusBadge />
        </div>
        <Card className="relative bg-card/70 ring-1 ring-border/60 shadow-sm transition-colors hover:bg-card/90">
          <CopyMessageButton />
          <CardContent className="px-4 py-2.5 text-sm">
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

/**
 * Tiny label next to "Kairos" that calls out incomplete messages — a user
 * who hits the Stop button mid-stream should see *why* the response is
 * partial, not silent truncation. Also surfaces length/content-filter
 * cutoffs if the underlying runtime ever reports them.
 */
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
      className="relative flex items-end gap-2 rounded-2xl border border-border/70 bg-card/60 p-2 shadow-sm transition-all focus-within:border-blue-500/50 focus-within:ring-2 focus-within:ring-blue-500/20"
    >
      <SlashAutocomplete onLocalExecute={executeLocalCommand} />
      <ComposerPrimitive.Input
        placeholder="메시지를 입력하세요... (/ 로 명령 자동완성)"
        rows={1}
        autoFocus
        className="max-h-40 min-h-[2.5rem] flex-1 resize-none bg-transparent px-2 py-1.5 text-sm outline-none placeholder:text-muted-foreground"
      />
      {isRunning ? (
        <ComposerPrimitive.Cancel asChild>
          <Button
            size="icon"
            variant="destructive"
            className="size-9 shrink-0"
            aria-label="생성 중지"
          >
            <Square className="size-3.5 fill-current" />
          </Button>
        </ComposerPrimitive.Cancel>
      ) : (
        <ComposerPrimitive.Send asChild>
          <Button
            size="icon"
            className="size-9 shrink-0 bg-gradient-to-br from-blue-500 to-purple-500 text-white shadow-sm transition-transform hover:scale-[1.02] hover:shadow-[0_0_20px_-6px_rgba(139,92,246,0.6)]"
            aria-label="전송"
          >
            <SendHorizonal className="size-4" />
          </Button>
        </ComposerPrimitive.Send>
      )}
    </ComposerPrimitive.Root>
  );
}
