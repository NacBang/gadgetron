"use client";

import { useCallback, useEffect, useState } from "react";
import { useEvidence, type ServerContextItem } from "../../lib/evidence-context";
import { useAuth } from "../../lib/auth-context";

// Server-context card stack — a collapsed-by-default list of hosts the
// active chat has touched (via `server.*` tool calls), surfaced inside
// the right-rail evidence pane so the operator can glance at the
// servers Penny is reasoning about without reading the tool log.
//
// Driven entirely by `useEvidence().serverContext` which the WebSocket
// consumer maintains as `tool_call_completed` events arrive (see
// evidence-context.tsx). Click a card to expand → fires a one-shot
// `/api/v1/web/workbench/actions/server-info/invoke` to pull alias /
// gpus / coolant headline, plus subscribes the card body to the
// metrics endpoint for sparklines.
//
// Why one-shot fetch + lazy expand instead of WebSocket-pushed full
// state: the host-detail-drawer (servers/page.tsx) already does the
// chart-heavy live polling for an opened host, and this card is meant
// to be a *peek*, not a dashboard. Cheap collapsed view, opt-in deep
// dive.

interface ServerSnapshot {
  hostId: string;
  alias?: string | null;
  host: string;
  cpuModel?: string | null;
  gpus: string[];
  lastOkAt?: string | null;
}

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function relativeAge(now: number, then: number): string {
  const diff = Math.max(0, Math.round((now - then) / 1000));
  if (diff < 5) return "just now";
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.round(diff / 60)}m ago`;
  return `${Math.round(diff / 3600)}h ago`;
}

function shortToolName(toolName: string): string {
  return toolName.replace(/^server\./, "");
}

/// Workbench result envelopes can wrap the gadget output as a raw
/// JSON value OR as an MCP-style `[{type:"text", text:"<json>"}]`
/// content array. Same logic as `(shell)/servers/page.tsx` — keep
/// it inline here so the cards can ship without a new shared util.
function unwrapWorkbenchPayload(resp: Record<string, unknown>): unknown {
  const payload = (resp as { result?: { payload?: unknown } }).result?.payload;
  if (Array.isArray(payload)) {
    const first = payload[0] as { text?: string } | undefined;
    if (first?.text) {
      try {
        return JSON.parse(first.text);
      } catch {
        return first.text;
      }
    }
  }
  return payload;
}

async function fetchHostSnapshot(
  apiKey: string | null,
  hostId: string,
): Promise<ServerSnapshot | null> {
  // server-info workbench action is the cheapest path to alias / gpus
  // / cpu without holding open a metrics polling loop. Returns the
  // serialized HostRecord — same shape as `~/.gadgetron/server-monitor/
  // inventory.json` rows.
  const headers: Record<string, string> = {
    "Content-Type": "application/json",
  };
  if (apiKey) headers["Authorization"] = `Bearer ${apiKey}`;
  // Endpoint shape mirrors `(shell)/servers/page.tsx::invokeAction` —
  // `/workbench/actions/{id}` (no `/invoke` suffix) with
  // `credentials: "include"` so the cookie session reaches the
  // gateway even when localStorage has no Bearer key.
  const res = await fetch(`${getApiBase()}/workbench/actions/server-info`, {
    method: "POST",
    headers,
    credentials: "include",
    body: JSON.stringify({ args: { id: hostId } }),
  });
  if (!res.ok) return null;
  const body = (await res.json()) as Record<string, unknown>;
  const parsed = unwrapWorkbenchPayload(body) as Record<string, unknown> | null;
  if (!parsed || typeof parsed !== "object") return null;
  // server.info returns the HostRecord plus, optionally, a
  // last-stats subobject. We only need the static identifying bits.
  const host = (parsed.host as string | undefined) ?? hostId;
  const alias = (parsed.alias as string | null | undefined) ?? null;
  const cpuModel = (parsed.cpu_model as string | null | undefined) ?? null;
  const gpus = Array.isArray(parsed.gpus)
    ? (parsed.gpus as unknown[]).filter(
        (x): x is string => typeof x === "string",
      )
    : [];
  const lastOkAt = (parsed.last_ok_at as string | null | undefined) ?? null;
  return { hostId, alias, host, cpuModel, gpus, lastOkAt };
}

