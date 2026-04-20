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

interface EvidenceContextValue {
  items: EvidenceItem[];
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

function wsUrl(actorKey: string): string {
  if (typeof location === "undefined") return "";
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const base = `${scheme}//${location.host}${getApiBase()}/workbench/events/ws`;
  return `${base}?token=${encodeURIComponent(actorKey)}`;
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

export function EvidenceProvider({ children }: { children: ReactNode }) {
  const { apiKey } = useAuth();
  const [items, setItems] = useState<EvidenceItem[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "disconnected" | "connecting" | "open" | "closed"
  >("disconnected");
  const wsRef = useRef<WebSocket | null>(null);

  useEffect(() => {
    if (!apiKey) {
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
          const item = toEvidenceItem(parsed);
          if (!item) return;
          setItems((prev) => [item, ...prev].slice(0, MAX_ITEMS));
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
  }, [apiKey]);

  return (
    <EvidenceContext.Provider
      value={{ items, wsStatus, clear: () => setItems([]) }}
    >
      {children}
    </EvidenceContext.Provider>
  );
}

export function useEvidence(): EvidenceContextValue {
  const ctx = useContext(EvidenceContext);
  if (!ctx) {
    return { items: [], wsStatus: "disconnected", clear: () => {} };
  }
  return ctx;
}
