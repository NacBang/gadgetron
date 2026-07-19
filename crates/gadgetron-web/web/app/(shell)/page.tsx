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
import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import {
  SendHorizonal,
  User,
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
import { ChatAttachmentTray } from "../components/chat/chat-attachment-tray";
import { authHeaders, useAuth } from "../lib/auth-context";
import { WikiPagesProvider } from "../lib/wiki-link";
import {
  ACTIVE_CONVERSATION_EVENT,
  getActiveConversationId,
  setActiveConversationId,
} from "../lib/conversation-id";
import {
  useWorkbenchSubject,
  withSubjectContext,
} from "../lib/workbench-subject-context";
import { getApiBase } from "../lib/workbench-client";
import { toast } from "sonner";
import { useConfirm } from "../components/ui/confirm";
import { safeRandomUUID } from "../lib/uuid";
import { cancelActiveConversationJob } from "../lib/chat-resume";
import {
  AGENT_MODEL_OPTIONS,
  agentEffortOptions,
  cacheConversationAgentProfile,
  getConversationAgentProfile,
  listAvailableLlmEndpointModels,
  modelOptionKey,
  normalizeAgentEffort,
  updateConversationAgentProfile,
  type AgentEffort,
  type AvailableLlmEndpointModel,
  type ConversationAgentProfile,
} from "../lib/agent-profile";
import { useI18n } from "../lib/i18n";

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

interface ActiveHistoryState {
  past: HistoryMsg[];
  err: string | null;
}

interface PennyBrainSettings {
  mode?: string;
  external_base_url?: string;
  model?: string;
  // High-level admin axes. Older API responses may
  // omit these — the summary fn falls back to the raw fields above.
  backend?: "claude_code" | "codex_exec";
  agent?: "claude_code" | "codex_exec";
  model_source?: "default" | "local";
  local_base_url?: string;
  effort?: "auto" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
}

function endpointHost(baseUrl: string | undefined): string {
  const trimmed = (baseUrl ?? "").trim();
  if (!trimmed) return "";
  try {
    return new URL(trimmed).host;
  } catch {
    return trimmed.replace(/^https?:\/\//, "").split("/")[0] ?? trimmed;
  }
}

function pennyBackendSummary(settings: PennyBrainSettings): string {
  // Prefer the new high-level axes (admin "Agent Backend" + "Model"). Falls
  // back to the raw `mode`/`model` projection so old backends keep
  // working.
  const backend = settings.backend ?? settings.agent;
  const agentLabel =
    backend === "codex_exec"
      ? "Codex"
      : backend === "claude_code"
        ? "Claude"
        : settings.mode?.trim() || "Penny";
  const effort = settings.effort ?? "";
  const isLocal = settings.model_source === "local";
  const explicitModel = settings.model?.trim() ?? "";
  const localHost =
    endpointHost(settings.local_base_url) || endpointHost(settings.external_base_url);

  // model segment: explicit override > local host > "default"
  const modelLabel = explicitModel
    ? isLocal && localHost
      ? `${explicitModel} @ ${localHost}`
      : explicitModel
    : isLocal
      ? localHost
        ? `local @ ${localHost}`
        : "local"
      : backend
        ? "default"
        : "";

  const parts = [agentLabel];
  if (modelLabel) parts.push(modelLabel);
  if (effort) parts.push(effort);
  return parts.join(" · ");
}

function usePennyBackendSummary(enabled: boolean): string | null {
  const { apiKey } = useAuth();
  const [summary, setSummary] = useState<string | null>(null);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    const load = async () => {
      const conversationId = getActiveConversationId();
      if (!conversationId) return;
      try {
        const res = await fetch(
          `${getApiBase()}/workbench/conversations/${encodeURIComponent(conversationId)}/agent-profile`,
          {
          credentials: "include",
          headers: authHeaders(apiKey),
          cache: "no-store",
          },
        );
        if (!res.ok || cancelled) return;
        const body = (await res.json()) as { profile: PennyBrainSettings };
        if (!cancelled) setSummary(pennyBackendSummary(body.profile));
      } catch {
        // Runtime details are supplemental; keep the running badge visible.
      }
    };
    void load();
    return () => {
      cancelled = true;
    };
  }, [apiKey, enabled]);

  return summary;
}

