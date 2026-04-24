"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { PanelRight, X, Zap, BookOpen, Activity } from "lucide-react";
import { useEvidence, type EvidenceItem } from "../../lib/evidence-context";
import { useAuth } from "../../lib/auth-context";

// ---------------------------------------------------------------------------
// Side panel (ex-Evidence)
//
// Three tabs:
//   Actions  (default) — pending operator decisions. Today sourced
//                         from `loganalysis-list`: every finding with
//                         a `remediation` field surfaces as a one-click
//                         card. Future home of Penny's inline action
//                         proposals once the Ask approval flow (Task
//                         #52) lands.
//   Sources           — wiki / web calls consumed in the current
//                         conversation. Filtered from the Evidence WS
//                         feed; lets the operator audit Penny's
//                         citations without scrolling chat history.
//   Activity          — full raw tool/action log (previous default).
//                         Useful as backstage debugging, not primary.
// ---------------------------------------------------------------------------

interface EvidencePaneProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  width?: number;
}

type TabId = "actions" | "sources" | "activity";

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

function formatRelative(at: number, now: number): string {
  const s = Math.max(0, Math.floor((now - at) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h`;
}

function isKnowledgeKind(name: string): boolean {
  return (
    name.startsWith("wiki.") ||
    name === "web.search" ||
    name === "knowledge-search" ||
    name === "wiki-read" ||
    name === "wiki-list" ||
    name === "wiki-search"
  );
}

// ---------------------------------------------------------------------------
// Actions tab — pending remediations
// ---------------------------------------------------------------------------

interface PendingAction {
  findingId: string;
  hostId: string;
  severity: "critical" | "high" | "medium" | "info";
  category: string;
  summary: string;
  remediation: {
    tool: string;
    args: Record<string, unknown>;
    label?: string;
  };
}

function useActionsFeed(apiKey: string | null): PendingAction[] {
  const [actions, setActions] = useState<PendingAction[]>([]);
  useEffect(() => {
    let cancel = false;
    let timer: ReturnType<typeof setInterval> | null = null;
    const fetchOnce = async () => {
      try {
        const res = await fetch(
          `${getApiBase()}/workbench/actions/loganalysis-list`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: JSON.stringify({ args: {} }),
          },
        );
        if (!res.ok) return;
        const body = await res.json();
        const payload = body?.result?.payload;
        const raw =
          Array.isArray(payload) && payload[0]?.text
            ? JSON.parse(payload[0].text)
            : null;
        const findings: Array<Record<string, unknown>> = raw?.findings ?? [];
        const next: PendingAction[] = [];
        for (const f of findings) {
          const rem = f.remediation as PendingAction["remediation"] | null;
          if (!rem || typeof rem !== "object" || !rem.tool) continue;
          next.push({
            findingId: String(f.id),
            hostId: String(f.host_id),
            severity: (f.severity as PendingAction["severity"]) ?? "info",
            category: String(f.category ?? ""),
            summary: String(f.summary ?? ""),
            remediation: rem,
          });
        }
        if (!cancel) setActions(next);
      } catch {
        // Ignore — keep whatever we had on transient fail.
      }
    };
    void fetchOnce();
    timer = setInterval(fetchOnce, 15_000);
    return () => {
      cancel = true;
      if (timer) clearInterval(timer);
    };
  }, [apiKey]);
  return actions;
}

function severityTint(s: PendingAction["severity"]): string {
  switch (s) {
    case "critical":
      return "border-red-800 bg-red-950/30 text-red-200";
    case "high":
      return "border-amber-800 bg-amber-950/30 text-amber-200";
    case "medium":
      return "border-yellow-800 bg-yellow-950/30 text-yellow-200";
    default:
      return "border-zinc-800 bg-zinc-900/40 text-zinc-300";
  }
}

function ActionsTab({ apiKey }: { apiKey: string | null }) {
  const actions = useActionsFeed(apiKey);
  const run = useCallback(
    async (a: PendingAction) => {
      const label =
        a.remediation.label ??
        `${a.remediation.tool} ${JSON.stringify(a.remediation.args)}`;
      if (!window.confirm(`실행할까요?\n\n${label}`)) return;
      try {
        const actionId = a.remediation.tool.replace(".", "-");
        const args = { ...a.remediation.args, id: a.hostId };
        await fetch(`${getApiBase()}/workbench/actions/${actionId}`, {
          method: "POST",
          credentials: "include",
          headers: {
            ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
            "Content-Type": "application/json",
          },
          body: JSON.stringify({ args }),
        });
        // Auto-dismiss the finding so it stops surfacing.
        await fetch(
          `${getApiBase()}/workbench/actions/loganalysis-dismiss`,
          {
            method: "POST",
            credentials: "include",
            headers: {
              ...(apiKey ? { Authorization: `Bearer ${apiKey}` } : {}),
              "Content-Type": "application/json",
            },
            body: JSON.stringify({ args: { id: a.findingId } }),
          },
        );
      } catch (e) {
        alert(`실행 실패: ${(e as Error).message}`);
      }
    },
    [apiKey],
  );

  if (actions.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="actions-empty"
      >
        <Zap className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">대기 중 조치 없음</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Penny가 실행 가능한 조치를 제안하면 여기 쌓입니다.
          <br />
          Logs 탭에서 finding의 ⚡ 버튼을 누르면 같은 경로로 실행돼요.
        </p>
      </div>
    );
  }
  return (
    <ol className="flex-1 overflow-y-auto" data-testid="actions-list">
      {actions.map((a) => (
        <li
          key={a.findingId}
          className={`border-b border-zinc-900 px-3 py-2 text-[11px] ${severityTint(a.severity)}`}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="truncate font-mono">{a.category}</span>
            <span className="shrink-0 rounded bg-black/20 px-1 text-[9px] uppercase">
              {a.severity}
            </span>
          </div>
          <div className="mt-0.5 truncate text-[11px] text-zinc-100">
            {a.summary}
          </div>
          <div className="mt-1 flex items-center gap-2">
            <code className="truncate rounded bg-black/30 px-1.5 py-0.5 font-mono text-[10px] text-zinc-300">
              {a.remediation.tool}{" "}
              {Object.entries(a.remediation.args)
                .map(([k, v]) => `${k}=${JSON.stringify(v)}`)
                .join(" ")}
            </code>
            <button
              type="button"
              onClick={() => void run(a)}
              className="ml-auto shrink-0 rounded border border-blue-700 bg-blue-950/40 px-2 py-0.5 font-mono text-[10px] font-semibold text-blue-200 hover:border-blue-500 hover:bg-blue-900/60"
            >
              ⚡ {a.remediation.label ?? "실행"}
            </button>
          </div>
        </li>
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Sources tab — current-conversation citations
// ---------------------------------------------------------------------------

function SourcesTab({ items }: { items: EvidenceItem[] }) {
  const filtered = useMemo(
    () => items.filter((i) => isKnowledgeKind(i.name)),
    [items],
  );
  if (filtered.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="sources-empty"
      >
        <BookOpen className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">인용 없음</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Penny가 wiki 페이지나 웹을 조회하면 출처가 여기에 나타납니다.
        </p>
      </div>
    );
  }
  return (
    <ol
      className="flex-1 overflow-y-auto"
      aria-label="Sources feed"
      data-testid="sources-list"
    >
      {filtered.map((item) => (
        <EvidenceRow key={item.id} item={item} />
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Activity tab — full raw evidence log (old default)
// ---------------------------------------------------------------------------

function ActivityTab({ items }: { items: EvidenceItem[] }) {
  if (items.length === 0) {
    return (
      <div
        className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center"
        data-testid="activity-empty"
      >
        <Activity className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-400">아직 활동 없음</p>
        <p className="text-[11px] leading-relaxed text-zinc-600">
          Penny의 read-tier 호출 + workbench action이 실시간으로 기록됩니다.
        </p>
      </div>
    );
  }
  return (
    <ol
      className="flex-1 overflow-y-auto"
      aria-label="Activity feed"
      data-testid="evidence-list"
    >
      {items.map((item) => (
        <EvidenceRow key={item.id} item={item} />
      ))}
    </ol>
  );
}

// ---------------------------------------------------------------------------
// Shared row
// ---------------------------------------------------------------------------

function renderArgsPreview(item: EvidenceItem): string | null {
  const parsed = item.argumentsParsed;
  if (parsed) {
    if (typeof parsed.name === "string") return String(parsed.name);
    if (typeof parsed.query === "string") return `"${String(parsed.query)}"`;
    if (typeof parsed.path === "string") return String(parsed.path);
  }
  return item.argumentsSummary ?? null;
}

function EvidenceRow({ item }: { item: EvidenceItem }) {
  const now = Date.now();
  const ok = item.outcome === "success" || item.outcome === "ok";
  const argsPreview = renderArgsPreview(item);
  const inner = (
    <>
      <div className="flex items-center justify-between gap-2">
        <span
          className={`truncate font-mono ${ok ? "text-zinc-300" : "text-amber-400"}`}
          title={item.name}
        >
          {item.name}
        </span>
        <span className="shrink-0 font-mono text-[10px] text-zinc-600">
          {formatRelative(item.at, now)}
        </span>
      </div>
      {argsPreview && (
        <div
          className="mt-0.5 truncate font-mono text-[10px] text-zinc-400"
          data-testid="evidence-args"
          title={item.argumentsSummary ?? argsPreview}
        >
          {argsPreview}
        </div>
      )}
      <div className="mt-0.5 flex items-center gap-2 text-[10px] text-zinc-600">
        <span className="font-mono">{item.kind}</span>
        {item.tier && (
          <span className="rounded bg-zinc-900 px-1 text-zinc-500">{item.tier}</span>
        )}
        {typeof item.elapsedMs === "number" && (
          <span className="font-mono">{item.elapsedMs}ms</span>
        )}
        {!ok && (
          <span className="rounded bg-red-950/50 px-1 text-red-400">
            {item.outcome}
          </span>
        )}
      </div>
    </>
  );
  const common =
    "block border-b border-zinc-900 px-3 py-2 text-[11px] hover:bg-zinc-900/40";
  if (item.href) {
    return (
      <li data-testid="evidence-item" data-kind={item.kind} data-outcome={item.outcome}>
        <a
          href={item.href}
          target="_blank"
          rel="noopener noreferrer"
          className={`${common} cursor-pointer no-underline`}
          data-testid="evidence-link"
        >
          {inner}
        </a>
      </li>
    );
  }
  return (
    <li
      data-testid="evidence-item"
      data-kind={item.kind}
      data-outcome={item.outcome}
      className={common}
    >
      {inner}
    </li>
  );
}

// ---------------------------------------------------------------------------
// Main pane
// ---------------------------------------------------------------------------

export function EvidencePane({ open, onToggle, width = 320 }: EvidencePaneProps) {
  const { items, wsStatus, clear } = useEvidence();
  const { apiKey } = useAuth();
  const [tab, setTab] = useState<TabId>("actions");

  const actionsBadge = useActionsFeed(apiKey);
  const sourcesBadge = useMemo(
    () => items.filter((i) => isKnowledgeKind(i.name)).length,
    [items],
  );
  const totalBadge = actionsBadge.length + sourcesBadge;

  if (!open) {
    return (
      <div
        className="flex w-8 shrink-0 flex-col items-center gap-2 border-l border-zinc-800 bg-zinc-950 pt-3"
        data-testid="evidence-pane-collapsed"
      >
        <button
          type="button"
          aria-label="Open side panel"
          data-testid="evidence-pane-expand-btn"
          onClick={() => {
            if (typeof window !== "undefined") {
              localStorage.setItem(
                "gadgetron.workbench.evidencePaneOpen",
                "true",
              );
            }
            onToggle(true);
          }}
          className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
        >
          <PanelRight className="size-3.5" aria-hidden />
        </button>
        {actionsBadge.length > 0 && (
          <span
            data-testid="evidence-pane-badge"
            className="rounded bg-blue-900/50 px-1 font-mono text-[9px] text-blue-300"
            title={`${actionsBadge.length} 대기 조치`}
          >
            ⚡{actionsBadge.length}
          </span>
        )}
        {sourcesBadge > 0 && actionsBadge.length === 0 && (
          <span
            data-testid="evidence-pane-badge-sources"
            className="rounded bg-zinc-800 px-1 font-mono text-[9px] text-zinc-400"
            title={`${sourcesBadge} 인용`}
          >
            {sourcesBadge}
          </span>
        )}
      </div>
    );
  }

  return (
    <aside
      data-testid="evidence-pane"
      className="flex shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
      style={{ width }}
      aria-label="Side panel"
    >
      {/* Tab row + controls */}
      <div className="flex h-9 shrink-0 items-center border-b border-zinc-800 px-1">
        <TabButton
          active={tab === "actions"}
          onClick={() => setTab("actions")}
          label="Actions"
          count={actionsBadge.length}
          icon={<Zap className="size-3" aria-hidden />}
        />
        <TabButton
          active={tab === "sources"}
          onClick={() => setTab("sources")}
          label="Sources"
          count={sourcesBadge}
          icon={<BookOpen className="size-3" aria-hidden />}
        />
        <TabButton
          active={tab === "activity"}
          onClick={() => setTab("activity")}
          label="Activity"
          count={items.length}
          icon={<Activity className="size-3" aria-hidden />}
        />
        <div className="ml-auto flex items-center gap-1 pr-1">
          <span
            data-testid="evidence-ws-status"
            className={`rounded border px-1 py-px font-mono text-[9px] ${
              wsStatus === "open"
                ? "border-emerald-700/40 bg-emerald-900/20 text-emerald-400"
                : wsStatus === "connecting"
                  ? "border-amber-700/40 bg-amber-900/20 text-amber-400"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500"
            }`}
            title={`WebSocket ${wsStatus}`}
          >
            {wsStatus === "open" ? "●" : wsStatus === "connecting" ? "…" : "○"}
          </span>
          {tab === "activity" && items.length > 0 && (
            <button
              type="button"
              aria-label="Clear activity"
              data-testid="evidence-pane-clear-btn"
              onClick={clear}
              className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
              title="Clear activity log"
            >
              <X className="size-3" aria-hidden />
            </button>
          )}
          <button
            type="button"
            aria-label="Collapse side panel"
            data-testid="evidence-pane-collapse-btn"
            onClick={() => {
              if (typeof window !== "undefined") {
                localStorage.setItem(
                  "gadgetron.workbench.evidencePaneOpen",
                  "false",
                );
              }
              onToggle(false);
            }}
            className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
          >
            <PanelRight className="size-3.5" aria-hidden />
          </button>
        </div>
      </div>

      {/* Tab body */}
      {tab === "actions" && <ActionsTab apiKey={apiKey} />}
      {tab === "sources" && <SourcesTab items={items} />}
      {tab === "activity" && <ActivityTab items={items} />}
      {/* Hidden totalBadge marker for selectors/tests */}
      <span className="hidden" data-testid="side-panel-total-badge">
        {totalBadge}
      </span>
    </aside>
  );
}

function TabButton({
  active,
  onClick,
  label,
  count,
  icon,
}: {
  active: boolean;
  onClick: () => void;
  label: string;
  count: number;
  icon: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={`flex items-center gap-1 rounded px-2 py-1 text-[11px] font-medium transition-colors ${
        active
          ? "bg-zinc-800 text-zinc-100"
          : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300"
      }`}
    >
      {icon}
      <span>{label}</span>
      {count > 0 && (
        <span
          className={`ml-0.5 rounded px-1 font-mono text-[9px] ${
            active ? "bg-zinc-900 text-zinc-300" : "bg-zinc-900 text-zinc-500"
          }`}
        >
          {count}
        </span>
      )}
    </button>
  );
}
