"use client";

import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useAuth } from "./auth-context";

// Live evidence feed: subscribes to /workbench/events/ws once per session,
// keeps the most recent 50 read-tier tool calls + knowledge-category
// workbench actions so the EvidencePane can render what Penny has cited
// in the current conversation.

export interface EvidenceItem {
  id: string;
  kind: "tool" | "action";
  name: string;
  tier?: string;
  outcome: string;
  elapsedMs?: number;
  at: number;
  conversationId?: string | null;
  /** Short stringified JSON of the tool/action input (≤200 chars). */
  argumentsSummary?: string | null;
  /** Parsed input, used to derive href / pretty display. */
  argumentsParsed?: Record<string, unknown> | null;
  /** Optional deep-link — clicked opens in a new tab. */
  href?: string | null;
}

/// One host the operator (or Penny on their behalf) has touched in the
/// current chat. Surfaced in the evidence pane as a collapsed card so
/// the operator can see "I've been talking about these N servers"
/// without scrolling tool-call history.
///
/// Bus source: `tool_call_completed` events whose `tool_name` starts
/// with `server.` AND whose parsed `arguments_summary` carries an
/// `id` UUID. `server.list` (no host id) is intentionally skipped —
/// it doesn't bind the chat to one specific host.
export interface ServerContextItem {
  /// `HostRecord.id` — UUID. Stable key into `/api/v1/web/workbench/
  /// servers/{id}/metrics` for the live drawer fetch.
  hostId: string;
  /// Last `server.*` tool call's tool_name (e.g. `server.stats`).
  /// Surfaces what Penny was doing with the host so the operator can
  /// recall "ah I asked it to check journal logs", not just "host X".
  lastToolName: string;
  /// `Date.now()` of the most recent reference. Used to sort the stack
  /// so the most-recently-discussed host floats to the top.
  lastSeenAt: number;
  /// Total `server.*` calls touching this host in the current
  /// conversation. Hint: high number = "Penny keeps coming back to
  /// this host" → operator probably wants live status.
  mentionCount: number;
}

interface EvidenceContextValue {
  items: EvidenceItem[];
  /// Host-keyed roll-up of `server.*` tool calls in the active chat.
  /// Sorted by `lastSeenAt` descending (most recent first).
  serverContext: ServerContextItem[];
  wsStatus: "disconnected" | "connecting" | "open" | "closed";
  clear: () => void;
}

const EvidenceContext = createContext<EvidenceContextValue | null>(null);

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function wsUrl(actorKey: string | null): string {
  if (typeof location === "undefined") return "";
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const base = `${scheme}//${location.host}${getApiBase()}/workbench/events/ws`;
  return actorKey ? `${base}?token=${encodeURIComponent(actorKey)}` : base;
}