function PastBlocks({ blocks }: { blocks: HistoryBlock[] }) {
  const { labels } = useI18n();
  const copy = labels.chat;
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
                🧠 {copy.reasoning.history}
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
              className="rounded border border-[var(--line)] bg-[var(--surface-2)] px-2 py-1 text-[12px] text-[var(--ink-2)]"
            >
              <summary className="cursor-pointer select-none text-[11px] text-[var(--copper-hi)]">
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
                📦 {copy.tool.historyResult} {shortId && `· ${shortId}`}
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
function useActiveHistory(): ActiveHistoryState {
  const [past, setPast] = useState<HistoryMsg[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const { apiKey } = useAuth();
  useEffect(() => {
    let cancelled = false;
    let seq = 0;
    const load = async () => {
      const requestSeq = ++seq;
      const id = getActiveConversationId();
      if (!id) {
        setPast([]);
        setErr(null);
        return;
      }
      const base =
        document
          .querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]')
          ?.content ?? "/v1";
      const root = base.replace(/\/v\d+$/, "");
      try {
        setErr(null);
        const res = await fetch(
          `${root}/api/v1/web/workbench/conversations/${id}/messages`,
          { credentials: "include", headers: authHeaders(apiKey) },
        );
        if (!res.ok) return;
        const body = (await res.json()) as { messages: HistoryMsg[] };
        if (!cancelled && requestSeq === seq) setPast(body.messages ?? []);
      } catch (e) {
        if (!cancelled && requestSeq === seq) setErr((e as Error).message);
      }
    };
    const refreshWhenVisible = () => {
      if (document.visibilityState === "hidden") return;
      void load();
    };
    void load();
    window.addEventListener("focus", refreshWhenVisible);
    window.addEventListener(ACTIVE_CONVERSATION_EVENT, refreshWhenVisible);
    document.addEventListener("visibilitychange", refreshWhenVisible);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", refreshWhenVisible);
      window.removeEventListener(ACTIVE_CONVERSATION_EVENT, refreshWhenVisible);
      document.removeEventListener("visibilitychange", refreshWhenVisible);
    };
  }, [apiKey]);
  return { past, err };
}

/// Wraps `<ThreadPrimitive.Empty>` so the greeting/Ready screen only
/// appears when there's neither live nor historical content — a
/// resumed conversation jumps straight into the past-messages render
/// followed by the date divider, making it feel like a continuous
/// thread instead of a "new chat" welcome landing.
function HistoryAwareEmpty({
  children,
  past,
}: {
  children: ReactNode;
  past: HistoryMsg[];
}) {
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

function PastMessages({ history }: { history: ActiveHistoryState }) {
  const { past, err } = history;
  const { identity } = useAuth();
  const { labels } = useI18n();
  const copy = labels.chat.page;
  const groups = useMemo(() => coalesce(past), [past]);
  if (groups.length === 0) return null;
  const userLabel =
    identity?.display_name || identity?.email?.split("@")[0] || copy.you;
  return (
    <div className="mb-2 space-y-3" data-testid="past-messages">
      <div className="text-center text-[10px] uppercase tracking-wider text-zinc-700">
        {copy.pastConversation}
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
  const { locale } = useI18n();
  const label = useMemo(() => {
    if (!lastTs) return null;
    const d = new Date(lastTs);
    if (Number.isNaN(d.getTime())) return null;
    const browserLocale = locale === "ko" ? "ko-KR" : "en-US";
    const date = d.toLocaleDateString(browserLocale, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    });
    const time = d.toLocaleTimeString(browserLocale, {
      hour: "2-digit",
      minute: "2-digit",
    });
    return `${date} ${time}`;
  }, [lastTs, locale]);
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
  const { labels } = useI18n();
  const copy = labels.chat.page;
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
    let cancelled = false;
    let seq = 0;
    const load = async () => {
      const requestSeq = ++seq;
      const id = getActiveConversationId();
      if (!id) {
        setTitle(null);
        return;
      }
      const base =
        document
          .querySelector<HTMLMetaElement>('meta[name="gadgetron-api-base"]')
          ?.content ?? "/v1";
      const root = base.replace(/\/v\d+$/, "");
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
        if (!cancelled && requestSeq === seq) setTitle(hit?.title ?? null);
      } catch {
        // ignore
      }
    };
    const refresh = () => {
      void load();
    };
    void load();
    window.addEventListener(ACTIVE_CONVERSATION_EVENT, refresh);
    return () => {
      cancelled = true;
      window.removeEventListener(ACTIVE_CONVERSATION_EVENT, refresh);
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
        <span className="text-zinc-500">{copy.conversation}</span>
        <span className="truncate text-zinc-200" title={title}>
          {title}
        </span>
      </div>
    </div>
  );
}

