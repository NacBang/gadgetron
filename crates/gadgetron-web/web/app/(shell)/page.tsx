"use client";

import {
  ThreadPrimitive,
  MessagePrimitive,
  ComposerPrimitive,
  useComposerRuntime,
  useThread,
  useMessage,
  useThreadViewport,
} from "@assistant-ui/react";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
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

type HistoryBlock =
  | { kind: "text"; text: string }
  | { kind: "reasoning"; text: string }
  | { kind: "tool_use"; name: string; input: unknown }
  | { kind: "tool_result"; tool_use_id: string; content: string };

interface HistoryMsg {
  role: string;
  content: string;
  blocks?: HistoryBlock[];
  ts?: string | null;
}

function PastBlocks({ blocks }: { blocks: HistoryBlock[] }) {
  return (
    <div className="space-y-2">
      {blocks.map((b, i) => {
        if (b.kind === "text") {
          return (
            <div
              key={i}
              className="markdown-body prose prose-invert prose-sm max-w-none"
            >
              <ReactMarkdown remarkPlugins={[remarkGfm]}>{b.text}</ReactMarkdown>
            </div>
          );
        }
        if (b.kind === "reasoning") {
          return (
            <details
              key={i}
              className="rounded border border-zinc-800 bg-zinc-950/50 px-2 py-1 text-[12px] text-zinc-400"
            >
              <summary className="cursor-pointer select-none text-[11px] text-zinc-500">
                🧠 reasoning
              </summary>
              <div className="mt-2 whitespace-pre-wrap font-mono text-[11px]">
                {b.text}
              </div>
            </details>
          );
        }
        if (b.kind === "tool_use") {
          return (
            <details
              key={i}
              className="rounded border border-blue-900/50 bg-blue-950/20 px-2 py-1 text-[12px] text-blue-200"
            >
              <summary className="cursor-pointer select-none text-[11px] text-blue-300">
                🔧 {b.name}
              </summary>
              <pre className="mt-2 overflow-x-auto rounded bg-zinc-950 p-2 text-[11px] text-zinc-300">
                {JSON.stringify(b.input, null, 2)}
              </pre>
            </details>
          );
        }
        if (b.kind === "tool_result") {
          const shortId = (b.tool_use_id || "").slice(0, 8);
          return (
            <details
              key={i}
              className="rounded border border-zinc-800 bg-zinc-950/60 px-2 py-1 text-[12px] text-zinc-300"
            >
              <summary className="cursor-pointer select-none text-[11px] text-zinc-500">
                📦 tool result {shortId && `· ${shortId}`}
              </summary>
              <pre className="mt-2 max-h-64 overflow-auto rounded bg-zinc-950 p-2 text-[11px] text-zinc-300">
                {b.content}
              </pre>
            </details>
          );
        }
        return null;
      })}
    </div>
  );
}

/// Loads the Claude jsonl transcript for the active conversation once
/// per mount. Returns an empty array while loading or when no
/// conversation is active — callers can use `.length === 0` as the
/// "no history" signal. Shared between `<PastMessages>` and the
/// `<HistoryAwareEmpty>` wrapper so both render off the same fetch.
function useActiveHistory(): { past: HistoryMsg[]; err: string | null } {
  const [past, setPast] = useState<HistoryMsg[]>([]);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    const id =
      typeof window !== "undefined"
        ? window.localStorage.getItem("gadgetron_conversation_id")
        : null;
    if (!id) {
      setPast([]);
      return;
    }
    const base =
      document
        .querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]')
        ?.content ?? "/v1";
    const root = base.replace(/\/v\d+$/, "");
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch(
          `${root}/api/v1/web/workbench/conversations/${id}/messages`,
          { credentials: "include" },
        );
        if (!res.ok) return;
        const body = (await res.json()) as { messages: HistoryMsg[] };
        if (!cancelled) setPast(body.messages ?? []);
      } catch (e) {
        if (!cancelled) setErr((e as Error).message);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  return { past, err };
}