function parseArgsJson(
  raw: string | null | undefined,
): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" && !Array.isArray(v)
      ? (v as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

// Derive a clickable URL from the tool name + parsed input. All links
// open in a new tab so the chat session is preserved.
function deriveHref(
  name: string,
  args: Record<string, unknown> | null,
): string | null {
  if (!args) {
    // Defaults: tool-level jumps even without args.
    if (name === "wiki.list" || name === "wiki.search" || name === "wiki.get") {
      return "/web/wiki";
    }
    return null;
  }
  if (name === "wiki.get" || name === "wiki.read") {
    const page = typeof args.name === "string" ? args.name : null;
    return page ? `/web/wiki?page=${encodeURIComponent(page)}` : "/web/wiki";
  }
  if (name === "wiki.search" || name === "wiki.list") {
    const q = typeof args.query === "string" ? args.query : "";
    return q
      ? `/web/wiki?q=${encodeURIComponent(q)}`
      : "/web/wiki";
  }
  if (name === "web.search") {
    const q = typeof args.query === "string" ? args.query : "";
    return q
      ? `https://www.google.com/search?q=${encodeURIComponent(q)}`
      : null;
  }
  return null;
}

// Evidence filter: read-tier tool calls + knowledge workbench actions.
// Destructive writes (wiki.write, wiki.delete) are audit-worthy but
// not evidence for what Penny is reading. Chat completions + approvals
// are observability events, also excluded here.
function toEvidenceItem(ev: Record<string, unknown>): EvidenceItem | null {
  const type = ev.type;
  const at = Date.now();
  if (type === "tool_call_completed") {
    const tier = String(ev.tier ?? "");
    if (tier !== "read") return null;
    const name = String(ev.tool_name ?? "unknown");
    const argumentsSummary =
      (ev.arguments_summary as string | null | undefined) ?? null;
    const argumentsParsed = parseArgsJson(argumentsSummary);
    return {
      id: `${at}-${name}-${Math.random().toString(36).slice(2, 8)}`,
      kind: "tool",
      name,
      tier,
      outcome: String(ev.outcome ?? "unknown"),
      elapsedMs:
        typeof ev.elapsed_ms === "number" ? (ev.elapsed_ms as number) : undefined,
      at,
      conversationId: (ev.conversation_id as string | null | undefined) ?? null,
      argumentsSummary,
      argumentsParsed,
      href: deriveHref(name, argumentsParsed),
    };
  }
  if (type === "action_completed") {
    const actionId = String(ev.action_id ?? "");
    const isKnowledge =
      actionId.startsWith("wiki-") ||
      actionId.startsWith("knowledge-") ||
      actionId.startsWith("web-");
    const isRead =
      actionId === "wiki-list" ||
      actionId === "wiki-read" ||
      actionId === "knowledge-search" ||
      actionId === "web-search";
    if (!isKnowledge || !isRead) return null;
    const argumentsSummary =
      (ev.arguments_summary as string | null | undefined) ?? null;
    const argumentsParsed = parseArgsJson(argumentsSummary);
    // Map workbench action IDs onto the same href derivation used for
    // tool names so either surface links to the same destination.
    const toolLike =
      actionId === "wiki-list"
        ? "wiki.list"
        : actionId === "wiki-read"
          ? "wiki.get"
          : actionId === "knowledge-search"
            ? "wiki.search"
            : actionId === "web-search"
              ? "web.search"
              : actionId;
    return {
      id: `${at}-${actionId}-${Math.random().toString(36).slice(2, 8)}`,
      kind: "action",
      name: actionId,
      outcome: String(ev.outcome ?? "unknown"),
      elapsedMs:
        typeof ev.elapsed_ms === "number" ? (ev.elapsed_ms as number) : undefined,
      at,
      argumentsSummary,
      argumentsParsed,
      href: deriveHref(toolLike, argumentsParsed),
    };
  }
  return null;
}

const MAX_ITEMS = 50;
/// UUID-shaped string check for `args.id` — the host_id field is a
/// real UUID (per server-monitor schemas, see `bundles/server-monitor/
/// src/gadgets.rs::schema_server_*`). Validating here avoids raising a
/// card on an off-by-one accident where `arguments_summary` JSON drifts
/// to e.g. `{"id": "audit_log"}` for an unrelated tool that shares the
/// `server.` prefix in some future bundle.
const UUID_RE =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/// Pull a host id out of a `tool_call_completed` event when the tool
/// name targets a single host. Returns null for `server.list`,
/// `server.add`, and any non-`server.*` tool. `server.remove` is
/// intentionally surfaced too — even though the host is going away,
/// the operator still benefits from seeing its last-known card while
/// the destructive call resolves.
function deriveServerContextRef(
  toolName: string,
  args: Record<string, unknown> | null,
): { hostId: string; toolName: string } | null {
  if (!toolName.startsWith("server.")) return null;
  if (toolName === "server.list" || toolName === "server.add") return null;
  if (!args) return null;
  const id = args.id;
  if (typeof id !== "string" || !UUID_RE.test(id)) return null;
  return { hostId: id, toolName };
}

export function EvidenceProvider({ children }: { children: ReactNode }) {
  const { apiKey, identity } = useAuth();
  const [items, setItems] = useState<EvidenceItem[]>([]);
  const [serverContext, setServerContext] = useState<ServerContextItem[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "disconnected" | "connecting" | "open" | "closed"
  >("disconnected");
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    // Either an API key OR a logged-in session is enough — the gateway
    // accepts the session cookie on the WS upgrade request.
    if (!apiKey && !identity) {
      setWsStatus("disconnected");
      return;
    }
    let closed = false;
    const connect = () => {
      if (closed) return;
      setWsStatus("connecting");
      const socket = new WebSocket(wsUrl(apiKey));
      wsRef.current = socket;
      socket.onopen = () => setWsStatus("open");
      socket.onclose = () => {
        setWsStatus("closed");
        if (!closed) setTimeout(connect, 3000);
      };
      socket.onmessage = (msg) => {
        try {
          const parsed = JSON.parse(msg.data) as Record<string, unknown>;
          // 1. Evidence list (existing read-tier filter).
          const item = toEvidenceItem(parsed);
          if (item) {
            setItems((prev) => [item, ...prev].slice(0, MAX_ITEMS));
          }
          // 2. Server-context cards (host-id-keyed roll-up). Driven
          //    by the same `tool_call_completed` events but with a
          //    different filter — a `server.stats` call is "evidence"
          //    AND a "host context update" simultaneously.
          if (parsed.type === "tool_call_completed") {
            const toolName = String(parsed.tool_name ?? "");
            const argsRaw = parsed.arguments_summary as
              | string
              | null
              | undefined;
            const args = parseArgsJson(argsRaw);
            const ref = deriveServerContextRef(toolName, args);
            if (ref) {
              const now = Date.now();
              setServerContext((prev) => {
                const next = prev.filter((h) => h.hostId !== ref.hostId);
                const existing = prev.find((h) => h.hostId === ref.hostId);
                next.unshift({
                  hostId: ref.hostId,
                  lastToolName: ref.toolName,
                  lastSeenAt: now,
                  mentionCount: (existing?.mentionCount ?? 0) + 1,
                });
                return next;
              });
            }
          }
        } catch {
          // drop malformed frame
        }
      };
    };
    connect();
    return () => {
      closed = true;
      wsRef.current?.close();
    };
  }, [apiKey, identity]);

  return (
    <EvidenceContext.Provider
      value={{
        items,
        serverContext,
        wsStatus,
        clear: () => {
          setItems([]);
          setServerContext([]);
        },
      }}
    >
      {children}
    </EvidenceContext.Provider>
  );
}

export function useEvidence(): EvidenceContextValue {
  const ctx = useContext(EvidenceContext);
  if (!ctx) {
    return {
      items: [],
      serverContext: [],
      wsStatus: "disconnected",
      clear: () => {},
    };
  }
  return ctx;
}
