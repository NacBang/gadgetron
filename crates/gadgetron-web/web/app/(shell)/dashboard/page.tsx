"use client";

import Link from "next/link";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  AlertTriangle,
  BrainCircuit,
  CheckCircle2,
  CircleDot,
  Clock3,
  RefreshCw,
  ShieldAlert,
  Workflow,
} from "lucide-react";

import { Button } from "../../components/ui/button";
import { Card, CardContent } from "../../components/ui/card";
import {
  DeclarativeRenderer,
  EmptyState,
  InlineNotice,
  WorkbenchPage,
} from "../../components/workbench";
import { useAuth } from "../../lib/auth-context";
import {
  fetchContributionData,
  useCapabilities,
  type UiContribution,
} from "../../lib/capability-context";
import { getApiBase } from "../../lib/workbench-client";
import { useI18n } from "../../lib/i18n";

type CoreBootstrap = {
  gateway_version: string;
  active_plugs: Array<{ id: string; role: string; healthy: boolean }>;
  degraded_reasons: string[];
  knowledge: {
    canonical_ready: boolean;
    search_ready: boolean;
    relation_ready: boolean;
  };
};

type CoreDashboardSnapshot = {
  bootstrap: CoreBootstrap;
  activeJobs: number;
  pendingReview: number;
};

type LiveEvent = { type: string; [key: string]: unknown };

function authHeaders(apiKey: string | null): HeadersInit {
  return apiKey ? { Authorization: "Bearer " + apiKey } : {};
}

async function readJson<T>(url: string, apiKey: string | null): Promise<T> {
  const response = await fetch(url, {
    credentials: "include",
    headers: authHeaders(apiKey),
  });
  if (!response.ok) throw new Error("HTTP " + response.status);
  return (await response.json()) as T;
}

async function loadSnapshot(apiKey: string | null): Promise<CoreDashboardSnapshot> {
  const base = getApiBase() + "/workbench";
  const [bootstrap, jobs, review] = await Promise.all([
    readJson<CoreBootstrap>(base + "/bootstrap", apiKey),
    readJson<{ jobs?: unknown[] }>(base + "/jobs/active", apiKey),
    readJson<{ count?: number; approvals?: unknown[] }>(
      base + "/approvals/pending",
      apiKey,
    ),
  ]);
  return {
    bootstrap,
    activeJobs: Array.isArray(jobs.jobs) ? jobs.jobs.length : 0,
    pendingReview:
      typeof review.count === "number"
        ? review.count
        : Array.isArray(review.approvals)
          ? review.approvals.length
          : 0,
  };
}

function websocketUrl(apiKey: string | null): string {
  if (typeof location === "undefined") return "";
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  const base =
    scheme + "//" + location.host + getApiBase() + "/workbench/events/ws";
  return apiKey ? base + "?token=" + encodeURIComponent(apiKey) : base;
}

