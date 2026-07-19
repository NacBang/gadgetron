"use client";

import Link from "next/link";
import { useState, type ReactNode } from "react";
import { ChevronRight, MessageSquareText, X } from "lucide-react";

import { Card, CardContent } from "../ui/card";
import { Button } from "../ui/button";
import { EmptyState } from "./empty-state";
import { InlineNotice } from "./inline-notice";
import { MarkdownText } from "../markdown-text";
import { InteractiveGraphRenderer } from "./interactive-graph-renderer";
import { InteractiveTimeseriesRenderer } from "./interactive-timeseries-renderer";
import { TelemetryOverviewRenderer } from "./telemetry-overview-renderer";
import type { WorkspaceRenderer } from "../../lib/bundle-workspaces";
import { useI18n } from "../../lib/i18n";

type JsonRecord = Record<string, unknown>;
const MAX_ITEMS = 500;
const MAX_COLUMNS = 6;

export interface DeclarativeRowAction {
  label: string;
  onInvoke: (row: JsonRecord, index: number) => void;
  isAvailable?: (row: JsonRecord) => boolean;
  running?: boolean;
}

export interface DeclarativeEmptyState {
  title: string;
  description: string;
  action?: ReactNode;
}

export interface PlatformRendererState {
  selectedServerId: string;
  timeRange: string;
  selectServer: (targetId: string) => void;
  workspaceHref: (workspaceId: string) => string;
}

export function asRecord(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonRecord)
    : null;
}

export function collection(value: unknown): unknown[] {
  if (Array.isArray(value)) return value;
  const record = asRecord(value);
  if (!record) return [];
  for (const key of ["rows", "items", "entries", "records", "nodes", "events", "points"]) {
    if (Array.isArray(record[key])) return record[key] as unknown[];
  }
  return [];
}

export function scalarText(value: unknown): string {
  if (value === null || value === undefined) return "Not collected";
  if (typeof value === "string") return value.slice(0, 2_048);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return `List · ${value.length} items`;
  const record = asRecord(value);
  if (record) return `Object · ${Object.keys(record).length} fields`;
  return "Unsupported value";
}

function StructuredValue({ value, depth = 0 }: { value: unknown; depth?: number }) {
  const record = asRecord(value);
  if (record && depth < 4) {
    return (
      <dl className="grid gap-x-4 gap-y-2 sm:grid-cols-2">
        {Object.entries(record).slice(0, 100).map(([key, child]) => (
          <div key={key} className="min-w-0 border-l border-zinc-800 pl-2">
            <dt className="text-[9px] font-semibold uppercase tracking-wider text-zinc-600">{key}</dt>
            <dd className="mt-1 break-words text-[11px] text-zinc-300"><StructuredValue value={child} depth={depth + 1} /></dd>
          </div>
        ))}
      </dl>
    );
  }
  if (Array.isArray(value) && depth < 4) {
    if (value.length === 0) return <span className="text-zinc-600">No items collected</span>;
    return (
      <ol className="space-y-2">
        {value.slice(0, 100).map((item, index) => (
          <li key={index} className="rounded border border-zinc-800 bg-zinc-950/60 p-2">
            <StructuredValue value={item} depth={depth + 1} />
          </li>
        ))}
      </ol>
    );
  }
  return <span className="font-mono">{scalarText(value)}</span>;
}

function DetailRenderer({ payload }: { payload: unknown }) {
  const record = asRecord(payload);
  if (!record) return <Incompatible reason="Detail payload must be an object." />;
  return (
    <dl className="grid grid-cols-1 border-t border-zinc-800 md:grid-cols-2">
      {Object.entries(record).slice(0, 100).map(([key, value]) => (
        <div key={key} className="border-b border-zinc-800 px-3 py-2 md:border-r">
          <dt className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">{key}</dt>
          <dd className="mt-1 break-words text-xs text-zinc-300"><StructuredValue value={value} depth={1} /></dd>
        </div>
      ))}
    </dl>
  );
}

function rowsFrom(payload: unknown): JsonRecord[] {
  return collection(payload).slice(0, MAX_ITEMS).map((row) => asRecord(row) ?? { value: row });
}

function TableRenderer({ payload, rowAction, rowActions = [], emptyState }: { payload: unknown; rowAction?: DeclarativeRowAction; rowActions?: DeclarativeRowAction[]; emptyState?: DeclarativeEmptyState }) {
  const rows = rowsFrom(payload);
  const [inspected, setInspected] = useState<number | null>(null);
  if (rows.length === 0) return <EmptyState title={emptyState?.title ?? "No records"} description={emptyState?.description ?? "The Bundle returned an empty dataset."} action={emptyState?.action} />;
  const columns = Array.from(new Set(rows.slice(0, 100).flatMap(Object.keys)))
    .filter((column) => rows.some((row) => !Array.isArray(row[column]) && !asRecord(row[column])))
    .filter((column) => !isTechnicalColumn(column))
    .sort((left, right) => columnPriority(left) - columnPriority(right) || left.localeCompare(right))
    .slice(0, MAX_COLUMNS);
  return (
    <div className="max-w-full overflow-x-auto overscroll-x-contain" data-testid="record-table-scroll">
      <table className="w-full min-w-[760px] table-fixed border-collapse text-left text-xs">
        <colgroup>{columns.map((column) => <col key={column} />)}<col className="w-64" /></colgroup>
        <thead className="sticky top-0 z-10 bg-zinc-950"><tr>{columns.map((column) => <th key={column} className="border-b border-zinc-700 px-3 py-2 text-xs font-semibold uppercase tracking-wider text-zinc-400">{humanLabel(column)}</th>)}<th className="sticky right-0 border-b border-l border-zinc-700 bg-zinc-950 px-2 py-2 text-right text-xs font-semibold uppercase tracking-wider text-zinc-400">Actions</th></tr></thead>
        <tbody>{rows.map((row, index) => {
          const open = inspected === index;
          const availableActions = rowActions.filter((action) => action.isAvailable?.(row) ?? true);
          return [
            <tr key={`row-${index}`} className="border-b border-zinc-900 hover:bg-zinc-900/60">
              {columns.map((column) => <td key={column} className="max-w-80 truncate px-3 py-2 font-mono text-zinc-300" title={scalarText(row[column])}>{scalarText(row[column])}</td>)}
              <td className="sticky right-0 border-l border-zinc-800 bg-zinc-950 px-2 py-1">
                <div className="flex flex-wrap justify-end gap-1">
                  {rowAction && (
                    <Button
                      type="button"
                      size="sm"
                      variant="outline"
                      className="h-7 min-w-0 max-w-28 whitespace-nowrap px-2 text-xs"
                      aria-label={`${rowAction.label} for row ${index + 1}`}
                      onClick={() => rowAction.onInvoke(row, index)}
                    >
                      <MessageSquareText className="mr-1 size-3" aria-hidden />
                      {rowAction.label}
                    </Button>
                  )}
                  {availableActions.map((action) => (
                    <Button
                      key={action.label}
                      type="button"
                      size="sm"
                      variant="outline"
                      disabled={action.running}
                      className="h-7 min-w-0 max-w-40 truncate px-2 text-xs"
                      title={action.label}
                      onClick={() => action.onInvoke(row, index)}
                    >
                      {action.running ? "Running…" : action.label}
                    </Button>
                  ))}
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    className="h-7 min-w-0 px-2 text-xs"
                    aria-label={`Inspect row ${index + 1}`}
                    aria-expanded={open}
                    onClick={() => setInspected(open ? null : index)}
                  >
                    {open ? <X className="size-3" aria-hidden /> : <ChevronRight className="size-3" aria-hidden />}
                    <span className="ml-1">{open ? "Close" : "Inspect"}</span>
                  </Button>
                </div>
              </td>
            </tr>,
            open && <tr key={`detail-${index}`} className="border-b border-zinc-800 bg-zinc-950/70"><td colSpan={columns.length + 1} className="p-3"><DetailRenderer payload={row} /></td></tr>,
          ];
        })}</tbody>
      </table>
    </div>
  );
}

