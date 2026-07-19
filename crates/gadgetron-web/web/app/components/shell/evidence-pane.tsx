"use client";

import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  BookOpen,
  MessageSquareText,
  PanelRight,
  X,
} from "lucide-react";

import { useEvidence, type EvidenceItem } from "../../lib/evidence-context";
import { useInspector } from "../../lib/inspector-context";
import { useI18n } from "../../lib/i18n";
import { ContextTab } from "./side-panel-context";

interface EvidencePaneProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  width?: number;
}

type TabId = "context" | "sources" | "activity";
type InspectorMode = "screen" | "ai";

function isKnowledgeKind(name: string): boolean {
  return (
    name.startsWith("wiki.") ||
    name.startsWith("wiki-") ||
    name === "web.search" ||
    name === "web-search" ||
    name === "knowledge-search"
  );
}

function formatRelative(at: number, now: number): string {
  const seconds = Math.max(0, Math.floor((now - at) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  return `${Math.floor(minutes / 60)}h`;
}

function humanizeName(name: string, fallback: string): string {
  const normalized = name
    .replace(/^wiki[.-]/, "knowledge ")
    .replace(/^web[.-]/, "web ")
    .replace(/^knowledge[.-]/, "knowledge ")
    .split(/[._-]+/)
    .filter(Boolean)
    .join(" ");
  if (!normalized) return fallback;
  return `${normalized[0].toUpperCase()}${normalized.slice(1)}`;
}

function renderArgsPreview(item: EvidenceItem): string | null {
  const parsed = item.argumentsParsed;
  if (parsed) {
    if (typeof parsed.name === "string") return String(parsed.name);
    if (typeof parsed.query === "string") return `“${String(parsed.query)}”`;
    if (typeof parsed.path === "string") return String(parsed.path);
  }
  return item.argumentsSummary ?? null;
}

function ActivityRow({ item, sourceMode = false }: { item: EvidenceItem; sourceMode?: boolean }) {
  const { labels } = useI18n();
  const ok = item.outcome === "success" || item.outcome === "ok";
  const argsPreview = renderArgsPreview(item);
  const body = (
    <>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className={`truncate text-xs font-medium ${ok ? "text-zinc-200" : "text-amber-300"}`}>
            {humanizeName(item.name, labels.evidence.activity)}
          </div>
          <div className="mt-0.5 truncate font-mono text-[10px] text-zinc-600" title={item.name}>
            {item.name}
          </div>
        </div>
        <time className="shrink-0 font-mono text-[10px] text-zinc-600">
          {formatRelative(item.at, Date.now())}
        </time>
      </div>
      {argsPreview && (
        <div
          className="mt-1.5 line-clamp-2 break-all text-xs leading-4 text-zinc-400"
          data-testid="evidence-args"
          title={item.argumentsSummary ?? argsPreview}
        >
          {argsPreview}
        </div>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[10px] text-zinc-600">
        <span className="rounded border border-zinc-800 px-1.5 py-0.5">
          {sourceMode ? labels.evidence.sourceRead : item.kind}
        </span>
        {item.tier && <span>{item.tier}</span>}
        {typeof item.elapsedMs === "number" && <span className="font-mono">{item.elapsedMs}ms</span>}
        {!ok && <span className="text-amber-400">{item.outcome}</span>}
      </div>
    </>
  );

  const className = "block border-b border-zinc-900 px-3 py-3 text-left hover:bg-zinc-900/50";
  return (
    <li data-testid="evidence-item" data-kind={item.kind} data-outcome={item.outcome}>
      {item.href ? (
        <a
          href={item.href}
          target="_blank"
          rel="noopener noreferrer"
          className={className}
          data-testid="evidence-link"
        >
          {body}
        </a>
      ) : (
        <div className={className}>{body}</div>
      )}
    </li>
  );
}

function SourcesTab({ items }: { items: EvidenceItem[] }) {
  const { labels } = useI18n();
  const sources = useMemo(() => items.filter((item) => isKnowledgeKind(item.name)), [items]);
  if (sources.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center" data-testid="sources-empty">
        <BookOpen className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-300">{labels.evidence.noSourceActivity}</p>
        <p className="text-xs leading-5 text-zinc-500">
          {labels.evidence.noSourceActivityDescription}
        </p>
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b border-zinc-900 px-3 py-2 text-xs leading-5 text-zinc-500">
        {labels.evidence.sourceActivityNotice}
      </div>
      <ol className="flex-1 overflow-y-auto" aria-label={labels.evidence.sourceActivity} data-testid="sources-list">
        {sources.map((item) => <ActivityRow key={item.id} item={item} sourceMode />)}
      </ol>
    </div>
  );
}

function ActivityTab({ items }: { items: EvidenceItem[] }) {
  const { labels } = useI18n();
  if (items.length === 0) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center" data-testid="activity-empty">
        <Activity className="size-4 text-zinc-700" aria-hidden />
        <p className="text-xs font-medium text-zinc-300">{labels.evidence.noLiveActivity}</p>
        <p className="text-xs leading-5 text-zinc-500">
          {labels.evidence.noLiveActivityDescription}
        </p>
      </div>
    );
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="border-b border-zinc-900 px-3 py-2 text-xs leading-5 text-zinc-500">
        {labels.evidence.liveActivityNotice}
      </div>
      <ol className="flex-1 overflow-y-auto" aria-label={labels.evidence.liveActivity} data-testid="evidence-list">
        {items.map((item) => <ActivityRow key={item.id} item={item} />)}
      </ol>
    </div>
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
  count?: number;
  icon: ReactNode;
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      aria-label={label}
      onClick={onClick}
      className={`flex h-8 min-w-0 flex-1 items-center justify-center gap-1.5 rounded px-2 text-xs transition-colors ${
        active
          ? "bg-zinc-800 text-zinc-100"
          : "text-zinc-500 hover:bg-zinc-900 hover:text-zinc-300"
      }`}
    >
      {icon}
      <span className="truncate">{label}</span>
      {count !== undefined && count > 0 && (
        <span className="rounded bg-zinc-700 px-1 font-mono text-xs text-zinc-200">
          {count > 99 ? "99+" : count}
        </span>
      )}
    </button>
  );
}

export function EvidencePane({ open, onToggle, width = 320 }: EvidencePaneProps) {
  const { items, wsStatus, clear } = useEvidence();
  const { view } = useInspector();
  const { labels } = useI18n();
  const [tab, setTab] = useState<TabId>("context");
  const [mode, setMode] = useState<InspectorMode>(view ? "screen" : "ai");
  const [seenItemIds, setSeenItemIds] = useState<Set<string>>(() => new Set());
  const sourceCount = useMemo(() => items.filter((item) => isKnowledgeKind(item.name)).length, [items]);
  const unreadCount = useMemo(
    () => (open ? 0 : items.filter((item) => !seenItemIds.has(item.id)).length),
    [items, open, seenItemIds],
  );

  useEffect(() => {
    if (open) {
      setSeenItemIds(new Set(items.map((item) => item.id)));
    }
  }, [items, open]);

  useEffect(() => {
    setMode(view ? "screen" : "ai");
  }, [view?.id]);

  if (!open) {
    const inspectorTitle = view?.title ?? labels.evidence.inspector;
    const unreadActivity = labels.evidence.newActivityItems(unreadCount);
    const unreadLabel = unreadCount > 0 ? `, ${unreadActivity}` : "";
    return (
      <div
        className="flex w-8 shrink-0 flex-col items-center border-l border-zinc-800 bg-zinc-950 pt-3"
        data-testid="evidence-pane-collapsed"
      >
        <button
          type="button"
          aria-label={`${labels.evidence.openInspector(inspectorTitle)}${unreadLabel}`}
          title={unreadCount > 0 ? unreadActivity : labels.evidence.openInspector(inspectorTitle)}
          data-testid="evidence-pane-expand-btn"
          onClick={() => onToggle(true)}
          className="relative flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
        >
          <PanelRight className="size-3.5" aria-hidden />
          {unreadCount > 0 && (
            <span
              data-testid="evidence-pane-activity-badge"
              className="absolute -right-2 -top-2 min-w-4 rounded-full bg-sky-500 px-1 text-center font-mono text-[10px] leading-4 text-slate-950"
            >
              {unreadCount > 99 ? "99+" : unreadCount}
            </span>
          )}
        </button>
      </div>
    );
  }

  return (
    <aside
      id="evidence-pane"
      data-testid="evidence-pane"
      className="flex h-full min-h-0 shrink-0 flex-col border-l border-zinc-800 bg-zinc-950"
      style={{ width }}
      aria-label={labels.evidence.inspector}
    >
      <div className="flex h-9 shrink-0 items-center border-b border-zinc-800 px-3">
        <span className="truncate text-xs font-semibold tracking-wide text-zinc-200">{view?.title ?? labels.evidence.inspector}</span>
        <div className="ml-auto flex items-center gap-2">
          {mode === "ai" && items.length > 0 && <span
            data-testid="evidence-ws-status"
            className={`inline-flex items-center gap-1 text-xs ${
              wsStatus === "open" ? "text-emerald-500" : wsStatus === "connecting" ? "text-amber-500" : "text-zinc-600"
            }`}
            title={labels.evidence.liveStream(wsStatus)}
          >
            <span className="size-1.5 rounded-full bg-current" aria-hidden />
            {wsStatus === "open" ? labels.evidence.live : wsStatus === "connecting" ? labels.evidence.connecting : labels.evidence.offline}
          </span>}
          {mode === "ai" && tab === "activity" && items.length > 0 && (
            <button
              type="button"
              aria-label={labels.evidence.clearActivity}
              title={labels.evidence.clearActivity}
              data-testid="evidence-pane-clear-btn"
              onClick={clear}
              className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
            >
              <X className="size-3" aria-hidden />
            </button>
          )}
          <button
            type="button"
            aria-label={labels.evidence.collapseInspector}
            title={labels.evidence.collapseInspector}
            data-testid="evidence-pane-collapse-btn"
            onClick={() => onToggle(false)}
            className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
          >
            <PanelRight className="size-3.5" aria-hidden />
          </button>
        </div>
      </div>

      {view && items.length > 0 && (
        <div className="flex shrink-0 gap-1 border-b border-zinc-800 p-1" role="tablist" aria-label={labels.evidence.inspectorModes}>
          <TabButton
            active={mode === "screen"}
            onClick={() => setMode("screen")}
            label={labels.evidence.preview}
            icon={<PanelRight className="size-3.5" aria-hidden />}
          />
          <TabButton
            active={mode === "ai"}
            onClick={() => setMode("ai")}
            label={labels.evidence.aiActivity}
            count={items.length}
            icon={<Activity className="size-3.5" aria-hidden />}
          />
        </div>
      )}

      {mode === "screen" && view?.content}
      {mode === "ai" && items.length > 0 && (
        <>
          <div className="flex shrink-0 gap-1 border-b border-zinc-800 p-1" role="tablist" aria-label={labels.evidence.aiInspectorViews}>
            <TabButton
              active={tab === "context"}
              onClick={() => setTab("context")}
              label={labels.evidence.aiContext}
              icon={<MessageSquareText className="size-3.5" aria-hidden />}
            />
            <TabButton
              active={tab === "sources"}
              onClick={() => setTab("sources")}
              label={labels.evidence.evidenceMaterials}
              count={sourceCount}
              icon={<BookOpen className="size-3.5" aria-hidden />}
            />
            <TabButton
              active={tab === "activity"}
              onClick={() => setTab("activity")}
              label={labels.evidence.activityHistory}
              count={items.length}
              icon={<Activity className="size-3.5" aria-hidden />}
            />
          </div>
          {tab === "context" && <ContextTab />}
          {tab === "sources" && <SourcesTab items={items} />}
          {tab === "activity" && <ActivityTab items={items} />}
        </>
      )}
      {mode === "ai" && items.length === 0 && (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center" data-testid="inspector-empty">
          <MessageSquareText className="size-4 text-zinc-700" aria-hidden />
          <p className="text-xs font-medium text-zinc-300">{labels.evidence.emptyTitle}</p>
          <p className="text-xs leading-5 text-zinc-500">{labels.evidence.emptyDescription}</p>
        </div>
      )}
      <span className="hidden" data-testid="side-panel-total-badge">{sourceCount}</span>
    </aside>
  );
}