export default function DashboardPage() {
  const { labels } = useI18n();
  const { apiKey } = useAuth();
  const { snapshot: capabilities, status: capabilityStatus } = useCapabilities();
  const [snapshot, setSnapshot] = useState<CoreDashboardSnapshot | null>(null);
  const [snapshotError, setSnapshotError] = useState(false);
  const [events, setEvents] = useState<LiveEvent[]>([]);
  const [wsStatus, setWsStatus] = useState<
    "connecting" | "open" | "closed"
  >("connecting");
  const wsRef = useRef<WebSocket | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSnapshot(await loadSnapshot(apiKey));
      setSnapshotError(false);
    } catch {
      setSnapshotError(true);
    }
  }, [apiKey]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    let reconnect: ReturnType<typeof setTimeout> | null = null;
    const connect = () => {
      if (disposed) return;
      setWsStatus("connecting");
      const socket = new WebSocket(websocketUrl(apiKey));
      wsRef.current = socket;
      socket.onopen = () => setWsStatus("open");
      socket.onclose = () => {
        setWsStatus("closed");
        if (!disposed) reconnect = setTimeout(connect, 3_000);
      };
      socket.onmessage = (message) => {
        try {
          const event = JSON.parse(message.data) as LiveEvent;
          setEvents((previous) => [event, ...previous].slice(0, 100));
        } catch {
          // Invalid external frames do not enter the human activity view.
        }
      };
    };
    connect();
    return () => {
      disposed = true;
      if (reconnect) clearTimeout(reconnect);
      wsRef.current?.close();
    };
  }, [apiKey]);

  const coreHealthy = Boolean(
    snapshot && snapshot.bootstrap.degraded_reasons.length === 0,
  );
  const knowledgePlanes = snapshot
    ? [
        snapshot.bootstrap.knowledge.canonical_ready,
        snapshot.bootstrap.knowledge.search_ready,
        snapshot.bootstrap.knowledge.relation_ready,
      ].filter(Boolean).length
    : 0;
  const widgets = useMemo(
    () =>
      capabilities.ui_contributions
        .filter(
          (item) =>
            item.kind === "dashboard_widget" &&
            item.placement === "dashboard" &&
            item.renderer,
        )
        .sort(
          (left, right) =>
            left.order_hint - right.order_hint || left.id.localeCompare(right.id),
        ),
    [capabilities.ui_contributions],
  );
  const headline = snapshotError
    ? "Current status is unavailable"
    : !snapshot
      ? "Reading current operations"
      : !coreHealthy
        ? "Core needs attention"
        : snapshot.pendingReview > 0
          ? snapshot.pendingReview +
            (snapshot.pendingReview === 1
              ? " decision needs review"
              : " decisions need review")
          : "Operations are steady";
  const attentionCount =
    (snapshot?.bootstrap.degraded_reasons.length ?? 0) +
    (snapshot?.pendingReview ?? 0);

  return (
    <WorkbenchPage
      title="Dashboard"
      headerTestId="dashboard-header"
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => void refresh()}
          className="h-8 px-2 text-xs"
        >
          <RefreshCw className="size-3.5" aria-hidden />
          Refresh
        </Button>
      }
    >
      <div className="space-y-4 p-3">
        <section
          className="border border-zinc-800 bg-[#101418]"
          aria-labelledby="mission-status-heading"
        >
          <div className="grid gap-px bg-zinc-800 lg:grid-cols-[minmax(260px,1.35fr)_minmax(0,2fr)]">
            <div className="bg-[#101418] p-5">
              <div className="text-xs font-semibold uppercase tracking-[0.16em] text-zinc-400">
                Mission status
              </div>
              <div className="mt-4 flex items-start gap-3">
                {coreHealthy && !snapshotError ? (
                  <CheckCircle2 className="mt-0.5 size-5 shrink-0 text-zinc-500" aria-hidden />
                ) : (
                  <AlertTriangle className="mt-0.5 size-5 shrink-0 text-amber-400" aria-hidden />
                )}
                <div>
                  <h2 id="mission-status-heading" className="text-xl font-semibold text-zinc-100">
                    {headline}
                  </h2>
                  <div className="mt-2 flex items-center gap-2 text-xs text-zinc-400">
                    <CircleDot
                      className={
                        "size-3 " +
                        (wsStatus === "open" ? "text-zinc-500" : "text-amber-400")
                      }
                      aria-hidden
                    />
                    {wsStatus === "open"
                      ? "Live updates connected"
                      : "Live updates reconnecting"}
                  </div>
                </div>
              </div>
            </div>
            <dl
              className="grid bg-[#101418] sm:grid-cols-2 xl:grid-cols-4"
              data-testid="dashboard-vitals"
            >
              <Vital
                testId="vital-core"
                icon={ShieldAlert}
                label="System"
                value={snapshot ? (coreHealthy ? "Ready" : "Attention") : "—"}
                context={snapshot?.bootstrap.gateway_version ?? "Loading"}
                attention={!coreHealthy && Boolean(snapshot)}
              />
              <Vital
                testId="vital-review"
                icon={ShieldAlert}
                label="Review"
                value={snapshot ? String(snapshot.pendingReview) : "—"}
                context="manager decisions"
                attention={(snapshot?.pendingReview ?? 0) > 0}
              />
              <Vital
                testId="vital-jobs"
                icon={Workflow}
                label="Active work"
                value={snapshot ? String(snapshot.activeJobs) : "—"}
                context="background jobs"
              />
              <Vital
                testId="vital-knowledge"
                icon={BrainCircuit}
                label="Knowledge"
                value={snapshot ? knowledgePlanes + " / 3" : "—"}
                context={labels.dashboard.knowledgeFeaturesAvailable}
                attention={Boolean(snapshot && knowledgePlanes < 3)}
              />
            </dl>
          </div>
        </section>

        {snapshotError && (
          <InlineNotice tone="error" title="Core status unavailable">
            Last known values are not presented as current.
          </InlineNotice>
        )}
        {capabilityStatus === "degraded" && (
          <InlineNotice tone="warn" title="Domain overview may be stale">
            The last complete signed layout remains visible.
          </InlineNotice>
        )}

        {attentionCount > 0 && snapshot && (
          <section aria-labelledby="attention-heading">
            <SectionHeading
              id="attention-heading"
              title="Needs attention"
              value={String(attentionCount)}
            />
            <div className="grid gap-2 md:grid-cols-2">
              {snapshot.pendingReview > 0 && (
                <AttentionItem
                  icon={ShieldAlert}
                  title={
                    snapshot.pendingReview === 1
                      ? "1 decision is waiting"
                      : snapshot.pendingReview + " decisions are waiting"
                  }
                  action="Open Review"
                  href="/review"
                />
              )}
              {snapshot.bootstrap.degraded_reasons.map((reason, index) => (
                <AttentionItem
                  key={index}
                  icon={AlertTriangle}
                  title={boundedText(reason, "Core dependency needs attention")}
                  action="Inspect system"
                  href="/admin"
                />
              ))}
            </div>
          </section>
        )}

        <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_340px]">
          <section aria-labelledby="domain-overview-heading">
            <SectionHeading
              id="domain-overview-heading"
              title="Domain overview"
              value={String(widgets.length)}
            />
            {widgets.length > 0 ? (
              <div
                className="grid grid-cols-1 gap-3 lg:grid-cols-2"
                aria-label="Bundle dashboard widgets"
              >
                {widgets.map((widget) => (
                  <BundleDashboardWidget
                    key={widget.id}
                    contribution={widget}
                    revision={capabilities.revision}
                    apiKey={apiKey}
                  />
                ))}
              </div>
            ) : (
              <Card className="border-zinc-800 bg-zinc-950/40">
                <CardContent className="flex items-center justify-between gap-4 p-4">
                  <span className="text-sm text-zinc-400">No domain overview is enabled.</span>
                  <Link
                    href="/admin"
                    className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
                  >
                    Manage Bundles
                  </Link>
                </CardContent>
              </Card>
            )}
          </section>

          <section aria-labelledby="recent-activity-heading">
            <SectionHeading
              id="recent-activity-heading"
              title="Recent activity"
              value={events.length > 0 ? String(Math.min(events.length, 12)) : undefined}
            />
            <Card className="border-zinc-800 bg-zinc-950/40">
              <CardContent className="p-0">
                <div
                  className="max-h-[34rem] divide-y divide-zinc-900 overflow-y-auto"
                  data-testid="dashboard-live-feed"
                >
                  {events.length === 0 ? (
                    <div className="flex min-h-28 items-center gap-3 px-4 text-sm text-zinc-400">
                      <Activity className="size-4" aria-hidden />
                      No recent activity
                    </div>
                  ) : (
                    events.slice(0, 12).map((event, index) => (
                      <ActivityRow
                        key={String(event.type) + "-" + index}
                        event={event}
                      />
                    ))
                  )}
                </div>
              </CardContent>
            </Card>
          </section>
        </div>
      </div>
    </WorkbenchPage>
  );
}