function isTechnicalColumn(column: string): boolean {
  return /(^id$|_id$|^dmi_|(^|_)(uuid|serial|digest|revision)(_|$))/.test(column);
}

function columnPriority(column: string): number {
  if (/(^|_)(status|severity)$/.test(column)) return 0;
  if (/(^|_)(name|title|label|hostname|summary)$/.test(column)) return 1;
  if (/(cpu|memory|disk|gpu|temperature|power|alert|failure)/.test(column)) return 2;
  if (/(observed|fetched|updated|created|attempt|success).*_at$/.test(column)) return 3;
  if (/(^id$|_id$|revision|serial|uuid)/.test(column)) return 5;
  return 4;
}

function ListRenderer({ payload }: { payload: unknown }) {
  const items = collection(payload).slice(0, MAX_ITEMS);
  if (items.length === 0) return asRecord(payload) ? <DetailRenderer payload={payload} /> : <EmptyState title="No items" description="The Bundle returned no list items." />;
  return <div className="divide-y divide-zinc-800">{items.map((item, index) => <div key={index} className="px-3 py-2 text-xs text-zinc-300">{asRecord(item) ? <DetailRenderer payload={item} /> : scalarText(item)}</div>)}</div>;
}

function humanLabel(value: string): string {
  const acronyms: Record<string, string> = { cpu: "CPU", dcgm: "DCGM", dmi: "DMI", gpu: "GPU", id: "ID", ip: "IP", ram: "RAM", ssh: "SSH", uuid: "UUID" };
  return value.replaceAll(/[_-]+/g, " ").split(" ").map((word) =>
    acronyms[word.toLowerCase()] ?? word.charAt(0).toUpperCase() + word.slice(1),
  ).join(" ");
}

function cardFields(row: JsonRecord): Array<[string, unknown]> {
  const preferred = ["server", "cluster", "signals", "impact", "next_action", "started_at", "last_observed_at", "environment", "purpose", "servers", "active_servers", "needs_attention", "quarantined", "enrolling", "compliance_drift", "qualification_backlog", "why_it_matters", "confidence_basis", "open_questions", "audience", "target_account", "impact_preview", "risk_notes", "affected_plan", "place", "booking_impact", "cost_change_minor", "address", "origin", "cuisine", "query", "conditions", "last_seen_at", "window_start", "window_end", "start_date", "end_date", "valid_at", "observed_at"];
  return preferred
    .filter((key) => row[key] !== undefined && row[key] !== null && row[key] !== "")
    .slice(0, row.signals === undefined ? 4 : 5)
    .map((key) => [key, row[key]]);
}

function evidencePreviews(row: JsonRecord): JsonRecord[] {
  if (!Array.isArray(row.evidence_preview)) return [];
  return row.evidence_preview.slice(0, 10).map(asRecord).filter((item): item is JsonRecord => item !== null);
}

function evidenceReference(item: JsonRecord, index: number): string {
  return typeof item.reference === "string" && /^log-evidence-[0-9a-f]{12}$/.test(item.reference)
    ? item.reference
    : `log-evidence-${index + 1}`;
}

function evidenceAnchor(reference: string): string {
  return `incident-${reference.replaceAll(/[^a-zA-Z0-9_-]/g, "-")}`;
}

function EvidencePreview({ item, anchorId }: { item: JsonRecord; anchorId?: string }) {
  const metadata = [
    typeof item.source === "string" ? humanLabel(item.source) : null,
    typeof item.category === "string" ? humanLabel(item.category) : null,
    typeof item.occurrences === "number" ? `${item.occurrences} occurrence${item.occurrences === 1 ? "" : "s"}` : null,
    typeof item.last_observed_at === "string" ? item.last_observed_at : null,
    typeof item.classifier === "string" ? `Classified by ${humanLabel(item.classifier)}` : null,
  ].filter((value): value is string => value !== null);
  return (
    <article id={anchorId} className="scroll-mt-24 border border-zinc-800 bg-[#101418] p-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="text-xs font-medium text-zinc-200">{scalarText(item.summary ?? item.title ?? item.kind ?? "Observed evidence")}</div>
        {typeof item.severity === "string" && <span className="border border-zinc-700 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">{humanLabel(item.severity)}</span>}
      </div>
      {metadata.length > 0 && <div className="mt-1 flex flex-wrap gap-x-2 gap-y-1 font-mono text-[10px] text-zinc-500">{metadata.map((value) => <span key={value}>{value}</span>)}</div>}
      {typeof item.excerpt === "string" && item.excerpt !== "" && <pre className="mt-2 max-h-28 overflow-auto whitespace-pre-wrap break-all border-l border-[#B87333] pl-3 font-mono text-[11px] leading-5 text-zinc-300">{item.excerpt}</pre>}
      {(typeof item.cause === "string" && item.cause !== "" || typeof item.solution === "string" && item.solution !== "") && (
        <dl className="mt-2 grid gap-2 border-t border-zinc-800 pt-2 sm:grid-cols-2">
          {typeof item.cause === "string" && item.cause !== "" && <div><dt className="text-[10px] uppercase tracking-wider text-zinc-600">Likely cause</dt><dd className="mt-1 text-xs leading-5 text-zinc-300">{item.cause}</dd></div>}
          {typeof item.solution === "string" && item.solution !== "" && <div><dt className="text-[10px] uppercase tracking-wider text-zinc-600">Suggested response</dt><dd className="mt-1 text-xs leading-5 text-zinc-300">{item.solution}</dd></div>}
        </dl>
      )}
    </article>
  );
}

function CardEvidence({ row }: { row: JsonRecord }) {
  const evidence = evidencePreviews(row);
  if (evidence.length === 0) return null;
  const declaredTotal = typeof row.evidence_total === "number" ? row.evidence_total : evidence.length;
  const total = Math.max(declaredTotal, evidence.length);
  return (
    <section className="space-y-2 border-t border-zinc-800 pt-2" data-testid="card-evidence-preview">
      <div className="flex items-center justify-between gap-3">
        <h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">Observed evidence</h4>
        <span className="font-mono text-[10px] text-zinc-600">{total}</span>
      </div>
      <EvidencePreview item={evidence[0]} anchorId={evidenceAnchor(evidenceReference(evidence[0], 0))} />
      {evidence.length > 1 && (
        <details>
          <summary className="cursor-pointer text-xs text-zinc-400 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">Show {evidence.length - 1} more</summary>
          <div className="mt-2 space-y-2">{evidence.slice(1).map((item, index) => <EvidencePreview key={index} item={item} anchorId={evidenceAnchor(evidenceReference(item, index + 1))} />)}</div>
        </details>
      )}
      {total > evidence.length && <div className="text-[10px] text-zinc-500">Showing {evidence.length} of {total}. Open diagnostics for the full set.</div>}
    </section>
  );
}

function incidentEnrichments(row: JsonRecord): Array<[string, JsonRecord]> {
  if (typeof row.incident_id !== "string") return [];
  const enrichments = asRecord(row.enrichments);
  if (!enrichments) return [];
  return Object.entries(enrichments)
    .map(([key, value]) => [key, asRecord(value)] as const)
    .filter((entry): entry is [string, JsonRecord] => entry[1] !== null);
}