function ActiveSubjectBanner() {
  const { subject, clearActiveSubject } = useWorkbenchSubject();
  const { labels } = useI18n();
  const copy = labels.chat.page;
  if (!subject) return null;
  return (
    <div
      className="shrink-0 border-b border-[var(--line)] bg-[var(--surface)] px-4 py-2"
      data-testid="active-subject-banner"
    >
      <div className="mx-auto flex w-full max-w-[min(1400px,92vw)] items-center gap-2 text-[11px]">
        <span className="shrink-0 text-[var(--copper-hi)]">{copy.talkingAbout}</span>
        <span className="min-w-0 flex-1 truncate text-zinc-100" title={subject.title}>
          {subject.title}
          {subject.subtitle && (
            <span className="ml-1.5 text-zinc-500">· {subject.subtitle}</span>
          )}
        </span>
        {subject.href && (
          <a
            href={subject.href}
            className="shrink-0 rounded border border-[var(--copper)] px-1.5 py-0.5 text-[10px] text-[var(--copper-hi)] hover:text-[var(--ink)]"
          >
            {copy.viewSource}
          </a>
        )}
        {/* Dismiss = forget the pinned subject for this conversation.
          * The transcript keeps the already-sent context message — this
          * only clears the banner/anchor (ISSUE 52; previously there
          * was no way to close it from the UI). */}
        <button
          type="button"
          onClick={clearActiveSubject}
          data-testid="active-subject-clear"
          title={copy.dismissSubjectTitle}
          aria-label={copy.dismissSubjectLabel}
          className="shrink-0 rounded border border-zinc-700 px-1.5 py-0.5 text-[10px] text-zinc-400 hover:border-zinc-500 hover:text-zinc-200"
        >
          ✕
        </button>
      </div>
    </div>
  );
}

export default function Home() {
  const [slashHelpOpen, setSlashHelpOpen] = useState(false);
  const history = useActiveHistory();

  return (
    // WikiPagesProvider feeds MarkdownText so knowledge citations in Penny's
    // answers (footnote definitions naming a page in inline code)
    // linkify to /web/knowledge?q=… (ISSUE 44).
    <WikiPagesProvider>
      <div
        className="flex flex-1 overflow-hidden"
        data-testid="chat-split"
      >
        <div
          className="flex min-w-0 flex-1 flex-col overflow-hidden"
          data-testid="chat-pane"
        >
          <SlashHelpDialog
            open={slashHelpOpen}
            onOpenChange={setSlashHelpOpen}
          />
          <ChatHeader />

      <ThreadPrimitive.Root className="flex flex-1 flex-col overflow-hidden">
        <ActiveConversationBanner />
        <ActiveSubjectBanner />
        <div className="relative flex flex-1 flex-col overflow-hidden">
          <ThreadPrimitive.Viewport className="penny-scroll flex-1 overflow-y-auto">
            {/* Named for stable chat typography and responsive styling. */}
            <div className="chat-thread-column mx-auto w-full max-w-[min(1400px,92vw)] px-4 py-6">
              <PastMessages history={history} />
              <HistoryAwareEmpty past={history.past}>
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
          <div className="chat-thread-column mx-auto w-full max-w-[min(1400px,92vw)] p-4">
            <Composer onOpenHelp={() => setSlashHelpOpen(true)} />
          </div>
        </div>
      </ThreadPrimitive.Root>
        </div>
      </div>
    </WikiPagesProvider>
  );
}