function Vital({
  testId,
  icon: Icon,
  label,
  value,
  context,
  attention = false,
}: {
  testId: string;
  icon: typeof Activity;
  label: string;
  value: string;
  context: string;
  attention?: boolean;
}) {
  return (
    <div
      className="min-w-0 border-b border-zinc-800 p-4 last:border-b-0 sm:border-r sm:[&:nth-child(2n)]:border-r-0 xl:border-b-0 xl:[&:nth-child(2n)]:border-r xl:last:border-r-0"
      data-testid={testId}
    >
      <dt className="flex items-center gap-2 text-xs font-semibold uppercase tracking-[0.12em] text-zinc-400">
        <Icon className="size-3.5" aria-hidden />
        {label}
      </dt>
      <dd
        className={
          "mt-3 font-mono text-xl font-semibold " +
          (attention ? "text-amber-300" : "text-zinc-100")
        }
      >
        {value}
      </dd>
      <dd className="mt-1 text-xs text-zinc-400">{context}</dd>
    </div>
  );
}

function SectionHeading({
  id,
  title,
  value,
}: {
  id: string;
  title: string;
  value?: string;
}) {
  return (
    <div className="mb-2 flex h-7 items-center gap-2">
      <h2
        id={id}
        className="text-xs font-semibold uppercase tracking-[0.14em] text-zinc-400"
      >
        {title}
      </h2>
      {value !== undefined && (
        <span className="font-mono text-xs text-zinc-400">{value}</span>
      )}
    </div>
  );
}

function AttentionItem({
  icon: Icon,
  title,
  action,
  href,
}: {
  icon: typeof Activity;
  title: string;
  action: string;
  href: string;
}) {
  return (
    <Link
      href={href}
      className="flex min-h-14 items-center gap-3 border border-[#B8733355] bg-[#B873330c] px-3 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
    >
      <Icon className="size-4 shrink-0 text-[#D89B5A]" aria-hidden />
      <span className="min-w-0 flex-1 truncate text-sm text-zinc-200">{title}</span>
      <span className="shrink-0 text-xs text-[#D89B5A]">{action}</span>
    </Link>
  );
}

