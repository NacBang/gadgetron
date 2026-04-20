"use client";

import { PanelRight, X } from "lucide-react";
import { useEvidence, type EvidenceItem } from "../../lib/evidence-context";

// ---------------------------------------------------------------------------
// EvidencePane
//
// Live feed of read-tier tool calls Penny makes + knowledge workbench
// actions the user invokes. Driven by `EvidenceProvider` which owns one
// shared WebSocket subscription to `/workbench/events/ws`. Empty state
// when nothing has been cited yet in the current session.
// ---------------------------------------------------------------------------

interface EvidencePaneProps {
  open: boolean;
  onToggle: (open: boolean) => void;
  width?: number;
}

function formatRelative(at: number, now: number): string {
  const s = Math.max(0, Math.floor((now - at) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h`;
}

function renderArgsPreview(item: EvidenceItem): string | null {
  const parsed = item.argumentsParsed;
  if (parsed) {
    // Prefer human-friendly keys when they exist.
    if (typeof parsed.name === "string") return String(parsed.name);
    if (typeof parsed.query === "string") return `"${String(parsed.query)}"`;
    if (typeof parsed.path === "string") return String(parsed.path);
    // Fall through to raw JSON summary if no preferred key matched.
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

export function EvidencePane({ open, onToggle, width = 320 }: EvidencePaneProps) {
  const { items, wsStatus, clear } = useEvidence();

  if (!open) {
    return (
      <div
        className="flex w-8 shrink-0 flex-col items-center border-l border-zinc-800 bg-zinc-950 pt-3"
        data-testid="evidence-pane-collapsed"
      >
        <button
          type="button"
          aria-label="Open evidence pane"
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
        {items.length > 0 && (
          <span
            data-testid="evidence-pane-badge"
            className="mt-2 rounded bg-emerald-900/40 px-1 text-[9px] font-mono text-emerald-400"
          >
            {items.length}
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
      aria-label="Evidence pane"
    >
      <div className="flex h-9 shrink-0 items-center justify-between border-b border-zinc-800 px-3">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-zinc-400">Evidence</span>
          <span
            data-testid="evidence-ws-status"
            className={`rounded border px-1 py-px font-mono text-[9px] ${
              wsStatus === "open"
                ? "border-emerald-700/40 bg-emerald-900/20 text-emerald-400"
                : wsStatus === "connecting"
                  ? "border-amber-700/40 bg-amber-900/20 text-amber-400"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500"
            }`}
          >
            {wsStatus}
          </span>
          {items.length > 0 && (
            <span className="font-mono text-[10px] text-zinc-600">
              {items.length}
            </span>
          )}
        </div>
        <div className="flex items-center gap-1">
          {items.length > 0 && (
            <button
              type="button"
              aria-label="Clear evidence"
              data-testid="evidence-pane-clear-btn"
              onClick={clear}
              className="flex size-6 items-center justify-center rounded text-zinc-600 hover:bg-zinc-800 hover:text-zinc-300"
            >
              <X className="size-3" aria-hidden />
            </button>
          )}
          <button
            type="button"
            aria-label="Collapse evidence pane"
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

      {items.length === 0 ? (
        <div
          className="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center"
          data-testid="evidence-pane-empty"
        >
          <div className="size-8 rounded border border-zinc-800 bg-zinc-900 flex items-center justify-center">
            <span className="font-mono text-[10px] text-zinc-600" aria-hidden>
              §
            </span>
          </div>
          <div className="flex flex-col gap-1">
            <p className="text-xs font-medium text-zinc-400">No evidence yet</p>
            <p
              className="text-[11px] leading-relaxed text-zinc-600"
              data-testid="evidence-empty-copy"
            >
              Read-tier tool calls (wiki.list / wiki.search / wiki.get / web.search)
              and knowledge workbench actions appear here in real time.
            </p>
          </div>
        </div>
      ) : (
        <ol
          data-testid="evidence-list"
          className="flex-1 overflow-y-auto"
          aria-label="Evidence feed"
        >
          {items.map((item) => (
            <EvidenceRow key={item.id} item={item} />
          ))}
        </ol>
      )}
    </aside>
  );
}