function HostCard({ item }: { item: ServerContextItem }) {
  const { apiKey } = useAuth();
  const [expanded, setExpanded] = useState(false);
  const [snapshot, setSnapshot] = useState<ServerSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  // Refresh "Xs ago" once per second while the card is mounted. Cheap
  // (one setState per card per second; the stack is bounded by how
  // many distinct hosts the chat touched — usually < 5).
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);

  const onToggle = useCallback(async () => {
    setExpanded((prev) => !prev);
    if (snapshot || error) return;
    try {
      const snap = await fetchHostSnapshot(apiKey, item.hostId);
      if (snap) {
        setSnapshot(snap);
      } else {
        setError("host info unavailable");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [apiKey, item.hostId, snapshot, error]);

  const headerLabel =
    snapshot?.alias || snapshot?.host || `host ${item.hostId.slice(0, 8)}`;

  return (
    <div
      className="rounded border border-zinc-800 bg-zinc-950 text-xs text-zinc-300"
      data-testid="server-context-card"
      data-host-id={item.hostId}
    >
      <button
        type="button"
        onClick={onToggle}
        className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-zinc-900"
        aria-expanded={expanded}
        data-testid="server-context-card-toggle"
      >
        <span className="text-[11px] text-zinc-500" aria-hidden>
          {expanded ? "▾" : "▸"}
        </span>
        <span className="flex-1 truncate font-mono text-zinc-200">
          {headerLabel}
        </span>
        <span className="shrink-0 text-[10px] text-zinc-500">
          {shortToolName(item.lastToolName)} · {relativeAge(now, item.lastSeenAt)}
        </span>
      </button>
      {expanded && (
        <div className="border-t border-zinc-800 px-2 py-1.5">
          {error && (
            <div className="text-[11px] text-red-400">{error}</div>
          )}
          {!snapshot && !error && (
            <div className="text-[11px] text-zinc-500">loading…</div>
          )}
          {snapshot && (
            <dl className="grid grid-cols-[max-content_1fr] gap-x-2 gap-y-0.5 text-[11px]">
              <dt className="text-zinc-500">host</dt>
              <dd className="font-mono text-zinc-300">{snapshot.host}</dd>
              {snapshot.cpuModel && (
                <>
                  <dt className="text-zinc-500">cpu</dt>
                  <dd className="text-zinc-300">{snapshot.cpuModel}</dd>
                </>
              )}
              {snapshot.gpus.length > 0 && (
                <>
                  <dt className="text-zinc-500">gpu</dt>
                  <dd className="text-zinc-300">
                    {snapshot.gpus.length === 1
                      ? snapshot.gpus[0]
                      : `${snapshot.gpus.length}× ${snapshot.gpus[0]}`}
                  </dd>
                </>
              )}
              <dt className="text-zinc-500">last ok</dt>
              <dd className="font-mono text-zinc-400">
                {snapshot.lastOkAt
                  ? new Date(snapshot.lastOkAt).toLocaleString()
                  : "—"}
              </dd>
              <dt className="text-zinc-500">mentions</dt>
              <dd className="text-zinc-300">{item.mentionCount} this chat</dd>
            </dl>
          )}
          <a
            href={`/web/servers?host=${encodeURIComponent(item.hostId)}`}
            target="_blank"
            rel="noreferrer"
            className="mt-1.5 inline-block text-[11px] text-blue-400 hover:underline"
            data-testid="server-context-card-open-drawer"
          >
            open full dashboard →
          </a>
        </div>
      )}
    </div>
  );
}

export function ServerContextCardStack() {
  const { serverContext } = useEvidence();
  if (serverContext.length === 0) return null;
  return (
    <section
      className="flex flex-col gap-1"
      data-testid="server-context-stack"
      aria-label="Servers mentioned in this chat"
    >
      <header className="px-1 text-[10px] uppercase tracking-wider text-zinc-500">
        Servers in chat ({serverContext.length})
      </header>
      {serverContext.map((item) => (
        <HostCard key={item.hostId} item={item} />
      ))}
    </section>
  );
}
