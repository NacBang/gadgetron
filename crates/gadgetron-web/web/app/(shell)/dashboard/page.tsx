"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "../../components/ui/button";
import { Card, CardContent } from "../../components/ui/card";
import {
  InlineNotice,
  PageToolbar,
  StatusBadge,
  WorkbenchPage,
} from "../../components/workbench";
import { useAuth } from "../../lib/auth-context";
import { getApiBase, invokeAction, unwrapPayload } from "../../lib/workbench-client";

// ---------------------------------------------------------------------------
// /web/dashboard — operator observability surface. Runs inside
// `(shell)/layout.tsx`; supplies only the dashboard header + tiles +
// live-feed right column. Auth gate / outer chrome live in the shell.
// ---------------------------------------------------------------------------

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

// Fleet summary — the `server-fleet` action payload (ISSUE 46). The
// dashboard leads with the REGISTERED SERVERS' health, not gadgetron's
// own usage counters (user direction 2026-06-11); usage numbers remain
// available via /workbench/usage/summary for the admin surface.
type FleetSummary = {
  generated_at: string;
  /** False in legacy no-DB mode — online/offline counts are then
   * meaningless and the UI renders "unknown" instead (ISSUE 50). */
  snapshots_available?: boolean;
  servers: { total: number; online: number; offline: number };
  gpus: {
    count: number;
    avg_util_pct: number | null;
    max_temp_c: number | null;
    total_power_w: number | null;
  };
  cpu: { avg_util_pct: number | null };
  mem: { used_bytes: number; total_bytes: number };
  warnings: number;
  hosts: Array<{
    id: string;
    host: string;
    alias: string | null;
    online: boolean;
    cpu_util_pct: number | null;
    gpu_count: number;
    gpu_avg_util_pct: number | null;
    gpu_max_temp_c: number | null;
    warnings: number;
  }>;
};

async function fetchFleet(apiKey: string | null): Promise<FleetSummary> {
  return unwrapPayload(
    await invokeAction(apiKey, "server-fleet", {}),
  ) as FleetSummary;
}

function fmtGiB(bytes: number): string {
  return `${(bytes / 1024 ** 3).toFixed(0)} GiB`;
}

type LiveEvent = {
  type: string;
  [k: string]: unknown;
};