/// Wraps `<ThreadPrimitive.Empty>` so the greeting/Ready screen only
/// appears when there's neither live nor historical content — a
/// resumed conversation jumps straight into the past-messages render
/// followed by the date divider, making it feel like a continuous
/// thread instead of a "new chat" welcome landing.
function HistoryAwareEmpty({ children }: { children: ReactNode }) {
  const { past } = useActiveHistory();
  if (past.length > 0) return null;
  return <ThreadPrimitive.Empty>{children}</ThreadPrimitive.Empty>;
}

// A single rendered bubble in the past-conversation transcript.
// Consecutive assistant turns (Claude/Penny splits its reply across
// N turns whenever a tool_use lands — each tool roundtrip starts a new
// assistant message) collapse into ONE assistant group, so a multi-step
// tool-using response renders as one "Penny" bubble instead of N.
type RenderGroup =
  | { kind: "user"; content: string; ts?: string | null }
  | { kind: "assistant"; blocks: HistoryBlock[]; ts?: string | null };

function coalesce(past: HistoryMsg[]): RenderGroup[] {
  const groups: RenderGroup[] = [];
  for (const m of past) {
    const visibleBlocks = (m.blocks ?? []).filter(
      (b) => b.kind === "text" || b.kind === "reasoning",
    );
    const hasPlainContent =
      typeof m.content === "string" && m.content.trim().length > 0;

    if (m.role === "user") {
      // Synth-user turns that only carry tool_result disappear.
      if (visibleBlocks.length === 0 && !hasPlainContent) continue;
      groups.push({ kind: "user", content: m.content ?? "", ts: m.ts });
      continue;
    }

    // Assistant turn: normalize `m.content` fallback into a synthetic
    // text block so downstream rendering only has to handle blocks.
    if (visibleBlocks.length === 0) {
      if (!hasPlainContent) continue;
      visibleBlocks.push({ kind: "text", text: m.content });
    }

    // Fold into the trailing assistant group if the last group is one;
    // otherwise start a new assistant group. Any real user message
    // above has already been pushed, which breaks the run naturally.
    const last = groups[groups.length - 1];
    if (last && last.kind === "assistant") {
      last.blocks.push(...visibleBlocks);
      if (m.ts) last.ts = m.ts;
    } else {
      groups.push({ kind: "assistant", blocks: visibleBlocks, ts: m.ts });
    }
  }
  return groups;
}

function PastMessages() {
  const { past, err } = useActiveHistory();
  const { identity } = useAuth();
  const groups = useMemo(() => coalesce(past), [past]);
  if (groups.length === 0) return null;
  const userLabel =
    identity?.display_name || identity?.email?.split("@")[0] || "You";
  return (
    <div className="mb-2 space-y-3" data-testid="past-messages">
      <div className="text-center text-[10px] uppercase tracking-wider text-zinc-700">
        과거 대화
      </div>
      {err && (
        <div className="rounded border border-red-900/40 bg-red-950/20 px-3 py-1 text-[11px] text-red-400">
          {err}
        </div>
      )}
      {groups.map((g, i) => {
        const isUser = g.kind === "user";
        return (
          <div
            key={i}
            className={`flex items-start gap-3 ${
              isUser ? "justify-end" : ""
            }`}
          >
            {!isUser && (
              <Avatar className="size-7 shrink-0">
                <AvatarImage src="/web/brand/penny.png" alt="Penny" />
                <AvatarFallback className="bg-zinc-800 text-zinc-400 text-[10px] font-bold">
                  P
                </AvatarFallback>
              </Avatar>
            )}
            <div
              className={`flex max-w-[85%] flex-col gap-1 ${
                isUser ? "items-end" : ""
              }`}
            >
              <span className="text-[11px] text-zinc-600">
                {isUser ? userLabel : "Penny"}
              </span>
              <Card
                className={
                  isUser
                    ? "border-transparent bg-primary/80 text-primary-foreground opacity-80 shadow-none"
                    : "border-zinc-800 bg-zinc-900/50 opacity-80 shadow-none"
                }
              >
                <CardContent className="px-4 py-2 text-sm leading-relaxed">
                  {isUser ? (
                    <div className="whitespace-pre-wrap">{g.content}</div>
                  ) : (
                    <PastBlocks blocks={g.blocks} />
                  )}
                </CardContent>
              </Card>
            </div>
            {isUser &&
              (identity?.avatar_url ? (
                // eslint-disable-next-line @next/next/no-img-element
                <img
                  src={identity.avatar_url}
                  alt=""
                  referrerPolicy="no-referrer"
                  className="size-7 shrink-0 rounded-full border border-zinc-700 object-cover"
                />
              ) : (
                <Avatar className="size-7 shrink-0">
                  <AvatarFallback>
                    <User className="size-3.5" />
                  </AvatarFallback>
                </Avatar>
              ))}
          </div>
        );
      })}
      <HistoryDivider lastTs={groups[groups.length - 1]?.ts ?? null} />
    </div>
  );
}