function normalizedEnrichmentStatus(enrichment: JsonRecord): string {
  return typeof enrichment.status === "string" ? enrichment.status.toLowerCase() : "";
}

function hasReadyIncidentEnrichment(row: JsonRecord): boolean {
  return incidentEnrichments(row).some(([, enrichment]) => normalizedEnrichmentStatus(enrichment) === "ready");
}

function IncidentAiEnrichment({ row }: { row: JsonRecord }) {
  const { labels } = useI18n();
  const enrichments = incidentEnrichments(row);
  if (enrichments.length === 0) return null;
  return (
    <section className="space-y-2 border-t border-zinc-800 pt-2" aria-label={labels.enrichment.section}>
      {enrichments.map(([key, enrichment]) => {
        const status = normalizedEnrichmentStatus(enrichment);
        const data = asRecord(enrichment.data) ?? {};
        const citations = Array.isArray(data.citations)
          ? data.citations.map(asRecord).filter((citation): citation is JsonRecord => citation !== null)
          : [];
        if (status === "ready") {
          return (
            <div key={key} className="space-y-2">
              <h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.enrichment.section}</h4>
              <p className="text-xs leading-5 text-zinc-300">{scalarText(data.summary ?? labels.enrichment.summaryReady)}</p>
              {citations.length > 0 && <ul className="space-y-1">{citations.map((citation, index) => {
                const reference = typeof citation.evidence_ref === "string" ? citation.evidence_ref : "";
                return <li key={`${reference}-${index}`}><a className="text-[11px] text-[#D69A5C] underline decoration-[#B87333]/50 underline-offset-2 hover:text-[#E1B07A]" href={`#${evidenceAnchor(reference)}`}>{scalarText(citation.reason ?? labels.enrichment.viewEvidenceLog)} · {labels.enrichment.viewEvidenceLog}</a></li>;
              })}</ul>}
            </div>
          );
        }
        if (status === "stale") {
          return <div key={key}><h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.enrichment.section}</h4><p className="mt-1 text-xs text-amber-300">{labels.enrichment.stale}</p></div>;
        }
        if (status.startsWith("failed")) {
          return <div key={key}><h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.enrichment.section}</h4><p className="mt-1 text-xs text-zinc-400">{labels.enrichment.failed}</p><details className="mt-2"><summary className="cursor-pointer text-[11px] text-zinc-500">{labels.enrichment.technicalDetails}</summary><code className="mt-1 block text-[10px] text-zinc-600">{enrichment.status as string}</code></details></div>;
        }
        if (status.startsWith("unavailable")) {
          return <div key={key} className="flex items-center justify-between gap-3"><h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.enrichment.section}</h4><span className="border border-zinc-700 px-1.5 py-0.5 text-[10px] text-zinc-400">{labels.enrichment.bundleDisabled}</span></div>;
        }
        if (status === "pending" || status === "running") {
          return <div key={key}><h4 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{labels.enrichment.section}</h4><p className="mt-1 text-xs text-zinc-500">{labels.enrichment.preparing}</p></div>;
        }
        return null;
      })}
    </section>
  );
}

function CardsRenderer({ payload, rowAction, rowActions = [] }: { payload: unknown; rowAction?: DeclarativeRowAction; rowActions?: DeclarativeRowAction[] }) {
  const rows = rowsFrom(payload);
  if (rows.length === 0) return <EmptyState title="No cards" description="The Bundle returned no card records." />;
  return <div className="grid gap-3 p-3 md:grid-cols-2">{rows.map((row, index) => {
    const semanticTitle = row.title ?? row.name ?? row.label ?? row.headline ?? row.subject ?? row.objective;
    const title = semanticTitle !== undefined && semanticTitle !== null && semanticTitle !== ""
      ? scalarText(semanticTitle)
      : row.kind !== undefined && row.kind !== null && row.kind !== ""
        ? humanLabel(scalarText(row.kind))
        : `Result ${index + 1}`;
    const summary = row.reason ?? row.summary ?? row.statement ?? row.body ?? row.key_changes ?? row.description ?? row.notes;
    const status = row.freshness ?? row.status ?? row.state ?? row.branch_status;
    const severity = typeof row.severity === "string" ? row.severity.toLowerCase() : null;
    const hasSupport = row.supporting_source_id !== undefined && row.supporting_source_id !== null;
    const supportingClaims = Number(row.supporting_claims ?? 0);
    const contradictingClaims = Number(row.contradicting_claims ?? 0);
    const sourceDiversity = [
      ["Official", Number(row.official_sources ?? 0)],
      ["Editorial", Number(row.editorial_sources ?? 0)],
      ["Community", Number(row.community_sources ?? 0)],
    ] as const;
    const hasDiversity = sourceDiversity.some(([, count]) => Number.isFinite(count) && count > 0);
    const hasConflict = row.contradicting_source_id !== undefined && row.contradicting_source_id !== null || contradictingClaims > 0;
    const critical = severity === "critical";
    const needsAttention = critical || severity === "high" || status === "firing"
      || ["needs_attention", "open_incidents", "quarantined", "unreachable", "compliance_drift"]
        .some((key) => typeof row[key] === "number" && row[key] > 0);
    return (
      <Card key={index} className={critical ? "border-red-900/70 bg-red-950/10" : needsAttention ? "border-amber-800/70 bg-amber-950/10" : "border-zinc-800 bg-zinc-950/60"}>
        <CardContent className="space-y-3 p-3">
          <div className="flex items-start justify-between gap-3">
            <h3 className="min-w-0 text-sm font-medium text-zinc-100">{title}</h3>
            <div className="flex shrink-0 gap-1">{severity && <span className={`rounded-sm border px-1.5 py-0.5 font-mono text-[10px] ${critical ? "border-red-800 text-red-300" : severity === "high" ? "border-amber-800 text-amber-300" : "border-zinc-700 text-zinc-400"}`}>{humanLabel(severity)}</span>}{status !== undefined && status !== null && <span className="rounded-sm border border-zinc-700 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">{humanLabel(scalarText(status))}</span>}{hasReadyIncidentEnrichment(row) && <span className="rounded-sm border border-[#B87333]/60 px-1.5 py-0.5 text-[10px] text-[#D69A5C]">AI</span>}</div>
          </div>
          {summary !== undefined && summary !== null && summary !== "" && <p className="line-clamp-3 text-xs leading-5 text-zinc-300">{scalarText(summary)}</p>}
          {cardFields(row).length > 0 && <dl className="grid gap-2 border-t border-zinc-800 pt-2">{cardFields(row).map(([key, value]) => <div key={key} className="min-w-0"><dt className="text-[10px] uppercase tracking-wider text-zinc-600">{humanLabel(key)}</dt><dd className="mt-0.5 break-words font-mono text-xs leading-4 text-zinc-400">{scalarText(value)}</dd></div>)}</dl>}
          <CardEvidence row={row} />
          <IncidentAiEnrichment row={row} />
          {(hasSupport || hasDiversity || hasConflict || supportingClaims > 0) && <div className="flex flex-wrap gap-x-3 gap-y-1 border-t border-zinc-800 pt-2 text-[10px]">{hasSupport && <span className="text-zinc-400">Evidence cited</span>}{hasDiversity && sourceDiversity.filter(([, count]) => Number.isFinite(count) && count > 0).map(([label, count]) => <span key={label} className="text-zinc-500">{label} · {count}</span>)}{supportingClaims > 0 && <span className="text-zinc-400">Supporting claims · {supportingClaims}</span>}{hasConflict && <span className="text-amber-300">Contradictions · {Math.max(1, contradictingClaims)}</span>}</div>}
          {rowAction && <Button type="button" size="sm" variant="outline" className="h-7 w-full text-xs" aria-label={`${rowAction.label} for card ${index + 1}`} onClick={() => rowAction.onInvoke(row, index)}><MessageSquareText className="mr-1 size-3" aria-hidden />{rowAction.label}</Button>}
          {rowActions.some((action) => action.isAvailable?.(row) ?? true) && <div className="grid gap-2 border-t border-zinc-800 pt-2 sm:grid-cols-2">{rowActions.filter((action) => action.isAvailable?.(row) ?? true).map((action) => <Button key={action.label} type="button" size="sm" variant="outline" disabled={action.running} className="h-8 text-xs" onClick={() => action.onInvoke(row, index)}>{action.running ? "Running…" : action.label}</Button>)}</div>}
        </CardContent>
      </Card>
    );
  })}</div>;
}

