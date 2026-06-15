"use client";

// Dense, sortable fleet TABLE for /web/servers (ISSUE 64). A BaseView-
// style overview that scales to hundreds of hosts where the cards do
// not: every column reads from the lightweight `server-fleet` summary
// (one call per refresh, no per-host SSH), the column headers drive the
// shared sort state, and a row click opens the existing
// HostDetailDrawer. We hand-roll a native <table> to match the admin
// tables and avoid a virtualization dependency — the scroll region is
// capped with a sticky header instead.

import { ArrowDown, ArrowUp, ChevronsUpDown, TriangleAlert } from "lucide-react";

import {
  temperatureColor,
  tileColor,
  tileMetricLabel,
  utilizationColor,
  type FleetHostRow,
  type ServerSortKey,
} from "../lib/server-fleet-view";

type TableHost = {
  id: string;
  host: string;
  alias?: string | null;
  gpus?: string[] | null;
};

// Direction is fixed per key (name ascending, metrics descending) to
// match filterSortHosts; the caret reflects that fixed order. A
// per-column direction toggle is a follow-up.
const SORT_DIR: Record<ServerSortKey, "asc" | "desc"> = {
  name: "asc",
  cpu: "desc",
  gpu: "desc",
  temp: "desc",
  warn: "desc",
};

function SortHeader({
  label,
  sortField,
  sortKey,
  onSortChange,
  align = "left",
}: {
  label: string;
  sortField: ServerSortKey;
  sortKey: ServerSortKey;
  onSortChange: (key: ServerSortKey) => void;
  align?: "left" | "right";
}) {
  const active = sortKey === sortField;
  const dir = SORT_DIR[sortField];
  return (
    <th
      className={`px-3 py-2 font-normal ${align === "right" ? "text-right" : "text-left"}`}
      aria-sort={active ? (dir === "asc" ? "ascending" : "descending") : "none"}
    >
      <button
        type="button"
        onClick={() => onSortChange(sortField)}
        className={`group/sort inline-flex items-center gap-1 transition-colors ${
          align === "right" ? "flex-row-reverse" : ""
        } ${active ? "text-zinc-200" : "text-zinc-500 hover:text-zinc-300"}`}
      >
        {label}
        {active ? (
          dir === "asc" ? (
            <ArrowUp className="size-3" />
          ) : (
            <ArrowDown className="size-3" />
          )
        ) : (
          <ChevronsUpDown className="size-3 opacity-0 transition-opacity group-hover/sort:opacity-50" />
        )}
      </button>
    </th>
  );
}

function UtilCell({ value }: { value: number | null }) {
  if (value == null) return <span className="text-zinc-600">—</span>;
  const clamped = Math.min(100, Math.max(0, value));
  return (
    <div className="flex items-center justify-end gap-2">
      <div className="h-1 w-12 overflow-hidden rounded-full bg-zinc-800">
        <div
          className="h-full rounded-full"
          style={{ width: `${clamped}%`, backgroundColor: utilizationColor(value) }}
        />
      </div>
      <span className="w-9 text-right font-mono tabular-nums text-zinc-300">
        {value.toFixed(0)}%
      </span>
    </div>
  );
}

