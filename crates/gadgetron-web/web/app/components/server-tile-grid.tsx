"use client";

// Datadog-host-map-style status tiles for /web/servers (ISSUE 48).
// One small colored tile per server — color = the selected metric
// (status / CPU / GPU util / hottest GPU), amber ring = warnings.
// Scales to hundreds of hosts where the cards cannot; detail lives a
// click away in the existing HostDetailDrawer.

import {
  tileColor,
  tileMetricLabel,
  type FleetHostRow,
  type TileColorBy,
} from "../lib/server-fleet-view";

const COLOR_BY_OPTIONS: Array<{ key: TileColorBy; label: string }> = [
  { key: "status", label: "Status" },
  { key: "cpu", label: "CPU" },
  { key: "gpu", label: "GPU util" },
  { key: "temp", label: "GPU temp" },
];

export function ServerTileGrid({
  hosts,
  fleet,
  colorBy,
  onColorByChange,
  onSelect,
}: {
  hosts: Array<{ id: string; host: string; alias?: string | null }>;
  fleet: ReadonlyMap<string, FleetHostRow>;
  colorBy: TileColorBy;
  onColorByChange: (next: TileColorBy) => void;
  onSelect: (hostId: string) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div
        className="flex flex-wrap items-center gap-1.5"
        data-testid="server-tile-colorby"
      >
        <span className="text-[11px] text-zinc-500">Color by</span>
        {COLOR_BY_OPTIONS.map((opt) => (
          <button
            key={opt.key}
            type="button"
            onClick={() => onColorByChange(opt.key)}
            aria-pressed={colorBy === opt.key}
            className={`rounded-full border px-2.5 py-0.5 text-[11px] transition-colors ${
              colorBy === opt.key
                ? "border-blue-700 bg-blue-950/40 text-blue-300"
                : "border-zinc-700 text-zinc-400 hover:text-zinc-200"
            }`}
          >
            {opt.label}
          </button>
        ))}
      </div>
      {hosts.length === 0 ? (
        <div
          className="surface-1 rounded-lg p-6 text-center text-xs text-zinc-500"
          data-testid="server-tiles-empty"
        >
          No servers match the current filters.
        </div>
      ) : (
        <div className="flex flex-wrap gap-1.5" data-testid="server-tile-grid">
          {hosts.map((h) => {
            const row = fleet.get(h.id);
            const label = h.alias ?? h.host;
            return (
              <button
                key={h.id}
                type="button"
                data-testid="server-tile"
                onClick={() => onSelect(h.id)}
                title={[
                  label,
                  row
                    ? row.online
                      ? `CPU ${row.cpu_util_pct != null ? `${row.cpu_util_pct.toFixed(0)}%` : "—"}`
                      : "offline"
                    : "no data",
                  row && row.gpu_count > 0
                    ? `GPU×${row.gpu_count}${
                        row.gpu_avg_util_pct != null
                          ? ` ${row.gpu_avg_util_pct.toFixed(0)}%`
                          : ""
                      }${
                        row.gpu_max_temp_c != null
                          ? ` · ${row.gpu_max_temp_c.toFixed(0)}°C`
                          : ""
                      }`
                    : null,
                  row && row.warnings > 0 ? `⚠ ${row.warnings}` : null,
                ]
                  .filter(Boolean)
                  .join(" · ")}
                style={{ backgroundColor: tileColor(colorBy, row) }}
                className={`flex h-12 w-24 flex-col items-center justify-center rounded-sm p-1 text-center transition-transform hover:scale-105 ${
                  row && row.warnings > 0 ? "ring-1 ring-amber-300" : ""
                }`}
              >
                <span className="w-full truncate text-[10px] font-bold text-zinc-950">
                  {label}
                </span>
                <span className="text-[9px] font-mono text-zinc-900">
                  {tileMetricLabel(colorBy, row)}
                </span>
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}
