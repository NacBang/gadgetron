"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { Card, CardContent } from "../../components/ui/card";
import { useAuth } from "../../lib/auth-context";

// ---------------------------------------------------------------------------
// /web/dashboard — operator observability surface. Runs inside
// `(shell)/layout.tsx`; supplies only the dashboard header + tiles +
// live-feed right column. Auth gate / outer chrome live in the shell.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function wsUrlFromHttp(httpBase: string, actorKey: string | null): string {
  if (typeof location === "undefined") return "";
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const host = location.host;
  const base = `${scheme}//${host}${httpBase}/workbench/events/ws`;
  // With a session cookie, the browser sends it automatically on the
  // WebSocket upgrade and the gateway middleware resolves it there.
  // Fall back to `?token=` when an API key is the only auth we have.
  return actorKey ? `${base}?token=${encodeURIComponent(actorKey)}` : base;
}

type UsageSummary = {
  window_hours: number;
  chat: {
    requests: number;
    errors: number;
    total_input_tokens: number;
    total_output_tokens: number;
    total_cost_cents: number;
    avg_latency_ms: number;
  };
  actions: {
    total: number;
    success: number;
    error: number;
    pending_approval: number;
    avg_elapsed_ms: number;
  };
  tools: { total: number; errors: number };
};

type LiveEvent = {
  type: string;
  [k: string]: unknown;
};

export default function DashboardPage() {
  const { apiKey } = useAuth();
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [events, setEvents] = useState<LiveEvent[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "disconnected" | "connecting" | "open" | "closed"
  >("disconnected");
  const wsRef = useRef<WebSocket | null>(null);

  const refreshSummary = useCallback(async () => {
    setSummaryError(null);
    try {
      const res = await fetch(`${getApiBase()}/workbench/usage/summary`, {
        credentials: "include", headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
      });
      if (!res.ok) {
        const text = await res.text();
        throw new Error(`${res.status} ${text.slice(0, 200)}`);
      }
      const json = (await res.json()) as UsageSummary;
      setSummary(json);
    } catch (e) {
      setSummaryError((e as Error).message);
    }
  }, [apiKey]);

  useEffect(() => {
    void refreshSummary();
  }, [apiKey, refreshSummary]);

  // Live WebSocket subscription. Reconnects on close; drops on unmount.
  useEffect(() => {
    let closed = false;
    const connect = () => {
      if (closed) return;
      setWsStatus("connecting");
      const socket = new WebSocket(wsUrlFromHttp(getApiBase(), apiKey));
      wsRef.current = socket;
      socket.onopen = () => setWsStatus("open");
      socket.onclose = () => {
        setWsStatus("closed");
        if (!closed) setTimeout(connect, 3000);
      };
      socket.onerror = () => {
        // onclose runs after onerror; the reconnect happens there.
      };
      socket.onmessage = (msg) => {
        try {
          const parsed = JSON.parse(msg.data) as LiveEvent;
          setEvents((prev) => {
            const next = [parsed, ...prev];
            return next.slice(0, 100);
          });
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
    <>
      <header
        className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4"
        data-testid="dashboard-header"
      >
        <div className="flex items-center gap-3">
          <span className="text-xs font-semibold text-zinc-300">
            Operator Dashboard
          </span>
          <span
            className="text-[11px] text-zinc-600"
            data-testid="dashboard-window-label"
          >
            · last {summary?.window_hours ?? 24}h
          </span>
          <span
            data-testid="dashboard-ws-status"
            className={`rounded border px-1.5 py-0.5 font-mono text-[10px] ${
              wsStatus === "open"
                ? "border-emerald-700/40 bg-emerald-900/20 text-emerald-400"
                : wsStatus === "connecting"
                  ? "border-amber-700/40 bg-amber-900/20 text-amber-400"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500"
            }`}
          >
            ws: {wsStatus}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => void refreshSummary()}
            className="h-6 px-2 text-[11px]"
          >
            Refresh
          </Button>
        </div>
      </header>

      <div className="flex flex-1 overflow-hidden">
        {/* Usage tiles */}
        <main className="flex-1 overflow-auto p-4">
          {summaryError && (
            <div className="mb-3 rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
              {summaryError}
            </div>
          )}
          {!summary && !summaryError && (
            <div className="text-[11px] text-zinc-600">Loading summary…</div>
          )}
          {summary && (
            <div
              className="grid grid-cols-1 gap-3 md:grid-cols-3"
              data-testid="dashboard-tiles"
            >
              <Tile
                testId="tile-chat"
                title="Chat"
                primary={`${summary.chat.requests}`}
                primaryLabel="requests"
                sub={[
                  [
                    "tokens",
                    `${summary.chat.total_input_tokens + summary.chat.total_output_tokens}`,
                  ],
                  [
                    "cost",
                    `$${(summary.chat.total_cost_cents / 100).toFixed(2)}`,
                  ],
                  [
                    "avg latency",
                    `${summary.chat.avg_latency_ms.toFixed(0)}ms`,
                  ],
                  ["errors", `${summary.chat.errors}`],
                ]}
              />
              <Tile
                testId="tile-actions"
                title="Actions"
                primary={`${summary.actions.total}`}
                primaryLabel="invocations"
                sub={[
                  ["success", `${summary.actions.success}`],
                  ["error", `${summary.actions.error}`],
                  ["pending", `${summary.actions.pending_approval}`],
                  [
                    "avg elapsed",
                    `${summary.actions.avg_elapsed_ms.toFixed(0)}ms`,
                  ],
                ]}
              />
              <Tile
                testId="tile-tools"
                title="Tools"
                primary={`${summary.tools.total}`}
                primaryLabel="calls"
                sub={[["errors", `${summary.tools.errors}`]]}
              />
            </div>
          )}
        </main>

        {/* Live feed */}
        <aside
          data-testid="dashboard-live-feed"
          className="flex w-80 shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
        >
          <div className="shrink-0 border-b border-zinc-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
            Live feed ({events.length})
          </div>
          <div className="flex-1 overflow-y-auto">
            {events.length === 0 && (
              <div className="px-3 py-2 text-[11px] text-zinc-600">
                Waiting for events…
              </div>
            )}
            {events.map((e, i) => (
              <div
                key={i}
                className="border-b border-zinc-900 px-3 py-2 text-[11px]"
                data-testid="dashboard-live-event"
              >
                <div className="font-mono text-zinc-300">{String(e.type)}</div>
                <pre className="mt-1 whitespace-pre-wrap break-all text-zinc-500">
                  {JSON.stringify(e, null, 0).slice(0, 180)}
                </pre>
              </div>
            ))}
          </div>
        </aside>
      </div>
    </>
  );
}

function Tile(props: {
  testId: string;
  title: string;
  primary: string;
  primaryLabel: string;
  sub: Array<[string, string]>;
}) {
  return (
    <Card
      className="border-zinc-800 bg-zinc-900/50"
      data-testid={props.testId}
    >
      <CardContent className="flex flex-col gap-3 p-4">
        <div className="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
          {props.title}
        </div>
        <div className="flex items-baseline gap-2">
          <span className="font-mono text-3xl font-semibold text-zinc-100">
            {props.primary}
          </span>
          <span className="text-[11px] text-zinc-500">
            {props.primaryLabel}
          </span>
        </div>
        <dl className="grid grid-cols-2 gap-1.5 text-[11px]">
          {props.sub.map(([k, v]) => (
            <div key={k} className="flex flex-col">
              <dt className="text-zinc-600">{k}</dt>
              <dd className="font-mono text-zinc-300">{v}</dd>
            </div>
          ))}
        </dl>
      </CardContent>
    </Card>
  );
}