export function ServerTable({
  hosts,
  fleet,
  sortKey,
  onSortChange,
  onSelect,
}: {
  hosts: TableHost[];
  fleet: ReadonlyMap<string, FleetHostRow>;
  sortKey: ServerSortKey;
  onSortChange: (key: ServerSortKey) => void;
  onSelect: (hostId: string) => void;
}) {
  if (hosts.length === 0) {
    return (
      <div
        className="surface-1 rounded-lg p-6 text-center text-xs text-zinc-500"
        data-testid="server-table-empty"
      >
        No servers match the current filters.
      </div>
    );
  }

  // When the fleet summary is absent (legacy no-DB mode, or the fleet
  // call failed) filterSortHosts forces name order regardless of
  // sortKey — so the header carets must reflect name order too, never a
  // metric caret that disagrees with (and misreports via aria-sort) the
  // actual rows.
  const effectiveSortKey: ServerSortKey = fleet.size === 0 ? "name" : sortKey;

  return (
    <div
      className="surface-1 penny-scroll max-h-[640px] overflow-auto rounded-lg"
      data-testid="server-table"
    >
      <table className="w-full border-collapse text-xs">
        <thead className="sticky top-0 z-10 bg-zinc-950/95 text-[11px] uppercase tracking-wide text-zinc-500 backdrop-blur">
          <tr className="border-b border-white/[0.08]">
            <th className="px-3 py-2 text-left font-normal">Status</th>
            <SortHeader
              label="Name"
              sortField="name"
              sortKey={effectiveSortKey}
              onSortChange={onSortChange}
            />
            <th className="px-3 py-2 text-left font-normal">Host</th>
            <SortHeader
              label="CPU"
              sortField="cpu"
              sortKey={effectiveSortKey}
              onSortChange={onSortChange}
              align="right"
            />
            <th className="px-3 py-2 text-right font-normal">GPUs</th>
            <SortHeader
              label="GPU util"
              sortField="gpu"
              sortKey={effectiveSortKey}
              onSortChange={onSortChange}
              align="right"
            />
            <SortHeader
              label="GPU temp"
              sortField="temp"
              sortKey={effectiveSortKey}
              onSortChange={onSortChange}
              align="right"
            />
            <SortHeader
              label="Warn"
              sortField="warn"
              sortKey={effectiveSortKey}
              onSortChange={onSortChange}
              align="right"
            />
          </tr>
        </thead>
        <tbody>
          {hosts.map((h) => {
            const row = fleet.get(h.id);
            const label = h.alias ?? h.host;
            const gpuCount = row?.gpu_count ?? h.gpus?.length ?? 0;
            // Reuse the tile view's status label so an online host with
            // warnings reads "⚠N" — matching the amber dot — instead of a
            // bare "online" that contradicts the non-green color.
            const statusLabel = tileMetricLabel("status", row);
            const temp = row?.gpu_max_temp_c ?? null;
            const warnings = row?.warnings ?? 0;
            return (
              <tr
                key={h.id}
                data-testid="server-table-row"
                onClick={() => onSelect(h.id)}
                className="cursor-pointer border-b border-white/[0.04] transition-colors last:border-b-0 hover:bg-white/[0.03]"
              >
                <td className="px-3 py-2">
                  <span className="flex items-center gap-2">
                    <span
                      className="size-2 shrink-0 rounded-full"
                      style={{ backgroundColor: tileColor("status", row) }}
                      title={`status: ${statusLabel}`}
                    />
                    <span className="text-zinc-400">{statusLabel}</span>
                  </span>
                </td>
                <td className="px-3 py-2">
                  {/* The real focusable control: one tab stop per row,
                      keyboard + assistive tech open the drawer here while
                      the whole-row onClick stays a mouse-only convenience.
                      Keeps <tr>/<td> table semantics intact rather than
                      overriding the row with role="button". */}
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      onSelect(h.id);
                    }}
                    title={label}
                    className="block max-w-[16rem] truncate rounded-sm text-left font-medium text-zinc-200 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-blue-500/70"
                  >
                    {label}
                  </button>
                </td>
                <td className="px-3 py-2 font-mono text-[11px] text-zinc-500">
                  {h.host}
                </td>
                <td className="px-3 py-2">
                  <UtilCell value={row?.cpu_util_pct ?? null} />
                </td>
                <td className="px-3 py-2 text-right font-mono tabular-nums text-zinc-400">
                  {gpuCount || "—"}
                </td>
                <td className="px-3 py-2">
                  <UtilCell value={row?.gpu_avg_util_pct ?? null} />
                </td>
                <td className="px-3 py-2 text-right font-mono tabular-nums">
                  {temp != null ? (
                    <span style={{ color: temperatureColor(temp) }}>
                      {temp.toFixed(0)}°C
                    </span>
                  ) : (
                    <span className="text-zinc-600">—</span>
                  )}
                </td>
                <td className="px-3 py-2 text-right">
                  {warnings > 0 ? (
                    <span className="inline-flex items-center gap-1 rounded-full bg-amber-500/15 px-1.5 py-0.5 font-mono text-[10px] text-amber-300">
                      <TriangleAlert className="size-3" />
                      {warnings}
                    </span>
                  ) : (
                    <span className="text-zinc-700">—</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