/**
 * Timestamp divider between the read-only past-messages block and the
 * live assistant-ui thread. Renders the last past message's
 * timestamp (locale-formatted), flanked by thin rules — mirrors the
 * date-separator idiom used in Gmail / Slack / KakaoTalk so users
 * immediately read "here's when that conversation happened".
 */
function HistoryDivider({ lastTs }: { lastTs: string | null }) {
  const label = useMemo(() => {
    if (!lastTs) return null;
    const d = new Date(lastTs);
    if (Number.isNaN(d.getTime())) return null;
    const date = d.toLocaleDateString(undefined, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
    const time = d.toLocaleTimeString(undefined, {
      hour: "2-digit",
      minute: "2-digit",
    });
    return `${date} ${time}`;
  }, [lastTs]);
  if (!label) return null;
  return (
    <div
      className="my-3 flex items-center gap-3"
      data-testid="history-divider"
    >
      <span className="h-px flex-1 bg-zinc-800" aria-hidden />
      <span className="font-mono text-[10px] text-zinc-600">{label}</span>
      <span className="h-px flex-1 bg-zinc-800" aria-hidden />
    </div>
  );
}

function ActiveConversationBanner() {
  // Shows the current conversation's title above the thread so
  // switching chats feels tangible. When the row is brand-new (title
  // still "New chat") we suppress the banner so the first turn isn't
  // cluttered. If the conversation has a real title the banner
  // doubles as "you resumed an existing thread — past messages are
  // hidden but Penny still has context" cue.
  const [title, setTitle] = useState<string | null>(null);
  const [turnCount, setTurnCount] = useState(0);
  const thread = useThread((s) => s.messages.length);
  useEffect(() => {
    setTurnCount(thread);
  }, [thread]);
  useEffect(() => {
    const id =
      typeof window !== "undefined"
        ? window.localStorage.getItem("gadgetron_conversation_id")
        : null;
    if (!id) {
      setTitle(null);
      return;
    }
    const base =
      document
        .querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]')
        ?.content ?? "/v1";
    const root = base.replace(/\/v\d+$/, "");
    let cancelled = false;
    (async () => {
      try {
        const res = await fetch(
          `${root}/api/v1/web/workbench/conversations`,
          { credentials: "include" },
        );
        if (!res.ok) return;
        const body = (await res.json()) as {
          conversations: Array<{ id: string; title: string }>;
        };
        const hit = body.conversations.find((c) => c.id === id);
        if (!cancelled) setTitle(hit?.title ?? null);
      } catch {
        // ignore
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);
  if (!title || title === "New chat") return null;
  const resumed = turnCount === 0;
  return (
    <div
      className="shrink-0 border-b border-zinc-800 bg-zinc-900/40 px-4 py-2"
      data-testid="active-conversation-banner"
    >
      <div className="mx-auto flex w-full max-w-[min(1400px,92vw)] items-center gap-2 text-[11px]">
        <span className="text-zinc-500">대화:</span>
        <span className="truncate text-zinc-200" title={title}>
          {title}
        </span>
      </div>
    </div>
  );
}

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
        <ActiveConversationBanner />
        <div className="relative flex flex-1 flex-col overflow-hidden">
          <ThreadPrimitive.Viewport className="penny-scroll flex-1 overflow-y-auto">
            <div className="mx-auto w-full max-w-[min(1400px,92vw)] px-4 py-6">
              <PastMessages />
              <HistoryAwareEmpty>
                <EmptyState />
              </HistoryAwareEmpty>
              <ThreadPrimitive.Messages
                components={{
                  UserMessage,
                  AssistantMessage,
                }}
              />
            </div>
          </ThreadPrimitive.Viewport>
          <BottomTypingIndicator />
          <JumpToLatest />
        </div>

        <div className="border-t border-zinc-800 bg-zinc-950/80 backdrop-blur">
          <div className="mx-auto w-full max-w-[min(1400px,92vw)] p-4">
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

// Suggestion pool — Penny cycles three picks at a time every ~8 s so
// the Ready screen feels alive instead of showing the same three
// prompts forever. Ordered roughly by category (knowledge / operator /
// self-discovery / fleet ops) so a random 3-slice usually covers a
// mix. Add more lines here — they'll join the rotation automatically.
const SUGGESTION_POOL = [
  // Knowledge / wiki
  "매니코어소프트가 어떤 회사인지 조사해서 위키에 정리해줘",
  "위키에 저장된 페이지 전체 목록을 보여줘",
  "GPU 벤치마크 결과를 요약해서 `perf/gpu-bench` 페이지로 정리해줘",
  "회의록 양식을 `meta/meeting-template` 에 작성해줘",
  "최근에 작성된 위키 페이지 3개를 요약해줘",
  // Operator / runbook
  "운영자가 참고할 troubleshooting 런북을 알려줘",
  "NCCL 에러가 났을 때 확인 순서를 정리해줘",
  "오늘자 incident 포스트모템 템플릿을 만들어줘",
  "`runbooks/` 밑에 새 리커버리 절차를 초안으로 써줘",
  // Self-discovery
  "현재 내가 쓸 수 있는 MCP 도구들을 카테고리별로 보여줘",
  "이 워크스페이스에서 Penny 가 할 수 있는 일 5가지 알려줘",
  "`/web/servers` 에 어떤 호스트가 등록돼 있어?",
  // Fleet ops
  "등록된 호스트의 GPU 온도를 한 번에 확인해줘",
  "지난 5분 동안 CPU 가 가장 높았던 서버는?",
  "디스크 사용률 80% 넘는 호스트가 있는지 점검해줘",
  "각 서버의 uptime 을 비교해서 표로 보여줘",
  // Conversation starters
  "지금 gadgetron 클러스터 상태를 한 문장으로 요약해줘",
  "어제 이후 바뀐 wiki 페이지가 있어?",
  "내가 해볼 만한 다음 작업 3개 추천해줘",
  "`/web/dashboard` 수치 중 주목할 만한 게 있나?",
];

const ROTATE_COUNT = 3;
const ROTATE_INTERVAL_MS = 8_000;

function shuffle<T>(arr: readonly T[]): T[] {
  const out = arr.slice();
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [out[i], out[j]] = [out[j], out[i]];
  }
  return out;
}

/** Yield a new 3-slice each tick. Avoids replaying the previous slice
 * verbatim so the rotation always feels fresh. */
function useRotatingSuggestions(): string[] {
  const [slice, setSlice] = useState<string[]>(() =>
    shuffle(SUGGESTION_POOL).slice(0, ROTATE_COUNT),
  );
  useEffect(() => {
    const id = window.setInterval(() => {
      setSlice((prev) => {
        // Find a new slice that differs from the current one by at
        // least one entry — defensive against the pool being smaller
        // than the slice (can happen if someone trims SUGGESTION_POOL).
        for (let attempt = 0; attempt < 5; attempt++) {
          const candidate = shuffle(SUGGESTION_POOL).slice(0, ROTATE_COUNT);
          if (candidate.some((s, i) => prev[i] !== s)) {
            return candidate;
          }
        }
        return prev;
      });
    }, ROTATE_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, []);
  return slice;
}

function EmptyState() {
  const composer = useComposerRuntime();
  const suggestions = useRotatingSuggestions();

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
        {suggestions.map((s) => (
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
  // Only render when the user has scrolled away from the bottom.
  // At the bottom the button is redundant noise — it was visible on
  // every turn even when a new delta had already snapped the viewport
  // down. `useThreadViewport` exposes the live scroll state; this
  // hook re-runs on every scroll event, so the button shows up exactly
  // during back-reading sessions.
  const isAtBottom = useThreadViewport((s) => s.isAtBottom);
  if (isAtBottom) return null;
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

/**
 * Floats a subtle centered "…" pulse at the bottom of the chat viewport
 * while Penny is composing — whether she's still thinking (last message
 * is the user's turn) or mid-stream. Using absolute positioning so it
 * sits just above the composer without pushing any messages around.
 */
function BottomTypingIndicator() {
  const isRunning = useThread((s) => s.isRunning);
  if (!isRunning) return null;
  return (
    <div
      className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center motion-safe:animate-in motion-safe:fade-in duration-200"
      aria-live="polite"
      aria-label="Penny is composing"
      data-testid="bottom-typing-indicator"
    >
      <span className="flex items-center gap-1.5 rounded-full border border-zinc-800/80 bg-zinc-950/80 px-3 py-1.5 shadow-lg backdrop-blur">
        <span
          className="size-1.5 rounded-full bg-blue-400/80 motion-safe:animate-pulse"
          style={{ animationDelay: "-0.3s" }}
        />
        <span
          className="size-1.5 rounded-full bg-blue-400/80 motion-safe:animate-pulse"
          style={{ animationDelay: "-0.15s" }}
        />
        <span className="size-1.5 rounded-full bg-blue-400/80 motion-safe:animate-pulse" />
      </span>
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
  const { identity } = useAuth();
  const label =
    identity?.display_name || identity?.email?.split("@")[0] || "You";
  return (
    <div className="group mb-6 flex items-start justify-end gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300">
      <div className="flex max-w-[85%] flex-col items-end gap-1">
        <span className="text-xs text-muted-foreground">{label}</span>
        <Card className="relative bg-primary text-primary-foreground shadow-sm">
          <CardContent className="px-4 py-2.5 text-sm leading-relaxed whitespace-pre-wrap">
            <MessagePrimitive.Parts />
          </CardContent>
        </Card>
      </div>
      {identity?.avatar_url ? (
        // Plain <img> with `referrerPolicy="no-referrer"` — Google's
        // `lh3.googleusercontent.com` returns 403 when the Referer
        // points at a non-Google origin. AvatarImage's upstream prim
        // silently dropped the prop; use a raw tag to guarantee it
        // reaches the network request.
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={identity.avatar_url}
          alt={label}
          referrerPolicy="no-referrer"
          className="size-8 shrink-0 rounded-full border border-zinc-700 object-cover"
        />
      ) : (
        <Avatar className="size-8 shrink-0">
          <AvatarFallback>
            <User className="size-4" />
          </AvatarFallback>
        </Avatar>
      )}
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
        <AvatarImage src="/web/brand/penny.png" alt="Penny" />
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

  // Preserve the in-progress draft across chat switches / reloads.
  // Keyed by active conversation id so each chat gets its own pending
  // text. Restored on mount; saved on every keystroke via the composer
  // store subscription.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const convId =
      window.localStorage.getItem("gadgetron_conversation_id") ?? "default";
    const storageKey = `gadgetron_draft_${convId}`;
    const saved = window.localStorage.getItem(storageKey);
    if (saved && !composer.getState().text) {
      composer.setText(saved);
    }
    const unsub = composer.subscribe(() => {
      const t = composer.getState().text;
      if (t) window.localStorage.setItem(storageKey, t);
      else window.localStorage.removeItem(storageKey);
    });
    return () => {
      unsub?.();
    };
  }, [composer]);

  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    // Clear any saved draft — the message is heading out.
    if (typeof window !== "undefined") {
      const convId =
        window.localStorage.getItem("gadgetron_conversation_id") ?? "default";
      window.localStorage.removeItem(`gadgetron_draft_${convId}`);
    }
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
