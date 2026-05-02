"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useThread } from "@assistant-ui/react";
import { MessageSquarePlus, Trash2 } from "lucide-react";
import { useAuth } from "../../lib/auth-context";
import {
  clearActiveConversationId,
  getActiveConversationId,
  setActiveConversationId,
} from "../../lib/conversation-id";
import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------
// Left-rail bottom pane: per-user conversation list (ISSUE 31).
//
// Lists the calling user's non-deleted conversations newest-first.
// Clicking switches the active `gadgetron_conversation_id` which the
// chat transport reads on the next turn and sends as
// `X-Gadgetron-Conversation-Id`. The backend maps that UUID back to a
// Claude Code `--session-id` so the thread resumes from where it left
// off server-side. Client history re-fetch is a P2B concern — for now,
// switching conversations hard-reloads the page so the assistant-ui
// runtime boots into a clean thread pointed at the new conversation.
//
// All access to the active id MUST go through
// `app/lib/conversation-id.ts` — direct localStorage / sessionStorage
// access for this key is a bug (breaks the per-tab isolation invariant).
// ---------------------------------------------------------------------------

interface ConvRow {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

function getServerRoot(): string {
  if (typeof document === "undefined") return "";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const base = meta?.content ?? "/v1";
  return base.replace(/\/v\d+$/, "");
}

function randomUuid(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  // Fallback: 128-bit-ish v4-shaped random (insecure contexts).
  const hex = Array.from({ length: 36 }, (_, i) => {
    if (i === 8 || i === 13 || i === 18 || i === 23) return "-";
    if (i === 14) return "4";
    if (i === 19) return ((Math.random() * 4) | 8).toString(16);
    return Math.floor(Math.random() * 16).toString(16);
  });
  return hex.join("");
}

async function listConversations(apiKey: string | null): Promise<ConvRow[]> {
  const res = await fetch(
    `${getServerRoot()}/api/v1/web/workbench/conversations`,
    {
      credentials: "include",
      headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
    },
  );
  if (!res.ok) throw new Error(`list conversations: HTTP ${res.status}`);
  const body = (await res.json()) as { conversations: ConvRow[] };
  return body.conversations ?? [];
}

async function deleteConversation(
  apiKey: string | null,
  id: string,
): Promise<void> {
  const res = await fetch(
    `${getServerRoot()}/api/v1/web/workbench/conversations/${id}`,
    {
      method: "DELETE",
      credentials: "include",
      headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
    },
  );
  if (!res.ok) throw new Error(`delete conversation: HTTP ${res.status}`);
}

function readActiveConvId(): string | null {
  return getActiveConversationId();
}

function writeActiveConvId(id: string | null): void {
  if (id) setActiveConversationId(id);
  else clearActiveConversationId();
}

export function ConversationsPane({ collapsed }: { collapsed: boolean }) {
  const { apiKey, identity } = useAuth();
  const [rows, setRows] = useState<ConvRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [active, setActive] = useState<string | null>(null);

  // Hydrate the active id once on mount.
  useEffect(() => {
    setActive(readActiveConvId());
  }, []);

  const refresh = useCallback(async () => {
    // Only logged-in users (session or api key) see real conversations.
    if (!apiKey && !identity) {
      setRows([]);
      return;
    }
    setLoading(true);
    setErr(null);
    try {
      const list = await listConversations(apiKey);
      setRows(list);
    } catch (e) {
      setErr((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, identity]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Refresh every 5s so new chats started in the main pane appear in
  // the sidebar quickly. Cheap query — single index scan per tick.
  useEffect(() => {
    const iv = window.setInterval(() => void refresh(), 5_000);
    return () => window.clearInterval(iv);
  }, [refresh]);

  // Also refresh on each transition of `isRunning` — the server-side
  // upsert happens when a turn starts AND the title may finalize when
  // the turn finishes, so we poll at both edges. Tiny delay so the
  // tokio::spawn upsert has a beat to commit.
  const isRunning = useThread((s) => s.isRunning);
  const lastRunningRef = useRef(false);
  useEffect(() => {
    const t = window.setTimeout(() => void refresh(), 400);
    lastRunningRef.current = isRunning;
    return () => window.clearTimeout(t);
  }, [isRunning, refresh]);

  const goTo = useCallback((id: string | null) => {
    writeActiveConvId(id);
    setActive(id);
    if (typeof window === "undefined") return;
    // `location.assign("/web/")` from within `/web/` would be a no-op
    // in some browsers; force a reload so the assistant-ui runtime
    // boots fresh for the chosen conversation.
    if (window.location.pathname === "/web" || window.location.pathname === "/web/") {
      window.location.reload();
    } else {
      window.location.assign("/web/");
    }
  }, []);

  const startNewChat = useCallback(() => {
    goTo(randomUuid());
  }, [goTo]);

  const switchTo = useCallback((id: string) => {
    goTo(id);
  }, [goTo]);

  const remove = useCallback(
    async (id: string) => {
      if (!window.confirm("이 대화를 삭제할까요? (되돌릴 수 없습니다)")) return;
      try {
        await deleteConversation(apiKey, id);
        if (active === id) {
          writeActiveConvId(null);
          setActive(null);
        }
        await refresh();
      } catch (e) {
        setErr((e as Error).message);
      }
    },
    [apiKey, active, refresh],
  );

  if (collapsed) {
    return (
      <div className="flex shrink-0 flex-col gap-1 border-t border-zinc-800 px-1 py-2">
        <button
          type="button"
          onClick={startNewChat}
          className="flex size-8 items-center justify-center rounded text-zinc-500 hover:bg-zinc-900 hover:text-zinc-200"
          title="새 대화"
        >
          <MessageSquarePlus className="size-4" aria-hidden />
        </button>
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col border-t border-zinc-800">
      <div className="flex shrink-0 items-center justify-between px-3 py-1.5">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">
          Chats
        </span>
        <button
          type="button"
          onClick={startNewChat}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-zinc-500 hover:bg-zinc-900 hover:text-zinc-200"
          data-testid="new-chat-btn"
          title="새 대화"
        >
          <MessageSquarePlus className="size-3" aria-hidden />
          새 대화
        </button>
      </div>

      <div
        className="flex-1 overflow-y-auto px-1 pb-2"
        data-testid="conversations-list"
      >
        {loading && rows.length === 0 && (
          <div className="px-2 py-1 text-[11px] text-zinc-600">…</div>
        )}
        {err && (
          <div className="mx-1 rounded border border-red-900/40 bg-red-950/20 px-2 py-1 text-[10px] text-red-400">
            {err}
          </div>
        )}
        {!loading && rows.length === 0 && !err && (
          <div className="px-2 py-1 text-[11px] text-zinc-600">
            아직 대화가 없습니다.
          </div>
        )}
        {rows.map((r) => (
          <div
            key={r.id}
            className={cn(
              "group flex items-center gap-1 rounded px-1.5 py-1 text-[11px]",
              active === r.id
                ? "bg-zinc-800 text-zinc-100"
                : "text-zinc-400 hover:bg-zinc-900 hover:text-zinc-200",
            )}
          >
            <button
              type="button"
              onClick={() => switchTo(r.id)}
              className="flex-1 truncate text-left"
              title={r.title}
              data-testid={`conv-${r.id}`}
            >
              {r.title}
            </button>
            <button
              type="button"
              onClick={() => void remove(r.id)}
              className="flex size-5 shrink-0 items-center justify-center rounded text-zinc-600 opacity-0 transition group-hover:opacity-100 hover:bg-zinc-800 hover:text-red-400"
              title="삭제"
              data-testid={`conv-delete-${r.id}`}
            >
              <Trash2 className="size-3" aria-hidden />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}
