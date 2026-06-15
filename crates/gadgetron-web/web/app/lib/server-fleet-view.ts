// Shared filtering / sorting / tile-coloring logic for the /web/servers
// views (ISSUE 48). Pure — vitest covers it without a DOM. Metric and
// status values come from the `server-fleet` action (one call per
// refresh regardless of fleet size); the card view's own 1 Hz stats
// polling stays untouched and is display-only.

export interface FleetHostRow {
  id: string;
  host: string;
  alias: string | null;
  online: boolean;
  cpu_util_pct: number | null;
  gpu_count: number;
  gpu_avg_util_pct: number | null;
  gpu_max_temp_c: number | null;
  warnings: number;
}

export type ServerStatusFilter = "all" | "online" | "offline" | "warn";
export type ServerSortKey = "name" | "cpu" | "gpu" | "temp" | "warn";
export type TileColorBy = "status" | "cpu" | "gpu" | "temp";

interface FilterableHost {
  id: string;
  host: string;
  alias?: string | null;
  gpus?: string[] | null;
}

/**
 * Filter + sort the inventory host list. `fleet` supplies status and
 * metric values; when it is EMPTY (legacy no-DB mode, or the fleet
 * call failed) the status filter is a no-op rather than hiding
 * everything, and metric sorts fall back to name order.
 */
export function filterSortHosts<H extends FilterableHost>(
  hosts: readonly H[],
  fleet: ReadonlyMap<string, FleetHostRow>,
  query: string,
  status: ServerStatusFilter,
  sortKey: ServerSortKey,
): H[] {
  const q = query.trim().toLowerCase();
  let out = hosts.filter((h) => {
    if (q) {
      const hay = `${h.alias ?? ""} ${h.host} ${(h.gpus ?? []).join(" ")}`
        .toLowerCase();
      if (!hay.includes(q)) return false;
    }
    if (status !== "all" && fleet.size > 0) {
      const row = fleet.get(h.id);
      if (status === "online" && !(row?.online ?? false)) return false;
      if (status === "offline" && (row?.online ?? false)) return false;
      if (status === "warn" && (row?.warnings ?? 0) === 0) return false;
    }
    return true;
  });

  const name = (h: FilterableHost) => (h.alias ?? h.host).toLowerCase();
  const metric = (h: FilterableHost): number => {
    const row = fleet.get(h.id);
    if (!row) return Number.NEGATIVE_INFINITY;
    const v =
      sortKey === "cpu"
        ? row.cpu_util_pct
        : sortKey === "gpu"
          ? row.gpu_avg_util_pct
          : sortKey === "warn"
            ? row.warnings
            : row.gpu_max_temp_c;
    return v ?? Number.NEGATIVE_INFINITY;
  };
  out = out.slice().sort((a, b) => {
    if (sortKey === "name" || fleet.size === 0) {
      return name(a).localeCompare(name(b));
    }
    const d = metric(b) - metric(a); // metric sorts: hottest/busiest first
    return d !== 0 ? d : name(a).localeCompare(name(b));
  });
  return out;
}

/** Unknown / no-data tile — light gray so dark text stays readable. */
const TILE_UNKNOWN = "#a1a1aa";

export function utilizationColor(pct: number | null): string {
  if (pct == null) return TILE_UNKNOWN;
  if (pct < 50) return "#22c55e";
  if (pct < 75) return "#eab308";
  if (pct < 90) return "#f97316";
  return "#ef4444";
}

export function temperatureColor(celsius: number | null): string {
  if (celsius == null) return TILE_UNKNOWN;
  if (celsius < 60) return "#22c55e";
  if (celsius < 75) return "#eab308";
  if (celsius < 85) return "#f97316";
  return "#ef4444";
}

/**
 * Background color for one tile under the selected color-by mode
 * (Datadog host-map convention: green = fine, red = needs eyes).
 * Status mode: offline beats warnings beats online.
 */
export function tileColor(
  colorBy: TileColorBy,
  row: FleetHostRow | undefined,
): string {
  if (!row) return TILE_UNKNOWN;
  switch (colorBy) {
    case "status":
      if (!row.online) return "#dc2626";
      if (row.warnings > 0) return "#eab308";
      return "#22c55e";
    case "cpu":
      return utilizationColor(row.online ? row.cpu_util_pct : null);
    case "gpu":
      return utilizationColor(row.online ? row.gpu_avg_util_pct : null);
    case "temp":
      return temperatureColor(row.online ? row.gpu_max_temp_c : null);
  }
}

/** Short value rendered inside the tile under the host name. */
export function tileMetricLabel(
  colorBy: TileColorBy,
  row: FleetHostRow | undefined,
): string {
  if (!row) return "—";
  switch (colorBy) {
    case "status":
      return row.online
        ? row.warnings > 0
          ? `⚠${row.warnings}`
          : "online"
        : "offline";
    case "cpu":
      return row.cpu_util_pct != null
        ? `${row.cpu_util_pct.toFixed(0)}%`
        : "—";
    case "gpu":
      return row.gpu_avg_util_pct != null
        ? `${row.gpu_avg_util_pct.toFixed(0)}%`
        : "—";
    case "temp":
      return row.gpu_max_temp_c != null
        ? `${row.gpu_max_temp_c.toFixed(0)}°C`
        : "—";
  }
}
