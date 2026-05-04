"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Area,
  CartesianGrid,
  ComposedChart,
  Line,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { counterToRollingRate } from "../lib/metric-series";
import { Button } from "./ui/button";

// Drawer (full-height right panel) opened when an operator clicks a
// host card on /web/servers. Lets them pick a time range, browse a
// curated set of metrics, and see avg/min/max overlays for each.
//
// All requests go through the same `/api/v1/web/workbench/servers/{id}/metrics`
// endpoint that the sparklines use — `bucket=auto` picks a sensible
// resolution per range.

interface ApiPoint {
  ts: string;
  avg: number;
  min: number;
  max: number;
  samples: number;
}

interface ApiResponse {
  metric: string;
  unit: string | null;
  resolution: string;
  points: ApiPoint[];
  refresh_lag_seconds: number;
  dropped_frames: number;
}

interface RangeOption {
  label: string;
  ms: number;
}

const RANGES: RangeOption[] = [
  { label: "5m", ms: 5 * 60 * 1000 },
  { label: "1h", ms: 60 * 60 * 1000 },
  { label: "6h", ms: 6 * 60 * 60 * 1000 },
  { label: "24h", ms: 24 * 60 * 60 * 1000 },
  { label: "7d", ms: 7 * 24 * 60 * 60 * 1000 },
];

function getApiBase(): string {
  if (typeof document === "undefined") return "/api/v1/web";
  const meta = document.querySelector<HTMLMetaElement>(
    'meta[name="gadgetron-api-base"]',
  );
  const chatBase = meta?.content || "/v1";
  return chatBase.replace(/\/v1$/, "/api/v1/web");
}

/** A single series drawn as one line on the chart. */
interface SeriesSpec {
  metric: string;
  sourceMetric?: string;
  derive?: "counter_rate";
  /** Legend label (e.g. "GPU 0" when the metric is `gpu.0.util`). */
  label: string;
  /** Hex color for the line/area. */
  tone: string;
}

/** One chart = one category (CPU / GPU util / NIC rx / …). Renders
 *  one line per member so multi-device hosts fit on a single chart. */
interface MetricGroup {
  id: string;
  label: string;
  unit?: string;
  yMin?: number;
  yMax?: number;
  series: SeriesSpec[];
  /** Optional projection (e.g. bytes → percent) applied per point. */
  transform?: (v: number, ctx: HostDetailContext) => number;
  /** Display formatter for axis + tooltip. */
  fmt?: (v: number) => string;
}

export interface HostDetailContext {
  totalRamBytes?: number;
}

// Palette cycled across series inside a group — keeps GPU 0 / 1 / 2
// visually distinct without the developer picking colors by hand.
const SERIES_PALETTE = [
  "#60a5fa", // blue
  "#fbbf24", // amber
  "#34d399", // emerald
  "#f87171", // red
  "#a78bfa", // violet
  "#22d3ee", // cyan
  "#fb923c", // orange
  "#c084fc", // purple
];

function decodeThrottleShort(v: number): string {
  // DCGM throttle-reasons bitmask → short axis label. Ordered by
  // severity so the worst cause wins when multiple bits are set.
  if (!v) return "ok";
  const n = Math.round(v);
  if (n & 0x40) return "HW-thermal";
  if (n & 0x80) return "HW-power";
  if (n & 0x08) return "HW-slow";
  if (n & 0x20) return "SW-thermal";
  if (n & 0x04) return "SW-power";
  if (n & 0x02) return "app-clock";
  return `0x${n.toString(16)}`;
}

function pctFmt(v: number): string {
  return `${v.toFixed(1)}%`;
}
function bytesFmt(v: number): string {
  if (v < 1024) return `${v.toFixed(0)} B`;
  if (v < 1024 ** 2) return `${(v / 1024).toFixed(1)} KiB`;
  if (v < 1024 ** 3) return `${(v / 1024 ** 2).toFixed(1)} MiB`;
  if (v < 1024 ** 4) return `${(v / 1024 ** 3).toFixed(1)} GiB`;
  return `${(v / 1024 ** 4).toFixed(1)} TiB`;
}
function bpsFmt(v: number): string {
  if (v < 1024) return `${v.toFixed(0)} B/s`;
  if (v < 1024 ** 2) return `${(v / 1024).toFixed(1)} KiB/s`;
  if (v < 1024 ** 3) return `${(v / 1024 ** 2).toFixed(1)} MiB/s`;
  return `${(v / 1024 ** 3).toFixed(2)} GiB/s`;
}
function celsiusFmt(v: number): string {
  return `${v.toFixed(1)}°C`;
}
function wattsFmt(v: number): string {
  return `${v.toFixed(0)} W`;
}