function DashboardRenderer({ payload, rowAction, platformState }: { payload: unknown; rowAction?: DeclarativeRowAction; platformState?: PlatformRendererState }) {
  const { labels } = useI18n();
  const record = asRecord(payload);
  if (!record) return <Incompatible reason="Dashboard payload must be an object of metrics." />;
  const summary = asRecord(record.summary) ?? record;
  const entries = Object.entries(summary)
    .filter(([, value]) => value === null || ["string", "number", "boolean"].includes(typeof value))
    .filter(([key, value]) => key !== "truncated" || value === true)
    .sort(([left], [right]) => dashboardPriority(left) - dashboardPriority(right) || left.localeCompare(right))
    .slice(0, 24);
  const clusters = Array.isArray(record.clusters) ? record.clusters : [];
  const servers = Array.isArray(record.servers) ? record.servers : [];
  const fleet = asRecord(record.fleet);
  return <div className="space-y-4 p-3"><div className="grid grid-cols-2 gap-3 xl:grid-cols-4">{entries.map(([key, value]) => {
    const count = typeof value === "number" ? value : null;
    const needsAttention = count !== null && count > 0 && /(attention|incident|quarantined|unreachable|drift|failed|critical)/.test(key);
    const partial = key === "truncated" && value === true;
    return <Card key={key} className={needsAttention ? "border-amber-800/70 bg-amber-950/10" : "border-zinc-800 bg-zinc-950/60"}><CardContent className="p-3"><div className={needsAttention ? "text-[10px] font-semibold uppercase tracking-wider text-amber-300" : "text-[10px] font-semibold uppercase tracking-wider text-zinc-600"}>{partial ? labels.emptyStates.partialDataLabel : humanLabel(key)}</div><div className="mt-2 break-words font-mono text-lg text-zinc-200">{partial ? labels.emptyStates.partialDataValue : scalarText(value)}</div></CardContent></Card>;
  })}</div>{clusters.length > 0 && <ClusterDashboard clusters={clusters} />}{servers.length > 0 && <FleetHostMap servers={servers} fleet={fleet} rowAction={rowAction} platformState={platformState} />}</div>;
}

function ClusterDashboard({ clusters }: { clusters: unknown[] }) {
  return (
    <section aria-labelledby="cluster-status-heading">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h2 id="cluster-status-heading" className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">Cluster status</h2>
        <span className="font-mono text-[10px] text-zinc-600">{clusters.length} total</span>
      </div>
      <div className="grid gap-3 xl:grid-cols-2">
        {clusters.map((value, index) => {
          const cluster = asRecord(value) ?? {};
          const telemetry = asRecord(cluster.telemetry) ?? {};
          const servers = numericValue(cluster.servers) ?? 0;
          const active = numericValue(cluster.active_servers) ?? 0;
          const attention = cluster.operational_status === "needs_attention"
            || ["needs_attention", "quarantined", "compliance_drift"].some((key) => (numericValue(cluster[key]) ?? 0) > 0);
          const empty = cluster.operational_status === "empty" || servers === 0;
          const statusLabel = attention ? "Needs attention" : empty ? "No servers" : "No issues detected";
          const vitals = [
            ["CPU", telemetry.cpu_average_util_percent, "%"],
            ["GPU", telemetry.gpu_average_util_percent, "%"],
            ["Max temp", telemetry.max_temperature_c, "°C"],
            ["Telemetry", telemetry.current_servers, ` / ${servers}`],
          ] as const;
          return (
            <Card key={scalarText(cluster.cluster_id ?? index)} className={attention ? "border-amber-800/70 bg-amber-950/10" : "border-zinc-800 bg-zinc-950/60"}>
              <CardContent className="p-0">
                <header className="flex items-start justify-between gap-3 border-b border-zinc-800 px-3 py-3">
                  <div className="min-w-0">
                    <h3 className="truncate text-sm font-medium text-zinc-100">{scalarText(cluster.label ?? cluster.cluster_id ?? `Cluster ${index + 1}`)}</h3>
                    <div className="mt-1 text-xs text-zinc-500">{active} of {servers} servers active</div>
                  </div>
                  <span className={attention ? "shrink-0 border border-amber-800 px-2 py-1 text-[10px] text-amber-300" : "shrink-0 border border-zinc-700 px-2 py-1 text-[10px] text-zinc-400"}>{statusLabel}</span>
                </header>
                <dl className="grid grid-cols-2 gap-px bg-zinc-800 sm:grid-cols-4">
                  {vitals.map(([label, value, suffix]) => (
                    <div key={label} className="min-w-0 bg-[#101418] px-3 py-3">
                      <dt className="text-[10px] uppercase tracking-wider text-zinc-600">{label}</dt>
                      <dd className="mt-1 truncate font-mono text-base text-zinc-200">{numericValue(value) === null ? "—" : `${numericValue(value)}${suffix}`}</dd>
                    </div>
                  ))}
                </dl>
              </CardContent>
            </Card>
          );
        })}
      </div>
    </section>
  );
}

function numericValue(value: unknown): number | null {
  const numeric = typeof value === "number" ? value : Number.NaN;
  return Number.isFinite(numeric) ? numeric : null;
}

type FleetFill = "health" | "cpu" | "memory" | "gpu" | "temperature" | "power";
type FleetGrouping = "cluster-role" | "cluster" | "role";
type FleetFilter = "all" | "attention" | "healthy" | "no-telemetry";
type FleetDensity = "auto" | "dense" | "labeled";
type FleetResolvedDensity = "dense" | "compact" | "labeled";

const FLEET_FILL_OPTIONS: Array<{ id: FleetFill; label: string; key?: string; unit?: string; range?: [number, number] }> = [
  { id: "health", label: "Health" },
  { id: "cpu", label: "CPU", key: "cpu_util_percent", unit: "%", range: [0, 100] },
  { id: "memory", label: "Memory", key: "memory_used_percent", unit: "%", range: [0, 100] },
  { id: "gpu", label: "GPU", key: "gpu_util_percent", unit: "%", range: [0, 100] },
  { id: "temperature", label: "Temperature", key: "temperature_c", unit: "°C" },
  { id: "power", label: "Power", key: "power_w", unit: "W" },
];

const FLEET_STATUS_COLORS: Record<string, string> = {
  critical: "#7f1d1d",
  unreachable: "#581c87",
  quarantined: "#9a3412",
  warning: "#92400e",
  stale: "#713f12",
  enrolling: "#334155",
  no_telemetry: "#18181b",
  healthy: "#27272a",
};