export default function DashboardPage() {
  const { apiKey } = useAuth();
  const [summary, setSummary] = useState<FleetSummary | null>(null);
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [events, setEvents] = useState<LiveEvent[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "disconnected" | "connecting" | "open" | "closed"
  >("disconnected");
  const wsRef = useRef<WebSocket | null>(null);

  const refreshSummary = useCallback(async () => {
    setSummaryError(null);
    try {
      setSummary(await fetchFleet(apiKey));
    } catch (e) {
      setSummaryError((e as Error).message);
    }
  }, [apiKey]);

  // Snapshot rows refresh at poller cadence; 10 s keeps the tiles live
  // without hammering the action endpoint.
  useEffect(() => {
    void refreshSummary();
    const t = window.setInterval(() => void refreshSummary(), 10_000);
    return () => window.clearInterval(t);
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

  const connected = wsStatus === "open";

  return (
    <WorkbenchPage
      title="Dashboard"
      subtitle="Registered-fleet health at a glance, plus live operational events."
      headerTestId="dashboard-header"
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void refreshSummary()}
          className="h-7 px-2 text-[11px]"
        >
          Refresh
        </Button>
      }
      toolbar={
        <PageToolbar
          status={
            <StatusBadge status={connected ? "healthy" : "degraded"} />
          }
        >
          <span className="text-xs text-zinc-500">
            Fleet status and live feed
          </span>
          <span
            className="text-[11px] text-zinc-600"
            data-testid="dashboard-window-label"
          >
            {summary
              ? `${summary.servers.online}/${summary.servers.total} online`
              : "—"}
          </span>
          <span
            data-testid="dashboard-ws-status"
            className="rounded border border-zinc-800 bg-zinc-900 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400"
          >
            ws: {wsStatus}
          </span>
        </PageToolbar>
      }
    >
      <div className="space-y-3">
        {!connected && (
          <InlineNotice tone="warn" title="Live feed disconnected">
            Gadgetron will keep retrying the activity stream.
          </InlineNotice>
        )}
        {summaryError && (
          <InlineNotice
            tone="error"
            title="Fleet summary request failed"
            details={summaryError}
          >
            Gadgetron could not load the registered-server summary.
          </InlineNotice>
        )}
        <div className="flex min-h-[520px] overflow-hidden rounded border border-zinc-800">
          <main className="flex-1 overflow-auto bg-zinc-950/30 p-4">
            {!summary && !summaryError && (
              <div className="text-[11px] text-zinc-600">
                Loading summary...
              </div>
            )}
            {summary && (
              <div className="flex flex-col gap-3">
                <div
                  className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4"
                  data-testid="dashboard-tiles"
                >
                  <Tile
                    testId="tile-servers"
                    title="서버"
                    primary={`${summary.servers.online}/${summary.servers.total}`}
                    primaryLabel="online"
                    sub={[
                      ["online", `${summary.servers.online}`],
                      ["offline", `${summary.servers.offline}`],
                    ]}
                  />
                  <Tile
                    testId="tile-gpus"
                    title="GPU"
                    primary={`${summary.gpus.count}`}
                    primaryLabel="gpus"
                    sub={[
                      [
                        "avg util",
                        summary.gpus.avg_util_pct != null
                          ? `${summary.gpus.avg_util_pct.toFixed(0)}%`
                          : "—",
                      ],
                      [
                        "max temp",
                        summary.gpus.max_temp_c != null
                          ? `${summary.gpus.max_temp_c.toFixed(0)}°C`
                          : "—",
                      ],
                      [
                        "power",
                        summary.gpus.total_power_w != null
                          ? `${summary.gpus.total_power_w.toFixed(0)}W`
                          : "—",
                      ],
                    ]}
                  />
                  <Tile
                    testId="tile-resources"
                    title="자원"
                    primary={
                      summary.cpu.avg_util_pct != null
                        ? `${summary.cpu.avg_util_pct.toFixed(0)}%`
                        : "—"
                    }
                    primaryLabel="avg cpu"
                    sub={[
                      [
                        "mem",
                        summary.mem.total_bytes > 0
                          ? `${fmtGiB(summary.mem.used_bytes)} / ${fmtGiB(summary.mem.total_bytes)}`
                          : "—",
                      ],
                      [
                        "mem %",
                        summary.mem.total_bytes > 0
                          ? `${((summary.mem.used_bytes / summary.mem.total_bytes) * 100).toFixed(0)}%`
                          : "—",
                      ],
                    ]}
                  />
                  <a
                    href="/web/findings"
                    className="block"
                    title="로그 분석 findings 열기"
                  >
                    <Tile
                      testId="tile-warnings"
                      title="경고"
                      primary={`${summary.warnings}`}
                      primaryLabel="warnings"
                      sub={[["findings", "열기 →"]]}
                    />
                  </a>
                </div>
                {/* Per-host status strip — one dot per server (green =
                  * online, red = offline, amber ring = warnings). Scales
                  * to hundreds of hosts; click → /web/servers. */}
                {summary.hosts.length > 0 && (
                  <div
                    className="rounded border border-zinc-800 bg-zinc-900/40 p-3"
                    data-testid="dashboard-host-strip"
                  >
                    <div className="mb-2 text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
                      Hosts ({summary.hosts.length})
                    </div>
                    <div className="flex flex-wrap gap-1.5">
                      {summary.hosts.map((h) => (
                        <a
                          key={h.id}
                          href="/web/servers"
                          data-testid="dashboard-host-dot"
                          title={`${h.alias ?? h.host}${h.online ? "" : " · offline"}${
                            h.cpu_util_pct != null
                              ? ` · CPU ${h.cpu_util_pct.toFixed(0)}%`
                              : ""
                          }${
                            h.gpu_count > 0
                              ? ` · GPU×${h.gpu_count}${
                                  h.gpu_avg_util_pct != null
                                    ? ` ${h.gpu_avg_util_pct.toFixed(0)}%`
                                    : ""
                                }`
                              : ""
                          }${h.warnings > 0 ? ` · ⚠${h.warnings}` : ""}`}
                          className={`size-3 rounded-sm ${
                            summary.snapshots_available === false
                              ? "bg-zinc-500"
                              : h.online
                                ? "bg-emerald-500"
                                : "bg-red-600"
                          } ${h.warnings > 0 ? "ring-1 ring-amber-400" : ""}`}
                        />
                      ))}
                    </div>
                  </div>
                )}
              </div>
            )}
          </main>

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
                  Waiting for events...
                </div>
              )}
              {events.map((e, i) => (
                <div
                  key={i}
                  className="border-b border-zinc-900 px-3 py-2 text-[11px]"
                  data-testid="dashboard-live-event"
                >
                  <div className="font-mono text-zinc-300">
                    {String(e.type)}
                  </div>
                  <pre className="mt-1 whitespace-pre-wrap break-all text-zinc-500">
                    {JSON.stringify(e, null, 0).slice(0, 180)}
                  </pre>
                </div>
              ))}
            </div>
          </aside>
        </div>
      </div>
    </WorkbenchPage>
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