function defaultGroups(
  available: {
    gpus: Array<{ index: number; name: string }>;
    nics: string[];
    temps: string[];
    cooling?: boolean;
  },
): MetricGroup[] {
  const groups: MetricGroup[] = [
    {
      id: "cpu",
      label: "CPU util",
      unit: "%",
      yMin: 0,
      yMax: 100,
      fmt: pctFmt,
      series: [{ metric: "cpu.util", label: "cpu", tone: SERIES_PALETTE[0] }],
    },
    {
      id: "mem",
      label: "Memory used",
      unit: "%",
      yMin: 0,
      yMax: 100,
      fmt: pctFmt,
      transform: (v, ctx) =>
        ctx.totalRamBytes && ctx.totalRamBytes > 0 ? (v / ctx.totalRamBytes) * 100 : v,
      series: [{ metric: "mem.used_bytes", label: "mem", tone: SERIES_PALETTE[2] }],
    },
  ];

  // GPUs — one chart per metric family, one line per GPU index.
  if (available.gpus.length > 0) {
    groups.push({
      id: "gpu_util",
      label: "GPU util",
      unit: "%",
      yMin: 0,
      yMax: 100,
      fmt: pctFmt,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.util`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    groups.push({
      id: "gpu_temp",
      label: "GPU temperature",
      unit: "°C",
      fmt: celsiusFmt,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.temp`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    groups.push({
      id: "gpu_power",
      label: "GPU power",
      unit: "W",
      fmt: wattsFmt,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.power_w`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    // VRAM usage (MiB). DCGM emits this directly; nvidia-smi fallback
    // also fills it. Showing raw MiB is more useful than a percent
    // because operators often think "did the model fit in 80 GB?".
    groups.push({
      id: "gpu_mem_used",
      label: "GPU memory used",
      unit: "MiB",
      fmt: (v: number) =>
        v >= 1024 ? `${(v / 1024).toFixed(1)} GiB` : `${v.toFixed(0)} MiB`,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.mem_used_mib`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    // DCGM-only: HBM / memory temperature tracked separately from the
    // SM die temp. Useful for spotting thermal imbalance that the
    // single "temp" value hides.
    groups.push({
      id: "gpu_mem_temp",
      label: "GPU memory temp",
      unit: "°C",
      fmt: celsiusFmt,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.mem_temp`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    // Throttle bitmask — non-zero ⇒ GPU running below requested
    // clocks. Format decodes the bits into human labels so operators
    // read "HW thermal" not "0x40" on the Y axis. State-trace line;
    // default-off to keep clean chart view.
    groups.push({
      id: "gpu_throttle",
      label: "GPU throttle",
      unit: "",
      fmt: decodeThrottleShort,
      series: available.gpus.map((g, i) => ({
        metric: `gpu.${g.index}.throttle_bits`,
        label: `gpu${g.index}`,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
  }

  // NICs — one chart for rx across all interfaces, one for tx.
  if (available.nics.length > 0) {
    groups.push({
      id: "nic_rx",
      label: "NIC rx",
      unit: "B/s",
      fmt: bpsFmt,
      series: available.nics.map((iface, i) => ({
        metric: `nic.${iface}.rx_bps`,
        sourceMetric: `nic.${iface}.rx_bytes_total`,
        derive: "counter_rate",
        label: iface,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
    groups.push({
      id: "nic_tx",
      label: "NIC tx",
      unit: "B/s",
      fmt: bpsFmt,
      series: available.nics.map((iface, i) => ({
        metric: `nic.${iface}.tx_bps`,
        sourceMetric: `nic.${iface}.tx_bytes_total`,
        derive: "counter_rate",
        label: iface,
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
  }

  // Temperature sensors — one chart, one line per sensor. Limit to 6
  // so an unusually chatty board (~20 sensors) stays readable.
  if (available.temps.length > 0) {
    const picked = available.temps.slice(0, 6);
    groups.push({
      id: "temps",
      label: "Temperatures",
      unit: "°C",
      fmt: celsiusFmt,
      series: picked.map((metric, i) => ({
        metric,
        label: metric.replace(/^temp\./, ""),
        tone: SERIES_PALETTE[i % SERIES_PALETTE.length],
      })),
    });
  }

  if (available.cooling) {
    groups.push({
      id: "cooling",
      label: "Liquid cooling",
      unit: "°C",
      fmt: celsiusFmt,
      series: [
        {
          metric: "cooling.coolant_temp",
          label: "coolant",
          tone: SERIES_PALETTE[5],
        },
        {
          metric: "cooling.air_temp",
          label: "air",
          tone: SERIES_PALETTE[2],
        },
        {
          metric: "cooling.coolant_delta_t",
          label: "delta",
          tone: SERIES_PALETTE[6],
        },
      ],
    });
  }

  groups.push({
    id: "power",
    label: "Power",
    unit: "W",
    fmt: wattsFmt,
    series: [
      { metric: "power.gpu_watts", label: "gpu total", tone: SERIES_PALETTE[4] },
      { metric: "power.psu_watts", label: "psu", tone: SERIES_PALETTE[3] },
    ],
  });

  return groups;
}

export function HostDetailDrawer({
  open,
  onClose,
  apiKey,
  hostId,
  hostLabel,
  available,
  context,
}: {
  open: boolean;
  onClose: () => void;
  apiKey: string | null;
  hostId: string;
  hostLabel: string;
  available: {
    gpus: Array<{ index: number; name: string }>;
    nics: string[];
    temps: string[];
    cooling?: boolean;
  };
  context: HostDetailContext;
}) {
  const groups = useMemo(() => defaultGroups(available), [available]);
  // Persist per-host: each host has its own GPU count / NIC list, so
  // applying host-A's "GPU 2-7 hidden" toggle to host-B (which may have
  // a different GPU layout entirely) would be wrong.
  //
  // localStorage keys:
  //   gadgetron.host-detail.<hostId>.range-ms
  //   gadgetron.host-detail.<hostId>.enabled
  //   gadgetron.host-detail.<hostId>.hidden-series
  //
  // State starts at the defaults; the `hydrate` effect overwrites state
  // from localStorage whenever `hostId` changes. A `hydratedHostIdRef`
  // guard prevents the save-on-change effects from firing before the
  // load completes — otherwise the default state would clobber stored
  // values on the first render of a new host.
  const [rangeMs, setRangeMs] = useState<number>(RANGES[0].ms);
  const [enabled, setEnabled] = useState<Set<string>>(new Set());
  const [hiddenSeries, setHiddenSeries] = useState<Set<string>>(new Set());
  const hydratedHostIdRef = useRef<string | null>(null);

  const storageKey = useCallback(
    (kind: "range-ms" | "enabled" | "hidden-series") =>
      `gadgetron.host-detail.${hostId}.${kind}`,
    [hostId],
  );

  // Hydrate state from localStorage when the drawer switches hosts.
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (!hostId) return;

    const rangeRaw = window.localStorage.getItem(storageKey("range-ms"));
    const parsedRange = rangeRaw ? parseInt(rangeRaw, 10) : NaN;
    setRangeMs(
      Number.isFinite(parsedRange) && parsedRange > 0
        ? parsedRange
        : RANGES[0].ms,
    );

    const enabledRaw = window.localStorage.getItem(storageKey("enabled"));
    let nextEnabled: Set<string> | null = null;
    if (enabledRaw) {
      try {
        const parsed = JSON.parse(enabledRaw) as string[];
        if (Array.isArray(parsed) && parsed.length > 0) {
          nextEnabled = new Set(parsed);
        }
      } catch {
        // fall through to default
      }
    }
    setEnabled(
      nextEnabled ?? new Set(groups.slice(0, 4).map((g) => g.id)),
    );

    const hiddenRaw = window.localStorage.getItem(storageKey("hidden-series"));
    let nextHidden: Set<string> = new Set();
    if (hiddenRaw) {
      try {
        const parsed = JSON.parse(hiddenRaw) as string[];
        if (Array.isArray(parsed)) nextHidden = new Set(parsed);
      } catch {
        // fall through to default
      }
    }
    setHiddenSeries(nextHidden);

    hydratedHostIdRef.current = hostId;
  }, [hostId, groups, storageKey]);

  // Save effects — skip until the current host has been hydrated to
  // prevent the initial default state from overwriting stored values.
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (hydratedHostIdRef.current !== hostId) return;
    window.localStorage.setItem(storageKey("range-ms"), String(rangeMs));
  }, [hostId, rangeMs, storageKey]);
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (hydratedHostIdRef.current !== hostId) return;
    window.localStorage.setItem(
      storageKey("enabled"),
      JSON.stringify(Array.from(enabled)),
    );
  }, [hostId, enabled, storageKey]);
  useEffect(() => {
    if (typeof window === "undefined") return;
    if (hydratedHostIdRef.current !== hostId) return;
    window.localStorage.setItem(
      storageKey("hidden-series"),
      JSON.stringify(Array.from(hiddenSeries)),
    );
  }, [hostId, hiddenSeries, storageKey]);
  const [series, setSeries] = useState<Record<string, ApiResponse>>({});
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const fetchSeries = useCallback(async () => {
    if (!open) return;
    setLoading(true);
    setErrorMsg(null);
    try {
      const to = new Date();
      const from = new Date(to.getTime() - rangeMs);
      // Flatten every member of every enabled group into one parallel
      // fetch plan. Duplicates are naturally deduped by the final
      // Record keying on metric name.
      const wantMetrics = new Set<string>();
      for (const g of groups) {
        if (!enabled.has(g.id)) continue;
        for (const s of g.series) {
          if (hiddenSeries.has(s.metric)) continue;
          wantMetrics.add(s.sourceMetric ?? s.metric);
        }
      }
      const results = await Promise.all(
        Array.from(wantMetrics).map(async (metric) => {
          const url =
            `${getApiBase()}/workbench/servers/${hostId}/metrics` +
            `?metric=${encodeURIComponent(metric)}` +
            `&from=${from.toISOString()}` +
            `&to=${to.toISOString()}` +
            `&bucket=auto`;
          const res = await fetch(url, {
            credentials: "include",
            headers: apiKey ? { Authorization: `Bearer ${apiKey}` } : {},
          });
          if (!res.ok) throw new Error(`${metric}: ${res.status}`);
          const body = (await res.json()) as ApiResponse;
          return [metric, body] as const;
        }),
      );
      const next: Record<string, ApiResponse> = {};
      for (const [m, b] of results) next[m] = b;
      setSeries(next);
    } catch (e) {
      setErrorMsg((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [apiKey, open, hostId, rangeMs, groups, enabled, hiddenSeries]);

  // Initial + refresh-on-range-change.
  useEffect(() => {
    void fetchSeries();
  }, [fetchSeries]);

  // Auto-refresh every 5 s while open.
  useEffect(() => {
    if (!open) return;
    const id = window.setInterval(fetchSeries, 5_000);
    return () => window.clearInterval(id);
  }, [open, fetchSeries]);

  if (!open) return null;

  const toggleGroup = (id: string) => {
    setEnabled((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleSeries = (metric: string) => {
    setHiddenSeries((prev) => {
      const next = new Set(prev);
      if (next.has(metric)) next.delete(metric);
      else next.add(metric);
      return next;
    });
  };

  return (
    <div
      className="fixed inset-0 z-50 flex"
      data-testid="host-detail-drawer"
      role="dialog"
      aria-label={`Host detail: ${hostLabel}`}
    >
      <div
        className="grow bg-black/60"
        onClick={onClose}
        aria-hidden
      />
      <aside className="flex h-full w-[min(720px,90vw)] flex-col overflow-y-auto border-l border-zinc-800 bg-zinc-950">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-3">
          <div className="flex items-center gap-3">
            <span className="font-mono text-sm text-zinc-200">{hostLabel}</span>
            <span className="font-mono text-[10px] text-zinc-600">{hostId.slice(0, 8)}</span>
          </div>
          <div className="flex items-center gap-1">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void fetchSeries()}
              className="h-6 px-2 text-[11px]"
              title={loading ? "Refreshing…" : "Refresh now"}
            >
              {/* Keep the label stable so 5-s auto-refresh doesn't
                * flash between "…" and "Refresh". A small dot signals
                * activity without swapping the text. */}
              <span className="flex items-center gap-1.5">
                <span
                  aria-hidden
                  className={`inline-block size-1 rounded-full transition-opacity ${
                    loading ? "bg-emerald-500 opacity-100" : "opacity-0"
                  }`}
                />
                Refresh
              </span>
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={onClose}
              data-testid="host-detail-close"
              className="h-6 px-2 text-[11px]"
            >
              Close ✕
            </Button>
          </div>
        </div>

        {/* Range picker */}
        <div
          className="flex items-center gap-1 border-b border-zinc-800 px-4 py-2"
          role="tablist"
          aria-label="time range"
        >
          {RANGES.map((r) => (
            <button
              key={r.label}
              type="button"
              role="tab"
              aria-selected={rangeMs === r.ms}
              onClick={() => setRangeMs(r.ms)}
              data-testid={`range-${r.label}`}
              className={`rounded border px-2 py-0.5 text-[11px] ${
                rangeMs === r.ms
                  ? "border-blue-500 bg-blue-950/40 text-blue-200"
                  : "border-zinc-700 bg-zinc-900 text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {r.label}
            </button>
          ))}
          <span className="ml-2 text-[10px] text-zinc-600">
            auto-tier; refresh 5s
          </span>
        </div>

        {/* Group toggles — one chip per category. A group containing
         *  multiple series (e.g. 4 GPUs) renders as one chart with 4
         *  lines overlaid, not four separate charts. */}
        <div
          className="flex flex-wrap gap-1 border-b border-zinc-800 px-4 py-2"
          aria-label="metric groups"
        >
          {groups.map((g) => {
            const on = enabled.has(g.id);
            return (
              <button
                key={g.id}
                type="button"
                onClick={() => toggleGroup(g.id)}
                className={`rounded border px-1.5 py-0.5 text-[10px] ${
                  on
                    ? "border-zinc-600 bg-zinc-800 text-zinc-200"
                    : "border-zinc-800 bg-zinc-950 text-zinc-600 hover:text-zinc-400"
                }`}
                data-testid={`group-toggle-${g.id}`}
              >
                {g.label}
                {g.series.length > 1 && (
                  <span className="ml-1 text-zinc-500">×{g.series.length}</span>
                )}
              </button>
            );
          })}
        </div>

        {/* Charts — one per enabled group, multi-line when the group
         *  has several series. Merge all series of a group onto a
         *  single `ComposedChart` by joining per-timestamp rows, so
         *  recharts can render them stacked in the legend. */}
        <div className="flex-1 space-y-4 p-4">
          {errorMsg && (
            <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
              {errorMsg}
            </div>
          )}
          {groups
            .filter((g) => enabled.has(g.id))
            .map((g) => {
              // Join each series' points onto a single {ts, <seriesKey>:avg}
              // row. Different series may have slightly different ts
              // samples; the join below gives a row per *union* of all
              // timestamps seen, with undefined for series that didn't
              // sample at that instant (recharts just omits that dot).
              const allTs = new Set<number>();
              const perSeries: Record<string, Map<number, number>> = {};
              let metaResolution: string | undefined;
              let metaLag = 0;
              for (const s of g.series) {
                const body = series[s.sourceMetric ?? s.metric];
                if (!body) continue;
                metaResolution = body.resolution;
                metaLag = Math.max(metaLag, body.refresh_lag_seconds);
                const bucket = new Map<number, number>();
                const points =
                  s.derive === "counter_rate"
                    ? counterToRollingRate(body.points)
                    : body.points;
                for (const p of points) {
                  const ts = new Date(p.ts).getTime();
                  const v = g.transform
                    ? g.transform(p.avg, context)
                    : p.avg;
                  bucket.set(ts, v);
                  allTs.add(ts);
                }
                perSeries[s.metric] = bucket;
              }
              const data = Array.from(allTs)
                .sort((a, b) => a - b)
                .map((ts) => {
                  const row: Record<string, number | null> = { ts };
                  for (const s of g.series) {
                    const v = perSeries[s.metric]?.get(ts);
                    row[s.metric] = v == null ? null : v;
                  }
                  return row;
                });
              const fmt = g.fmt ?? ((v: number) => v.toFixed(2));
              const hasAnyData = Object.values(perSeries).some(
                (m) => m.size > 0,
              );
              return (
                <section
                  key={g.id}
                  data-testid={`detail-chart-${g.id}`}
                  className="rounded border border-zinc-800 bg-zinc-900 p-3"
                >
                  {/* Title + meta on one row. Series toggles go on a
                    * SEPARATE row below when there's more than one
                    * series — otherwise the meta text on the right
                    * (which changes width every poll as `pts`/`lag`
                    * update) fights for horizontal space with the
                    * flex-wrap toggle pills and visibly oscillates
                    * the chip row between 1 and 2 lines when there
                    * are many GPUs. Separating the rows kills that
                    * layout thrash. */}
                  <div className="mb-1 flex items-center justify-between text-[11px]">
                    <span className="font-mono text-zinc-300">{g.label}</span>
                    <span className="text-zinc-600">
                      {hasAnyData && metaResolution
                        ? `${data.length} pts · ${metaResolution}${
                            metaLag > 0 ? ` · lag ${metaLag}s` : ""
                          }`
                        : "loading…"}
                    </span>
                  </div>
                  {g.series.length > 1 && (
                    <div className="mb-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px]">
                      {g.series.map((s) => {
                        const hidden = hiddenSeries.has(s.metric);
                        return (
                          <button
                            key={s.metric}
                            type="button"
                            onClick={() => toggleSeries(s.metric)}
                            aria-pressed={!hidden}
                            data-testid={`series-toggle-${s.metric}`}
                            className={`flex items-center gap-1 rounded px-1 ${
                              hidden
                                ? "text-zinc-600 line-through"
                                : "text-zinc-400 hover:text-zinc-200"
                            }`}
                            title={hidden ? "Click to show" : "Click to hide"}
                          >
                            <span
                              className="inline-block size-2 rounded-sm"
                              style={{
                                background: hidden ? "transparent" : s.tone,
                                border: `1px solid ${s.tone}`,
                              }}
                              aria-hidden
                            />
                            {s.label}
                          </button>
                        );
                      })}
                    </div>
                  )}
                  <div style={{ width: "100%", height: 160 }}>
                    <ResponsiveContainer>
                      <ComposedChart
                        data={data}
                        margin={{ top: 4, right: 12, left: 4, bottom: 4 }}
                      >
                        <CartesianGrid strokeDasharray="3 3" stroke="#27272a" />
                        <XAxis
                          dataKey="ts"
                          type="number"
                          domain={["dataMin", "dataMax"]}
                          tick={{ fill: "#71717a", fontSize: 10 }}
                          tickFormatter={(t) =>
                            new Date(t).toLocaleTimeString([], {
                              hour: "2-digit",
                              minute: "2-digit",
                              second: "2-digit",
                            })
                          }
                          stroke="#3f3f46"
                        />
                        <YAxis
                          domain={[g.yMin ?? "auto", g.yMax ?? "auto"]}
                          tick={{ fill: "#71717a", fontSize: 10 }}
                          tickFormatter={(v) => fmt(v)}
                          stroke="#3f3f46"
                          width={70}
                        />
                        <Tooltip
                          contentStyle={{
                            background: "#18181b",
                            border: "1px solid #3f3f46",
                            borderRadius: 4,
                            fontSize: 11,
                          }}
                          labelFormatter={(t) =>
                            new Date(t as number).toLocaleString()
                          }
                          formatter={(v, name) => {
                            const label = g.series.find(
                              (s) => s.metric === name,
                            )?.label ?? String(name);
                            return [
                              typeof v === "number" ? fmt(v) : String(v),
                              label,
                            ];
                          }}
                        />
                        {g.series
                          .filter((s) => !hiddenSeries.has(s.metric))
                          .map((s) => (
                            <Line
                              key={s.metric}
                              type="monotone"
                              dataKey={s.metric}
                              stroke={s.tone}
                              strokeWidth={1.5}
                              dot={false}
                              connectNulls
                              isAnimationActive={false}
                            />
                          ))}
                      </ComposedChart>
                    </ResponsiveContainer>
                  </div>
                </section>
              );
            })}
          {enabled.size === 0 && (
            <div className="text-center text-[11px] text-zinc-600">
              No categories selected. Toggle one of the chips above to draw a chart.
            </div>
          )}
        </div>
      </aside>
    </div>
  );
}