const FLEET_STATUS_PRIORITY: Record<string, number> = {
  critical: 0,
  unreachable: 1,
  quarantined: 2,
  warning: 3,
  stale: 4,
  no_telemetry: 5,
  enrolling: 6,
  healthy: 7,
};

function compareFleetRows(left: JsonRecord, right: JsonRecord): number {
  const leftStatus = scalarText(left.node_status);
  const rightStatus = scalarText(right.node_status);
  return (FLEET_STATUS_PRIORITY[leftStatus] ?? 99) - (FLEET_STATUS_PRIORITY[rightStatus] ?? 99)
    || scalarText(left.server).localeCompare(scalarText(right.server));
}

function fleetAttentionCount(rows: JsonRecord[]): number {
  return rows.filter((row) => !["healthy", "no_telemetry", "enrolling"].includes(scalarText(row.node_status))).length;
}

function initialFleetOption<T extends string>(name: string, allowed: readonly T[], fallback: T): T {
  if (typeof window === "undefined") return fallback;
  const value = new URLSearchParams(window.location.search).get(name) as T | null;
  return value && allowed.includes(value) ? value : fallback;
}

function initialFleetText(name: string): string {
  if (typeof window === "undefined") return "";
  return new URLSearchParams(window.location.search).get(name)?.slice(0, 128) ?? "";
}