// ---------------------------------------------------------------------------

function ActiveTaskIndicator() {
  const isRunning = useThread((s) => s.isRunning);
  const backendSummary = usePennyBackendSummary(isRunning);
  const { labels } = useI18n();
  const copy = labels.chat.page;
  if (!isRunning) return null;
  return (
    <span
      className="flex min-w-0 items-center gap-1 rounded border border-[var(--copper)] bg-[var(--surface-2)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--copper-hi)] motion-safe:animate-in motion-safe:fade-in duration-200"
      data-testid="active-task-indicator"
      title={backendSummary ? copy.backend(backendSummary) : undefined}
    >
      <span className="size-1.5 rounded-full bg-[var(--copper)] motion-safe:animate-pulse" />
      <span className="shrink-0">{copy.running}</span>
      {backendSummary && (
        <span className="min-w-0 max-w-[min(36vw,22rem)] truncate text-[var(--ink-2)]">
          · {backendSummary}
        </span>
      )}
    </span>
  );
}

function ConversationModelControls() {
  const { apiKey } = useAuth();
  const { labels } = useI18n();
  const copy = labels.chat.page;
  const confirm = useConfirm();
  const isRunning = useThread((state) => state.isRunning);
  const [conversationId, setConversationId] = useState<string | null>(() =>
    getActiveConversationId(),
  );
  const [profile, setProfile] = useState<ConversationAgentProfile | null>(null);
  const [pinned, setPinned] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editingCustomModel, setEditingCustomModel] = useState(false);
  const [customModelDraft, setCustomModelDraft] = useState("");
  const [localModels, setLocalModels] = useState<AvailableLlmEndpointModel[]>([]);

  const load = useCallback(async () => {
    const id = getActiveConversationId();
    setConversationId(id);
    if (!id) {
      setProfile(null);
      setPinned(false);
      return;
    }
    try {
      const result = await getConversationAgentProfile(apiKey, id);
      setProfile(result.profile);
      setCustomModelDraft(result.profile.model);
      setEditingCustomModel(modelOptionKey(result.profile) === "custom");
      setPinned(result.pinned);
      cacheConversationAgentProfile(id, result.profile);
    } catch (error) {
      toast.error((error as Error).message);
    }
  }, [apiKey]);

  useEffect(() => {
    void load();
    const onConversation = () => void load();
    window.addEventListener(ACTIVE_CONVERSATION_EVENT, onConversation);
    return () =>
      window.removeEventListener(ACTIVE_CONVERSATION_EVENT, onConversation);
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    void listAvailableLlmEndpointModels(apiKey)
      .then((models) => {
        if (!cancelled) setLocalModels(models ?? []);
      })
      .catch(() => {
        if (!cancelled) setLocalModels([]);
      });
    return () => {
      cancelled = true;
    };
  }, [apiKey]);

  // A first turn pins a formerly-default projection. Refresh when a job
  // crosses either running edge so the lock state is current.
  useEffect(() => {
    if (isRunning) return;
    void load();
  }, [isRunning, load]);

  const saveHere = useCallback(
    async (next: ConversationAgentProfile) => {
      if (!conversationId) return;
      const previous = profile;
      setProfile(next);
      cacheConversationAgentProfile(conversationId, next);
      setBusy(true);
      try {
        const saved = await updateConversationAgentProfile(
          apiKey,
          conversationId,
          next,
        );
        setProfile(saved.profile);
        setCustomModelDraft(saved.profile.model);
        setPinned(saved.pinned);
        cacheConversationAgentProfile(conversationId, saved.profile);
      } catch (error) {
        if (previous) {
          setProfile(previous);
          cacheConversationAgentProfile(conversationId, previous);
        }
        toast.error((error as Error).message);
      } finally {
        setBusy(false);
      }
    },
    [apiKey, conversationId, profile],
  );

  const chooseModel = useCallback(
    async (key: string) => {
      if (!profile || key === "custom") return;
      const preset = AGENT_MODEL_OPTIONS.find((item) => item.key === key);
      const local = key.startsWith("local:")
        ? localModels.find((item) => `local:${item.endpoint_id}` === key)
        : undefined;
      if (!preset && !local) return;
      const option = preset ?? {
        backend: local!.backend,
        model: local!.model_id,
      };
      setEditingCustomModel(false);
      setCustomModelDraft(option.model);
      const next: ConversationAgentProfile = {
        ...profile,
        backend: option.backend,
        model: option.model,
        effort: normalizeAgentEffort(option.backend, option.model, profile.effort),
        model_source: local ? "local" : "default",
        llm_endpoint_id: local?.endpoint_id ?? null,
        local_base_url: "",
        local_api_key_env: "",
      };
      if (pinned && option.backend !== profile.backend) {
        const accepted = await confirm({
          title: copy.startRuntimeTitle,
          description: copy.startRuntimeDescription,
          confirmLabel: copy.startNewChat,
        });
        if (!accepted) return;
        const newConversationId = safeRandomUUID();
        setBusy(true);
        try {
          const saved = await updateConversationAgentProfile(
            apiKey,
            newConversationId,
            next,
          );
          cacheConversationAgentProfile(newConversationId, saved.profile);
          setActiveConversationId(newConversationId);
        } catch (error) {
          toast.error((error as Error).message);
        } finally {
          setBusy(false);
        }
        return;
      }
      await saveHere(next);
    },
    [apiKey, confirm, copy.startNewChat, copy.startRuntimeDescription, copy.startRuntimeTitle, localModels, pinned, profile, saveHere],
  );

  const chooseEffort = useCallback(
    async (effort: AgentEffort) => {
      if (!profile) return;
      await saveHere({ ...profile, effort });
    },
    [profile, saveHere],
  );

  if (!profile) return null;
  const selectedModel = modelOptionKey(profile);
  const customLabel = `${profile.backend === "codex_exec" ? "Codex" : "Claude"} · ${profile.model || "default"}`;
  const disabled = busy || isRunning;
  return (
    <div className="flex items-center gap-1.5" data-testid="conversation-agent-profile">
      <select
        aria-label={copy.conversationModel}
        value={selectedModel}
        disabled={disabled}
        onChange={(event) => void chooseModel(event.target.value)}
        className="h-6 max-w-[13rem] rounded border border-zinc-800 bg-zinc-950 px-1.5 font-mono text-[10px] text-zinc-300 disabled:opacity-50"
        title={
          isRunning
            ? copy.modelLocked
            : pinned
              ? copy.runtimePinned
              : copy.modelForChat
        }
      >
        {selectedModel === "custom" && (
          <option value="custom">{customLabel}</option>
        )}
        <optgroup label={copy.claudeRuntime}>
          {AGENT_MODEL_OPTIONS.filter((item) => item.backend === "claude_code").map(
            (item) => (
              <option key={item.key} value={item.key}>
                {item.label}
              </option>
            ),
          )}
        </optgroup>
        <optgroup label={copy.codexRuntime}>
          {AGENT_MODEL_OPTIONS.filter((item) => item.backend === "codex_exec").map(
            (item) => (
              <option key={item.key} value={item.key}>
                {item.label}
              </option>
            ),
          )}
        </optgroup>
        {(localModels ?? []).length > 0 && (
          <optgroup label={copy.verifiedLocalModels}>
            {(localModels ?? []).map((item) => (
              <option
                key={`${item.endpoint_id}:${item.model_id}`}
                value={`local:${item.endpoint_id}`}
              >
                {item.endpoint_name} · {item.model_id} · {item.backend === "codex_exec" ? "Codex" : "Claude"}
              </option>
            ))}
          </optgroup>
        )}
      </select>
      {!profile.llm_endpoint_id && (editingCustomModel || selectedModel === "custom") ? (
        <input
          aria-label={copy.customModelId}
          value={customModelDraft}
          disabled={disabled}
          onChange={(event) => setCustomModelDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key !== "Enter") return;
            event.preventDefault();
            const model = customModelDraft.trim();
            if (model && model !== profile.model) void saveHere({ ...profile, model });
          }}
          onBlur={() => {
            const model = customModelDraft.trim();
            if (model && model !== profile.model) void saveHere({ ...profile, model });
          }}
          placeholder={copy.modelIdPlaceholder}
          className="h-6 w-28 rounded border border-zinc-800 bg-zinc-950 px-1.5 font-mono text-[10px] text-zinc-300 disabled:opacity-50"
        />
      ) : !profile.llm_endpoint_id ? (
        <button
          type="button"
          disabled={disabled}
          onClick={() => {
            setCustomModelDraft(profile.model);
            setEditingCustomModel(true);
          }}
          className="h-6 rounded border border-zinc-800 px-1.5 font-mono text-[9px] text-zinc-500 hover:text-zinc-300 disabled:opacity-50"
          title={copy.customModelTitle}
        >
          {copy.custom}
        </button>
      ) : null}
      <select
        aria-label={copy.conversationEffort}
        value={profile.effort}
        disabled={disabled}
        onChange={(event) => void chooseEffort(event.target.value as AgentEffort)}
        className="h-6 rounded border border-zinc-800 bg-zinc-950 px-1.5 font-mono text-[10px] text-zinc-300 disabled:opacity-50"
        title={copy.effortTitle}
      >
        {agentEffortOptions(profile.backend, profile.model).map((effort) => (
          <option key={effort} value={effort}>
            {copy.effort} · {effort === "auto" ? copy.auto : effort}
          </option>
        ))}
      </select>
      {pinned && (
        <span className="font-mono text-[9px] text-zinc-600" title={copy.pinnedTitle}>
          {copy.pinned}
        </span>
      )}
    </div>
  );
}

