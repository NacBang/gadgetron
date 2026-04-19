"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import Link from "next/link";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { Card, CardContent } from "../components/ui/card";

// ---------------------------------------------------------------------------
// /web/dashboard — operator observability surface (ISSUE 4).
//
// Reads GET /api/v1/web/workbench/usage/summary on mount + opens a
// WebSocket to /events/ws for live activity. No chat, no wiki CRUD —
// just the "what's happening" panel an operator can keep open.
// ---------------------------------------------------------------------------

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function wsUrlFromHttp(httpBase: string, actorKey: string): string {
  // http://host:port/api/v1/web/workbench/events/ws?token=…
  // ws:// or wss:// depending on source scheme.
  if (typeof location === "undefined") return "";
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const host = location.host;
  // Token in query string is the only option — WebSocket handshake
  // can't set Authorization headers from the browser. Server reads
  // it via the standard auth middleware (same token either header
  // or ?token= query arg; see auth middleware doc).
  const base = `${scheme}//${host}${httpBase}/workbench/events/ws`;
  return `${base}?token=${encodeURIComponent(actorKey)}`;
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

function useApiKey(): [string | null, () => void] {
  const [key, setKey] = useState<string | null>(null);
  useEffect(() => {
    const stored = localStorage.getItem("gadgetron_api_key");
    if (stored) setKey(stored);
  }, []);
  const clear = useCallback(() => {
    localStorage.removeItem("gadgetron_api_key");
    setKey(null);
  }, []);
  return [key, clear];
}

export default function DashboardPage() {
  const [apiKey, clearKey] = useApiKey();
  const [keyInput, setKeyInput] = useState("");
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [events, setEvents] = useState<LiveEvent[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "disconnected" | "connecting" | "open" | "closed"
  >("disconnected");
  const wsRef = useRef<WebSocket | null>(null);

  const refreshSummary = useCallback(async () => {
    if (!apiKey) return;
    setSummaryError(null);
    try {
      const res = await fetch(`${getApiBase()}/workbench/usage/summary`, {
        headers: { Authorization: `Bearer ${apiKey}` },
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
    if (apiKey) void refreshSummary();
  }, [apiKey, refreshSummary]);

  // Live WebSocket subscription. Reconnects on close; drops on unmount.
  useEffect(() => {
    if (!apiKey) return;
    let closed = false;
    const connect = () => {
      if (closed) return;
      setWsStatus("connecting");
      const socket = new WebSocket(wsUrlFromHttp(getApiBase(), apiKey));
      wsRef.current = socket;
      socket.onopen = () => setWsStatus("open");
      socket.onclose = () => {
        setWsStatus("closed");
        // Auto-reconnect after 3s unless the component unmounted.
        if (!closed) setTimeout(connect, 3000);
      };
      socket.onerror = () => {
        // onclose runs after onerror; the reconnect happens there.
      };
      socket.onmessage = (msg) => {
        try {
          const parsed = JSON.parse(msg.data) as LiveEvent;
          setEvents((prev) => {
            // Keep the last 100 events; the oldest slide off the top.
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

  if (!apiKey) {
    return (
      <div
        className="flex min-h-screen items-center justify-center bg-zinc-950 p-6"
        data-testid="dashboard-auth-gate"
      >
        <Card className="w-full max-w-md border-zinc-800 bg-zinc-900">
          <CardContent className="flex flex-col gap-4 p-6">
            <div>
              <h1 className="text-sm font-semibold text-zinc-200">
                Gadgetron Dashboard
              </h1>
              <p className="mt-1 text-xs text-zinc-500">
                Paste the API key generated by{" "}
                <code className="rounded bg-zinc-800 px-1 py-0.5 font-mono text-[11px] text-zinc-400">
                  gadgetron key create
                </code>
                .
              </p>
            </div>
            <Input
              type="password"
              value={keyInput}
              onChange={(e) => setKeyInput(e.target.value)}
              placeholder="gad_live_..."
              onKeyDown={(e) => {
                if (e.key === "Enter" && keyInput.trim()) {
                  localStorage.setItem("gadgetron_api_key", keyInput.trim());
                  location.reload();
                }
              }}
              className="border-zinc-700 bg-zinc-800 font-mono text-xs text-zinc-200 placeholder:text-zinc-600"
            />
            <Button
              onClick={() => {
                if (keyInput.trim()) {
                  localStorage.setItem("gadgetron_api_key", keyInput.trim());
                  location.reload();
                }
              }}
              className="w-full"
            >
              Sign in
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div
      className="flex h-screen flex-col bg-zinc-950 text-zinc-100"
      data-testid="dashboard"
    >
      <header className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 bg-zinc-950 px-4">
        <div className="flex items-center gap-3">
          <Link
            href="/"
            className="text-[11px] text-zinc-500 transition-colors hover:text-zinc-300"
          >
            ← Workbench
          </Link>
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
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              clearKey();
              location.reload();
            }}
            className="h-6 px-2 text-[11px] text-red-400 hover:text-red-300"
          >
            Sign out
          </Button>
        </div>
      </header>

      <div className="flex flex-1 overflow-hidden">
        {/* Left: usage tiles */}
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

        {/* Right: live feed */}
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
    </div>
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