function persistFleetOption(name: string, value: string, fallback = "") {
  if (typeof window === "undefined") return;
  const url = new URL(window.location.href);
  if (value === fallback || value === "") url.searchParams.delete(name);
  else url.searchParams.set(name, value);
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

function FleetHostMap({ servers, fleet, rowAction, platformState }: { servers: unknown[]; fleet?: JsonRecord | null; rowAction?: DeclarativeRowAction; platformState?: PlatformRendererState }) {
  const { labels } = useI18n();
  const allRows = servers.map((value) => asRecord(value)).filter((value): value is JsonRecord => value !== null);
  const rows = allRows.slice(0, MAX_ITEMS);
  const [query, setQuery] = useState(() => initialFleetText("fleet_q"));
  const [filter, setFilter] = useState<FleetFilter>(() => initialFleetOption("fleet_status", ["all", "attention", "healthy", "no-telemetry"] as const, "all"));
  const [grouping, setGrouping] = useState<FleetGrouping>(() => initialFleetOption("fleet_group", ["cluster-role", "cluster", "role"] as const, "cluster-role"));
  const [fill, setFill] = useState<FleetFill>(() => initialFleetOption("fleet_fill", ["health", "cpu", "memory", "gpu", "temperature", "power"] as const, "health"));
  const [density, setDensity] = useState<FleetDensity>(() => initialFleetOption("fleet_density", ["auto", "dense", "labeled"] as const, "auto"));
  const [localSelectedTarget, setLocalSelectedTarget] = useState(() => initialFleetText("fleet_server"));
  const [listSort, setListSort] = useState<"server" | "status" | "cluster">("server");
  const [collapsedGroups, setCollapsedGroups] = useState<Set<string>>(() => new Set());
  const normalizedQuery = query.trim().toLowerCase();
  const filtered = rows.filter((row) => {
    const status = scalarText(row.node_status).toLowerCase();
    const matchesQuery = normalizedQuery === "" || [row.server, row.target_id, row.cluster, row.role]
      .some((value) => scalarText(value).toLowerCase().includes(normalizedQuery));
    const matchesStatus = filter === "all"
      || (filter === "attention" && !["healthy", "no_telemetry", "enrolling"].includes(status))
      || (filter === "healthy" && status === "healthy")
      || (filter === "no-telemetry" && status === "no_telemetry");
    return matchesQuery && matchesStatus;
  });
  const fillOption = FLEET_FILL_OPTIONS.find((option) => option.id === fill) ?? FLEET_FILL_OPTIONS[0];
  const observed = fillOption.key
    ? filtered.map((row) => numericValue(row[fillOption.key!])).filter((value): value is number => value !== null)
    : [];
  const observedMin = observed.length > 0 ? Math.min(...observed) : 0;
  const observedMax = observed.length > 0 ? Math.max(...observed) : 0;
  const range: [number, number] = fillOption.range ?? [observedMin, observedMax];
  const groups = new Map<string, JsonRecord[]>();
  for (const row of filtered) {
    const cluster = scalarText(row.cluster);
    const role = humanLabel(scalarText(row.role));
    const key = grouping === "cluster" ? cluster : grouping === "role" ? role : `${cluster} · ${role}`;
    groups.set(key, [...(groups.get(key) ?? []), row]);
  }
  const groupEntries = Array.from(groups.entries())
    .map(([group, groupRows]) => [group, [...groupRows].sort(compareFleetRows)] as const)
    .sort((left, right) => fleetAttentionCount(right[1]) - fleetAttentionCount(left[1]) || left[0].localeCompare(right[0]));
  const selectedTarget = platformState?.selectedServerId ?? localSelectedTarget;
  const selected = filtered.find((row) => scalarText(row.target_id) === selectedTarget) ?? null;
  const selectTarget = (target: string) => {
    if (platformState) {
      platformState.selectServer(target);
      return;
    }
    setLocalSelectedTarget(target);
    persistFleetOption("fleet_server", target);
  };
  const sortedList = [...filtered].sort((left, right) => {
    const leftValue = listSort === "server" ? left.server : listSort === "status" ? left.node_status : left.cluster;
    const rightValue = listSort === "server" ? right.server : listSort === "status" ? right.node_status : right.cluster;
    return scalarText(leftValue).localeCompare(scalarText(rightValue));
  });
  const resolvedDensity: FleetResolvedDensity = density === "dense"
    ? "dense"
    : density === "labeled"
      ? "labeled"
      : filtered.length > 120 ? "dense" : filtered.length > 40 ? "compact" : "labeled";
  const sourceTruncated = fleet?.truncated === true || allRows.length > MAX_ITEMS;
  const sourceTotal = numericValue(fleet?.total_servers);
  const attentionCount = fleetAttentionCount(filtered);
  const healthyCount = filtered.filter((row) => scalarText(row.node_status) === "healthy").length;
  const noTelemetryCount = filtered.filter((row) => scalarText(row.node_status) === "no_telemetry").length;

  return (
    <section className="space-y-3" aria-labelledby="fleet-map-heading" data-testid="fleet-host-map">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="fleet-map-heading" className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">Fleet map</h2>
          <p className="mt-1 text-xs text-zinc-500">One hex per server · attention first · current signed snapshot</p>
        </div>
        <span className="font-mono text-[10px] text-zinc-600">
          {sourceTruncated ? `${filtered.length} shown · source truncated` : `${filtered.length} of ${sourceTotal ?? rows.length}`}
        </span>
      </div>
      {sourceTruncated && <InlineNotice tone="warn" title="Fleet Map is showing a bounded subset">Refine the filter before treating this map as complete.</InlineNotice>}
      <div className="grid gap-2 border border-zinc-800 bg-zinc-950/40 p-3 sm:grid-cols-2 xl:grid-cols-5">
        <label className="grid gap-1 text-[10px] uppercase tracking-wider text-zinc-500">
          Find server
          <input value={query} onChange={(event) => { setQuery(event.target.value); persistFleetOption("fleet_q", event.target.value); }} placeholder="Name, cluster or role" className="h-8 min-w-0 border border-zinc-700 bg-zinc-950 px-2 text-xs normal-case tracking-normal text-zinc-200 outline-none focus:border-[#B87333]" />
        </label>
        <FleetSelect label="Status" value={filter} options={[["all", "All"], ["attention", "Needs attention"], ["healthy", "Healthy"], ["no-telemetry", "No telemetry"]]} onChange={(value) => { const next = value as FleetFilter; setFilter(next); persistFleetOption("fleet_status", next, "all"); }} />
        <FleetSelect label="Group" value={grouping} options={[["cluster-role", "Cluster → role"], ["cluster", "Cluster"], ["role", "Role"]]} onChange={(value) => { const next = value as FleetGrouping; setGrouping(next); persistFleetOption("fleet_group", next, "cluster-role"); }} />
        <FleetSelect label="Fill" value={fill} options={FLEET_FILL_OPTIONS.map((option) => [option.id, option.label])} onChange={(value) => { const next = value as FleetFill; setFill(next); persistFleetOption("fleet_fill", next, "health"); }} />
        <FleetSelect label="Density" value={density} options={[["auto", "Auto"], ["dense", "Dense"], ["labeled", "Labeled"]]} onChange={(value) => { const next = value as FleetDensity; setDensity(next); persistFleetOption("fleet_density", next, "auto"); }} />
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1 border-b border-zinc-800 pb-2 text-[10px] text-zinc-500" aria-label="Filtered fleet status">
        <span><strong className="font-mono text-amber-300">{attentionCount}</strong> attention</span>
        <span><strong className="font-mono text-zinc-300">{healthyCount}</strong> healthy</span>
        <span><strong className="font-mono text-zinc-400">{noTelemetryCount}</strong> no telemetry</span>
        <span className="ml-auto">{humanLabel(resolvedDensity)} density</span>
      </div>
      {fill !== "health" && (
        <div className="flex flex-wrap items-center gap-3 border-l border-[#B87333] pl-3 text-[10px] text-zinc-500" data-testid="fleet-fill-legend">
          <span className="font-semibold uppercase tracking-wider text-zinc-400">{fillOption.label} magnitude</span>
          <span className="h-2 w-32 bg-gradient-to-r from-sky-950 via-sky-600 to-[#D89B5A]" aria-hidden />
          <span className="font-mono">{formatFleetMetric(range[0], fillOption.unit)} – {formatFleetMetric(range[1], fillOption.unit)}</span>
          <span>Color shows magnitude, not health.</span>
        </div>
      )}
      {filtered.length === 0 ? (
        <EmptyState
          title={labels.emptyStates.fleetNoMatchesTitle}
          description={labels.emptyStates.fleetNoMatchesDescription}
        />
      ) : (
        <div className="grid gap-3 2xl:grid-cols-[minmax(0,1fr)_320px]">
          <div className="max-h-[calc(100vh-19rem)] min-h-[28rem] space-y-3 overflow-auto border border-zinc-800 bg-[#101418] p-3" data-testid="fleet-map-viewport">
            {groupEntries.map(([group, groupRows]) => {
              const groupAttention = fleetAttentionCount(groupRows);
              const collapsed = collapsedGroups.has(group);
              return (
                <section key={group} aria-label={group}>
                  <button
                    type="button"
                    aria-expanded={!collapsed}
                    onClick={() => setCollapsedGroups((current) => {
                      const next = new Set(current);
                      if (next.has(group)) next.delete(group); else next.add(group);
                      return next;
                    })}
                    className="sticky top-0 z-10 mb-2 flex w-full items-center gap-2 border-b border-zinc-800 bg-[#101418] pb-1.5 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]"
                  >
                    <ChevronRight className={`size-3 text-zinc-500 transition-transform ${collapsed ? "" : "rotate-90"}`} aria-hidden />
                    <h3 className="text-xs font-medium text-zinc-300">{group}</h3>
                    <span className="font-mono text-[10px] text-zinc-600">{groupRows.length}</span>
                    {groupAttention > 0 && <span className="ml-auto text-[10px] text-amber-300">{groupAttention} attention</span>}
                  </button>
                  {!collapsed && <div className="flex flex-wrap items-center gap-1" role="list" aria-label={`${group} servers`}>
                    {groupRows.map((row) => {
                      const target = scalarText(row.target_id);
                      const status = scalarText(row.node_status);
                      const value = fillOption.key ? numericValue(row[fillOption.key]) : null;
                      const background = fill === "health" ? fleetStatusBackground(status) : fleetMetricBackground(value, range);
                      return (
                        <div key={target} role="listitem">
                          <button
                            type="button"
                            aria-pressed={selectedTarget === target}
                            aria-label={`${scalarText(row.server)}, ${humanLabel(status)}, ${scalarText(row.cluster)}, ${humanLabel(scalarText(row.role))}`}
                            title={`${scalarText(row.server)} · ${humanLabel(status)}`}
                            onClick={() => selectTarget(target)}
                            className={`flex shrink-0 flex-col items-center justify-center text-center text-zinc-100 shadow-inner outline-none transition-transform hover:z-10 hover:scale-110 focus-visible:z-10 focus-visible:ring-2 focus-visible:ring-[#D89B5A] ${resolvedDensity === "dense" ? "h-6 w-7 p-0" : resolvedDensity === "compact" ? "h-10 w-12 px-1" : "h-[68px] w-[78px] px-2"}`}
                            style={{ clipPath: "polygon(25% 0%, 75% 0%, 100% 50%, 75% 100%, 25% 100%, 0% 50%)", background, boxShadow: selectedTarget === target ? "inset 0 0 0 3px #f4f4f5" : `inset 0 0 0 2px ${FLEET_STATUS_COLORS[status] ?? FLEET_STATUS_COLORS.healthy}` }}
                          >
                            {resolvedDensity === "labeled" && <span className="max-w-full truncate text-[10px] font-semibold">{scalarText(row.server)}</span>}
                            {resolvedDensity !== "dense" && <span className={resolvedDensity === "labeled" ? "mt-1 font-mono text-[10px] text-zinc-300" : "max-w-full truncate font-mono text-[9px] text-zinc-200"}>{fill === "health" ? humanLabel(status) : formatFleetMetric(value, fillOption.unit)}</span>}
                          </button>
                        </div>
                      );
                    })}
                  </div>}
                </section>
              );
            })}
          </div>
          <aside className="2xl:sticky 2xl:top-3 2xl:self-start" aria-label="Selected server">
            {selected ? <FleetServerDetail server={selected} rowAction={rowAction} platformState={platformState} /> : <Card className="border-zinc-800 bg-zinc-950/60"><CardContent className="p-4 text-xs leading-5 text-zinc-500">Select a server to inspect current vitals and continue to Metrics, Incidents, Logs or Penny.</CardContent></Card>}
          </aside>
        </div>
      )}
      <details className="border border-zinc-800 bg-zinc-950/40" data-testid="fleet-list-fallback">
        <summary className="flex cursor-pointer items-center justify-between px-3 py-2 text-xs text-zinc-300 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">
          Accessible server list
          <span className="font-mono text-[10px] text-zinc-600">{filtered.length}</span>
        </summary>
        <div className="border-t border-zinc-800 p-3">
          <div className="mb-2 flex justify-end"><FleetSelect label="Sort list" value={listSort} options={[["server", "Server"], ["status", "Status"], ["cluster", "Cluster"]]} onChange={(value) => setListSort(value as typeof listSort)} /></div>
          <div className="overflow-x-auto">
            <table className="w-full min-w-[640px] text-left text-xs">
              <thead><tr className="border-b border-zinc-700 text-zinc-500"><th className="px-2 py-2">Server</th><th className="px-2 py-2">Status</th><th className="px-2 py-2">Cluster</th><th className="px-2 py-2">Role</th><th className="px-2 py-2">Telemetry</th></tr></thead>
              <tbody>
                {sortedList.length === 0 ? (
                  <tr className="border-b border-zinc-900 text-zinc-500">
                    <td colSpan={5} className="px-2 py-4 text-center">
                      {labels.emptyStates.fleetNoMatchesCell}
                    </td>
                  </tr>
                ) : sortedList.map((row) => <tr key={scalarText(row.target_id)} className="border-b border-zinc-900 text-zinc-300"><td className="px-2 py-2">{scalarText(row.server)}</td><td className="px-2 py-2">{humanLabel(scalarText(row.node_status))}</td><td className="px-2 py-2">{scalarText(row.cluster)}</td><td className="px-2 py-2">{humanLabel(scalarText(row.role))}</td><td className="px-2 py-2">{humanLabel(scalarText(row.telemetry_status))}</td></tr>)}
              </tbody>
            </table>
          </div>
        </div>
      </details>
    </section>
  );
}

function FleetSelect({ label, value, options, onChange }: { label: string; value: string; options: string[][]; onChange: (value: string) => void }) {
  return <label className="grid gap-1 text-[10px] uppercase tracking-wider text-zinc-500">{label}<select value={value} onChange={(event) => onChange(event.target.value)} className="h-8 min-w-0 border border-zinc-700 bg-zinc-950 px-2 text-xs normal-case tracking-normal text-zinc-200 outline-none focus:border-[#B87333]">{options.map(([id, optionLabel]) => <option key={id} value={id}>{optionLabel}</option>)}</select></label>;
}

function fleetStatusBackground(status: string): string {
  if (status === "no_telemetry") return "repeating-linear-gradient(135deg, #18181b, #18181b 7px, #27272a 7px, #27272a 10px)";
  return FLEET_STATUS_COLORS[status] ?? FLEET_STATUS_COLORS.healthy;
}

function fleetMetricBackground(value: number | null, [minimum, maximum]: [number, number]): string {
  if (value === null) return fleetStatusBackground("no_telemetry");
  const span = maximum - minimum;
  const ratio = span <= 0 ? 0.5 : Math.max(0, Math.min(1, (value - minimum) / span));
  const hue = 210 - ratio * 180;
  const lightness = 20 + ratio * 18;
  return `hsl(${hue} 58% ${lightness}%)`;
}

function formatFleetMetric(value: number | null, unit = ""): string {
  return value === null ? "No data" : `${Math.round(value * 10) / 10}${unit}`;
}

function FleetServerDetail({ server, rowAction, platformState }: { server: JsonRecord; rowAction?: DeclarativeRowAction; platformState?: PlatformRendererState }) {
  const target = scalarText(server.target_id);
  const range = "live";
  const query = `&target_id=${encodeURIComponent(target)}&range=${range}`;
  const metricsHref = platformState
    ? platformState.workspaceHref("server-administrator.metrics")
    : `/workspace?id=server-administrator.metrics${query}`;
  const incidentsHref = platformState
    ? platformState.workspaceHref("server-administrator.alerts")
    : `/workspace?id=server-administrator.alerts${query}`;
  const logsHref = platformState
    ? platformState.workspaceHref("server-administrator.logs")
    : `/workspace?id=server-administrator.logs${query}`;
  const vitals = [
    ["CPU", server.cpu_util_percent, "%"],
    ["Memory", server.memory_used_percent, "%"],
    ["GPU", server.gpu_util_percent, "%"],
    ["Temperature", server.temperature_c, "°C"],
    ["Power", server.power_w, "W"],
  ] as const;
  return <Card className="border-zinc-700 bg-zinc-950/60" data-testid="fleet-server-detail"><CardContent className="space-y-3 p-3"><div className="flex flex-wrap items-start justify-between gap-3"><div><h3 className="text-sm font-medium text-zinc-100">{scalarText(server.server)}</h3><p className="mt-1 text-xs text-zinc-500">{scalarText(server.cluster)} · {humanLabel(scalarText(server.role))} · {humanLabel(scalarText(server.node_status))}</p></div><span className="font-mono text-[10px] text-zinc-600">Current snapshot</span></div><dl className="grid grid-cols-2 gap-px bg-zinc-800 sm:grid-cols-5">{vitals.map(([label, value, unit]) => <div key={label} className="bg-[#101418] px-3 py-2"><dt className="text-[10px] uppercase tracking-wider text-zinc-600">{label}</dt><dd className="mt-1 font-mono text-xs text-zinc-200">{formatFleetMetric(numericValue(value), unit)}</dd></div>)}</dl><div className="flex flex-wrap gap-2 border-t border-zinc-800 pt-3"><Link href={metricsHref} className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">Open Metrics</Link><Link href={incidentsHref} className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">Open Incidents</Link><Link href={logsHref} className="inline-flex h-8 items-center border border-zinc-700 px-3 text-xs text-zinc-300 hover:bg-zinc-900 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">Open Logs</Link>{rowAction && <Button type="button" size="sm" variant="outline" className="h-8 text-xs" onClick={() => rowAction.onInvoke(server, 0)}><MessageSquareText className="mr-1 size-3" aria-hidden />Ask Penny</Button>}</div></CardContent></Card>;
}

function dashboardPriority(key: string): number {
  if (/^(clusters|servers|registered_targets)$/.test(key)) return 0;
  if (/^(active|healthy|healthy_targets|active_servers)$/.test(key)) return 1;
  if (/(attention|incident|quarantined|unreachable|drift|failed|critical)/.test(key)) return 2;
  if (/(enrolling|commissioning|configuring|qualifying|backlog)/.test(key)) return 3;
  if (/(telemetry|observed|updated|fresh)/.test(key)) return 4;
  return 5;
}

function TimelineRenderer({ payload }: { payload: unknown }) {
  const items = rowsFrom(payload);
  if (items.length === 0) return <EmptyState title="No timeline entries" description="No dated activity was returned." />;
  return <ol className="relative ml-5 border-l border-zinc-800 py-2">{items.map((item, index) => {
    const details = [item.trip_title, item.place, item.timezone, item.kind, item.status]
      .filter((value) => value !== undefined && value !== null && value !== "")
      .map(scalarText);
    return <li key={index} className="relative mb-3 ml-4"><span className="absolute -left-[21px] top-1 size-2 rounded-full border border-zinc-600 bg-zinc-950" /><div className="text-[10px] font-mono text-zinc-500">{scalarText(item.at ?? item.timestamp ?? item.date ?? item.start ?? item.starts_at)}</div><div className="mt-1 text-xs text-zinc-200">{scalarText(item.title ?? item.label ?? item.summary ?? `Entry ${index + 1}`)}</div>{details.length > 0 && <div className="mt-1 font-mono text-[10px] text-zinc-500">{details.join(" · ")}</div>}</li>;
  })}</ol>;
}

const OPERATION_STATE_LABELS: Record<string, string> = {
  recovered: "Recovered",
  unchanged: "Already healthy",
  applied: "Plan updated",
  rolled_back: "Previous state restored",
  safe_stopped: "Stopped safely",
};

function OperationState({ title, value }: { title: string; value: unknown }) {
  const record = asRecord(value);
  const fields = record
    ? Object.entries(record).filter(([, child]) =>
        child === null || ["string", "number", "boolean"].includes(typeof child),
      ).slice(0, 8)
    : [];
  return (
    <section className="min-w-0 border border-zinc-800 bg-[#101418] p-3">
      <h3 className="text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{title}</h3>
      {fields.length > 0 ? (
        <dl className="mt-3 grid gap-2">
          {fields.map(([key, child]) => (
            <div key={key} className="flex items-baseline justify-between gap-4 border-t border-zinc-900 pt-2">
              <dt className="text-xs text-zinc-500">{humanLabel(key)}</dt>
              <dd className="max-w-[65%] break-words text-right font-mono text-xs text-zinc-200">{scalarText(child)}</dd>
            </div>
          ))}
        </dl>
      ) : (
        <div className="mt-3 font-mono text-xs text-zinc-300">{scalarText(value)}</div>
      )}
    </section>
  );
}

function OperationRenderer({ payload }: { payload: unknown }) {
  const record = asRecord(payload);
  if (!record) return <Incompatible reason="Operation payload must be an object." />;
  const status = typeof record.status === "string" ? record.status : "safe_stopped";
  const stateLabel = OPERATION_STATE_LABELS[status] ?? humanLabel(status);
  const target = scalarText(record.target ?? record.title ?? record.subject);
  const issue = scalarText(record.issue ?? record.summary ?? "State change");
  const action = scalarText(record.action ?? record.next_action);
  const terminalTone = status === "safe_stopped"
    ? "border-red-900/70 text-red-200"
    : status === "rolled_back"
      ? "border-amber-800/70 text-amber-200"
      : "border-zinc-700 text-zinc-200";
  const technical = Object.fromEntries(
    Object.entries(record).filter(([key]) =>
      !["status", "target", "title", "subject", "issue", "summary", "action", "next_action", "before", "after", "rollback_available"].includes(key),
    ),
  );
  return (
    <div className="space-y-3 p-3" data-testid="operation-result">
      <header className="flex flex-wrap items-start justify-between gap-3 border-b border-zinc-800 pb-3">
        <div className="min-w-0">
          <div className="text-sm font-medium text-zinc-100">{target}</div>
          <div className="mt-1 text-xs text-zinc-400">{issue}</div>
        </div>
        <span className={["shrink-0 border px-2 py-1 font-mono text-[10px] uppercase tracking-wider", terminalTone].join(" ")}>{stateLabel}</span>
      </header>
      <div className="flex items-center gap-2 text-xs text-zinc-300">
        <span className="text-zinc-600">Action</span>
        <span>{action}</span>
      </div>
      {(record.before !== undefined || record.after !== undefined) && (
        <div className="grid gap-3 md:grid-cols-2">
          <OperationState title="Before" value={record.before} />
          <OperationState title="After" value={record.after} />
        </div>
      )}
      {record.rollback_available === true && status !== "rolled_back" && (
        <div className="border-l border-[#B87333] pl-3 text-xs text-zinc-300">Previous state can be restored.</div>
      )}
      {Object.keys(technical).length > 0 && (
        <details className="border-t border-zinc-800 pt-2">
          <summary className="cursor-pointer text-xs text-zinc-500 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">Technical details</summary>
          <div className="mt-2"><DetailRenderer payload={technical} /></div>
        </details>
      )}
    </div>
  );
}

function CalendarRenderer({ payload }: { payload: unknown }) {
  const items = rowsFrom(payload).map((item, index) => ({ item, index, date: scalarText(item.date ?? item.start ?? item.at) })).sort((a, b) => a.date.localeCompare(b.date));
  if (items.length === 0) return <EmptyState title="No calendar items" description="No scheduled records were returned." />;
  return <div className="grid gap-2 p-3 md:grid-cols-2 xl:grid-cols-3">{items.map(({ item, index, date }) => <Card key={index} className="border-zinc-800 bg-zinc-950/60"><CardContent className="p-3"><div className="font-mono text-[10px] text-zinc-500">{date}</div><div className="mt-1 text-xs font-medium text-zinc-200">{scalarText(item.title ?? item.label ?? `Item ${index + 1}`)}</div></CardContent></Card>)}</div>;
}

function GraphRenderer({ payload }: { payload: unknown }) {
  return <InteractiveGraphRenderer payload={payload} />;
}

function MapRenderer({ payload }: { payload: unknown }) {
  const points = rowsFrom(payload).filter((item) => Number.isFinite(Number(item.latitude ?? item.lat)) && Number.isFinite(Number(item.longitude ?? item.lng ?? item.lon)));
  if (points.length === 0) return <EmptyState title="No map points" description="Map records require numeric latitude and longitude." />;
  return <div className="space-y-2 p-3"><div className="relative h-48 overflow-hidden rounded border border-zinc-800 bg-zinc-950" role="img" aria-label={`${points.length} bounded map points without external tiles`}>{points.slice(0, 100).map((point, index) => { const lat = Number(point.latitude ?? point.lat); const lon = Number(point.longitude ?? point.lng ?? point.lon); return <span key={index} className="absolute size-2 -translate-x-1/2 -translate-y-1/2 rounded-full bg-[#B87333]" style={{ left: `${((lon + 180) / 360) * 100}%`, top: `${((90 - lat) / 180) * 100}%` }} title={scalarText(point.label ?? point.title ?? `Point ${index + 1}`)} />; })}</div><RecordList title="Accessible point list" values={points} /></div>;
}

function RecordList({ title, values }: { title: string; values: unknown[] }) {
  return <Card className="border-zinc-800 bg-zinc-950/60"><CardContent className="p-0"><div className="border-b border-zinc-800 px-3 py-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-500">{title}</div><div className="max-h-96 divide-y divide-zinc-900 overflow-auto">{values.slice(0, MAX_ITEMS).map((value, index) => <div key={index} className="px-3 py-2 text-xs text-zinc-300">{asRecord(value) ? <DetailRenderer payload={value} /> : scalarText(value)}</div>)}</div></CardContent></Card>;
}

function Incompatible({ reason }: { reason: string }) {
  return <div className="p-3"><InlineNotice tone="warn" title="Incompatible contribution" details={reason}>This signed payload cannot be rendered safely. Review the Bundle package in Admin.</InlineNotice></div>;
}

export function DeclarativeRenderer({ renderer, payload, rowAction, rowActions = [], platformState, emptyState }: { renderer: WorkspaceRenderer; payload: unknown; rowAction?: DeclarativeRowAction; rowActions?: DeclarativeRowAction[]; platformState?: PlatformRendererState; emptyState?: DeclarativeEmptyState }) {
  switch (renderer) {
    case "table": return <TableRenderer payload={payload} rowAction={rowAction} rowActions={rowActions} emptyState={emptyState} />;
    case "list": return <ListRenderer payload={payload} />;
    case "cards": return <CardsRenderer payload={payload} rowAction={rowAction} rowActions={rowActions} />;
    case "detail":
    case "form": return <DetailRenderer payload={payload} />;
    case "dashboard": return <DashboardRenderer payload={payload} rowAction={rowAction} platformState={platformState} />;
    case "timeline": return <TimelineRenderer payload={payload} />;
    case "calendar": return <CalendarRenderer payload={payload} />;
    case "map": return <MapRenderer payload={payload} />;
    case "graph": return <GraphRenderer payload={payload} />;
    case "telemetry": return <TelemetryOverviewRenderer payload={payload} />;
    case "timeseries": return <InteractiveTimeseriesRenderer payload={payload} />;
    case "operation": return <OperationRenderer payload={payload} />;
    case "markdown_doc": return typeof payload === "string" ? <div className="p-3"><MarkdownText text={payload.slice(0, 262_144)} /></div> : <Incompatible reason="Markdown payload must be a string." />;
    default: return <Incompatible reason={`Renderer ${String(renderer)} is not supported by this Core build.`} />;
  }
}