function ChatHeader() {
  return (
    <header
      className="flex h-9 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4"
      data-testid="chat-header"
    >
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-zinc-300">Penny</span>
      </div>
      <div className="flex items-center gap-1.5">
        <ConversationModelControls />
        <ActiveTaskIndicator />
      </div>
    </header>
  );
}

// Suggestion pool — Penny cycles three picks at a time every ~8 s so
// the Ready screen feels alive instead of showing the same three
// prompts forever. Ordered roughly by category (knowledge / operator /
// self-discovery / fleet ops) so a random 3-slice usually covers a
// mix. Add more lines here — they'll join the rotation automatically.
const SUGGESTION_POOL_SIZE = 15;

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
  const { labels } = useI18n();
  const copy = labels.chat.page;
  const pool = [
    copy.suggestionCompany,
    copy.suggestionWikiList,
    copy.suggestionGpuSummary,
    copy.suggestionMeetingTemplate,
    copy.suggestionRecentPages,
    copy.suggestionTroubleshooting,
    copy.suggestionNccl,
    copy.suggestionPostmortem,
    copy.suggestionRecovery,
    copy.suggestionTools,
    copy.suggestionPennyCapabilities,
    copy.suggestionCluster,
    copy.suggestionWikiChanges,
    copy.suggestionNextTasks,
    copy.suggestionDashboard,
  ];
  const [slice, setSlice] = useState<number[]>(() =>
    shuffle(Array.from({ length: SUGGESTION_POOL_SIZE }, (_, index) => index)).slice(0, ROTATE_COUNT),
  );
  useEffect(() => {
    const id = window.setInterval(() => {
      setSlice((prev) => {
        // Find a new slice that differs from the current one by at
        // least one entry — defensive against the pool being smaller
        // than the slice.
        for (let attempt = 0; attempt < 5; attempt++) {
          const candidate = shuffle(
            Array.from({ length: SUGGESTION_POOL_SIZE }, (_, index) => index),
          ).slice(0, ROTATE_COUNT);
          if (candidate.some((s, i) => prev[i] !== s)) {
            return candidate;
          }
        }
        return prev;
      });
    }, ROTATE_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, []);
  return slice.map((index) => pool[index]);
}

// Shown instead of the generic rotation when the conversation is
// pinned to a subject ("Talking about …", ISSUE 53). Each pick goes out
// through withSubjectContext, so the structured context rides along
// even when the seeded auto-send lost its race.
function EmptyState() {
  const composer = useComposerRuntime();
  const { subject } = useWorkbenchSubject();
  const { labels } = useI18n();
  const copy = labels.chat.page;
  const rotating = useRotatingSuggestions();
  const suggestions = subject
    ? [copy.subjectCause, copy.subjectImpact, copy.subjectResolution]
    : rotating;

  const pick = useCallback(
    (text: string) => {
      composer.setText(withSubjectContext(text));
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
        <h1
          className="max-w-md truncate text-sm font-medium text-zinc-300"
          title={subject?.title}
        >
          {subject ? copy.askAbout(subject.title) : copy.ready}
        </h1>
      </div>
      <div className="flex w-full flex-col gap-1.5">
        {suggestions.map((s) => (
          <button
            key={s}
            onClick={() => pick(s)}
            className="rounded border border-zinc-800 bg-zinc-900/50 px-3 py-2.5 text-left text-xs text-zinc-400 transition-colors hover:border-zinc-700 hover:bg-zinc-900 hover:text-zinc-300"
          >
            {s}
          </button>
        ))}
      </div>
    </div>
  );
}

function JumpToLatest() {
  const { labels } = useI18n();
  const copy = labels.chat.page;
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
        aria-label={copy.jumpLatest}
        className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded border border-[var(--line)] bg-[var(--surface)] px-3 py-1.5 text-xs font-medium text-[var(--ink)] transition-all hover:bg-[var(--surface-2)] motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-1 duration-200"
      >
        <ChevronDown className="size-3.5" />
        {copy.latest}
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
  const { labels } = useI18n();
  if (!isRunning) return null;
  return (
    <div
      className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center motion-safe:animate-in motion-safe:fade-in duration-200"
      aria-live="polite"
      aria-label={labels.chat.page.composing}
      data-testid="bottom-typing-indicator"
    >
      <span className="flex items-center gap-1.5 rounded border border-[var(--line)] bg-[var(--surface)] px-3 py-1.5">
        <span
          className="size-1.5 rounded-full bg-[var(--copper)] motion-safe:animate-pulse"
          style={{ animationDelay: "-0.3s" }}
        />
        <span
          className="size-1.5 rounded-full bg-[var(--copper)] motion-safe:animate-pulse"
          style={{ animationDelay: "-0.15s" }}
        />
        <span className="size-1.5 rounded-full bg-[var(--copper)] motion-safe:animate-pulse" />
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
  const { labels } = useI18n();
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
      aria-label={labels.chat.page.copyMessage}
      onClick={onCopy}
      className="absolute right-2 top-2 flex size-7 items-center justify-center rounded border border-[var(--line)] bg-[var(--surface)] text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100"
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
  const { labels } = useI18n();
  const label =
    identity?.display_name || identity?.email?.split("@")[0] || labels.chat.page.you;
  return (
    <div className="group mb-6 flex items-start justify-end gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300">
      <div className="flex max-w-[85%] flex-col items-end gap-1">
        <span className="text-xs text-muted-foreground">{label}</span>
        <Card className="relative bg-primary text-primary-foreground">
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
    <div
      className="group mb-6 flex items-start gap-3 motion-safe:animate-in motion-safe:fade-in motion-safe:slide-in-from-bottom-2 duration-300"
      data-testid="penny-assistant-message"
    >
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
  const { labels } = useI18n();
  const copy = labels.chat.page;
  if (!status || status.type !== "incomplete") return null;
  const reason = status.reason;
  const labelMap: Record<string, { text: string; tint: string }> = {
    cancelled: {
      text: copy.stopped,
      tint: "text-amber-300/90 border-amber-400/30 bg-amber-400/10",
    },
    length: {
      text: copy.lengthLimit,
      tint: "text-[var(--warn)] border-[var(--warn)] bg-[var(--surface-2)]",
    },
    "content-filter": {
      text: copy.filtered,
      tint: "text-red-300/90 border-red-400/30 bg-red-400/10",
    },
    "tool-calls": {
      text: copy.toolPending,
      tint: "text-[var(--copper-hi)] border-[var(--copper)] bg-[var(--surface-2)]",
    },
    error: {
      text: copy.error,
      tint: "text-red-300/90 border-red-400/30 bg-red-400/10",
    },
    other: {
      text: copy.interrupted,
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
  const { labels } = useI18n();
  const copy = labels.chat.page;
  // First-message detector for guaranteed subject delivery (ISSUE 53).
  const messageCount = useThread((s) => s.messages.length);
  const { activeConversationId, refreshSubject } = useWorkbenchSubject();

  const handleStop = useCallback(() => {
    const conversationId = getActiveConversationId();
    if (!conversationId) return;
    void cancelActiveConversationJob(conversationId).catch(() => {
      toast.error(copy.stopFailed, {
        description: copy.stopFailedDescription,
      });
    });
  }, [copy.stopFailed, copy.stopFailedDescription]);

  // Preserve the in-progress draft across chat switches / reloads.
  // Keyed by active conversation id so each chat gets its own pending
  // text. Restored on mount; saved on every keystroke via the composer
  // store subscription.
  useEffect(() => {
    if (typeof window === "undefined") return;
    const convId = getActiveConversationId() ?? "default";
    refreshSubject();
    const storageKey = `gadgetron_draft_${convId}`;
    const pendingSubmitKey = `gadgetron_pending_submit_${convId}`;
    const saved = window.localStorage.getItem(storageKey);
    if (saved && !composer.getState().text) {
      composer.setText(saved);
    }
    // Deep-link entry point: when another page (e.g. the Logs tab
    // Ask Penny button) minted this conversation + seeded a
    // draft AND set the pending_submit flag, auto-fire the send so
    // the operator doesn't have to hit Enter again. Clear both
    // markers so a reload doesn't re-submit forever.
    if (
      saved &&
      window.localStorage.getItem(pendingSubmitKey) === "1"
    ) {
      window.localStorage.removeItem(pendingSubmitKey);
      // One tick delay so the composer has rendered its current text
      // + the runtime has finished spinning up.
      setTimeout(() => {
        try {
          composer.send();
        } catch {
          // If send throws (unlikely) the draft is still there for
          // the operator to submit manually.
        }
      }, 60);
    }
    const unsub = composer.subscribe(() => {
      const t = composer.getState().text;
      if (t) window.localStorage.setItem(storageKey, t);
      else window.localStorage.removeItem(storageKey);
    });
    return () => {
      unsub?.();
    };
  }, [composer, refreshSubject]);

  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    // Clear any saved draft — the message is heading out.
    if (typeof window !== "undefined") {
      const convId = getActiveConversationId() ?? "default";
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
    // Guaranteed subject delivery (ISSUE 53): on the FIRST message of a
    // conversation pinned to a subject, prepend the structured context.
    // The seeded auto-send can silently lose its race with the runtime
    // spinning up after a full-page navigation — without this, a vague
    // question about "this bug" reaches Penny without the pinned subject.
    // Visible on purpose: the transcript should show what Penny saw.
    if (messageCount === 0) {
      const next = withSubjectContext(text);
      if (next !== text) composer.setText(next);
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
      className="relative flex flex-col rounded border border-zinc-700 bg-zinc-900 p-2 transition-colors focus-within:border-zinc-600 focus-within:bg-zinc-900"
    >
      <ChatAttachmentTray conversationId={activeConversationId ?? getActiveConversationId()} />
      <SlashAutocomplete onLocalExecute={executeLocalCommand} />
      <div className="flex items-end gap-2">
        <ComposerPrimitive.Input
          placeholder={copy.askPenny}
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
              aria-label={copy.stopGeneration}
              onClick={handleStop}
            >
              <Square className="size-3.5 fill-current" />
            </Button>
          </ComposerPrimitive.Cancel>
        ) : (
          <ComposerPrimitive.Send asChild>
            <Button
              size="icon"
              className="size-8 shrink-0 bg-[var(--copper)] text-[var(--bg)] hover:bg-[var(--copper-hi)]"
              aria-label={copy.send}
            >
              <SendHorizonal className="size-4" />
            </Button>
          </ComposerPrimitive.Send>
        )}
      </div>
    </ComposerPrimitive.Root>
  );
}