function BundleDashboardWidget({
  contribution,
  revision,
  apiKey,
}: {
  contribution: UiContribution;
  revision: string;
  apiKey: string | null;
}) {
  const { labels } = useI18n();
  const [payload, setPayload] = useState<unknown>(null);
  const [state, setState] = useState<"loading" | "ready" | "error">("loading");
  const emptyServerFleet = isEmptyServerFleet(contribution, payload);

  useEffect(() => {
    let disposed = false;
    const load = async () => {
      try {
        const data = await fetchContributionData(apiKey, contribution.id);
        if (disposed) return;
        if (data.capability_revision === revision) {
          setPayload(data.payload);
          setState("ready");
        } else {
          setPayload(null);
          setState("error");
        }
      } catch {
        if (!disposed) {
          setPayload(null);
          setState("error");
        }
      }
    };
    setPayload(null);
    setState("loading");
    void load();
    const timer = window.setInterval(
      () => void load(),
      Math.max(5, contribution.refresh_seconds ?? 15) * 1000,
    );
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [apiKey, contribution.id, contribution.refresh_seconds, revision]);

  return (
    <Card
      className="overflow-hidden border-zinc-800 bg-zinc-950/40"
      data-testid={"bundle-widget-" + contribution.id}
    >
      <CardContent className="p-0">
        <div className="flex h-11 items-center justify-between border-b border-zinc-800 px-3">
          <h3 className="text-sm font-medium text-zinc-200">{contribution.label}</h3>
          {state === "loading" && <RefreshCw className="size-3.5 animate-spin text-zinc-600" aria-label="Loading" />}
          {state === "error" && <AlertTriangle className="size-3.5 text-amber-400" aria-label="Unavailable" />}
        </div>
        {state === "loading" ? (
          <div className="grid grid-cols-2 gap-px bg-zinc-900 p-px">
            <div className="h-20 animate-pulse bg-zinc-950" />
            <div className="h-20 animate-pulse bg-zinc-950" />
          </div>
        ) : state === "error" ? (
          <div className="p-4 text-sm text-amber-200">{contribution.error_state}</div>
        ) : payload === null ? (
          <div className="p-4 text-sm text-zinc-400">{contribution.empty_state}</div>
        ) : emptyServerFleet ? (
          <EmptyState
            className="m-3 min-h-32"
            title={labels.emptyStates.dashboardNoServersTitle}
            description={labels.emptyStates.dashboardNoServersDescription}
            action={(
              <Link
                href="/workspace?id=server-administrator.fleet"
                className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
              >
                {labels.emptyStates.dashboardStartInFleet}
              </Link>
            )}
          />
        ) : (
          <DeclarativeRenderer renderer={contribution.renderer!} payload={payload} />
        )}
      </CardContent>
    </Card>
  );
}

function isEmptyServerFleet(contribution: UiContribution, payload: unknown): boolean {
  if (
    contribution.owner_bundle !== "server-administrator"
    || contribution.gadget_name !== "server.fleet-summary"
    || payload === null
    || typeof payload !== "object"
    || Array.isArray(payload)
  ) return false;
  const summary = (payload as Record<string, unknown>).summary;
  return summary !== null
    && typeof summary === "object"
    && !Array.isArray(summary)
    && (summary as Record<string, unknown>).servers === 0;
}

function humanLabel(value: string): string {
  return value
    .replaceAll("_", " ")
    .replace(/\b\w/g, (letter) => letter.toUpperCase());
}

function ActivityRow({ event }: { event: LiveEvent }) {
  const title = humanLabel(String(event.type || "Activity"));
  const target = firstScalar(event, ["target", "target_id", "subject", "resource"]);
  const status = firstScalar(event, ["status", "state", "result"]);
  const action = firstScalar(event, ["action", "operation", "gadget_name"]);
  const timestamp = firstScalar(event, ["created_at", "timestamp", "at", "time"]);
  const summary = [action, target, status].filter(Boolean).join(" · ");
  return (
    <div className="px-3 py-3" data-testid="dashboard-live-event">
      <div className="flex items-start gap-2">
        <Activity className="mt-0.5 size-3.5 shrink-0 text-zinc-600" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium text-zinc-300">{title}</div>
          {summary && <div className="mt-1 truncate font-mono text-[10px] text-zinc-600">{summary}</div>}
        </div>
        {timestamp && (
          <span className="flex shrink-0 items-center gap-1 font-mono text-[9px] text-zinc-700" title={timestamp}>
            <Clock3 className="size-3" aria-hidden />
            {activityTime(timestamp)}
          </span>
        )}
      </div>
    </div>
  );
}

function firstScalar(event: LiveEvent, keys: string[]): string {
  for (const key of keys) {
    const value = event[key];
    if (typeof value === "string" || typeof value === "number") {
      return boundedText(String(value), "");
    }
  }
  return "";
}

function boundedText(value: string, fallback: string): string {
  const clean = value.replace(/[\u0000-\u001f\u007f]/g, " ").trim();
  return clean ? clean.slice(0, 160) : fallback;
}

function activityTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "now";
  return new Intl.DateTimeFormat("ko-KR", {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(date);
}
