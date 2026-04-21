"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
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

interface MetricChoice {
  metric: string;
  label: string;
  unit?: string;
  /** Color for the line/area. */
  tone: string;
  /** y-axis lower bound. `undefined` → autoscale. */
  yMin?: number;
  /** y-axis upper bound. `undefined` → autoscale. */
  yMax?: number;
  /** Optional projection onto display value (e.g. bytes → percent). */
  transform?: (v: number, ctx: HostDetailContext) => number;
  /** Display formatter for the tooltip / axis. */
  fmt?: (v: number) => string;
}

export interface HostDetailContext {
  totalRamBytes?: number;
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

function defaultMetrics(
  available: { gpus: Array<{ index: number; name: string }>; nics: string[]; temps: string[] },
): MetricChoice[] {
  const out: MetricChoice[] = [
    { metric: "cpu.util", label: "CPU util", unit: "%", tone: "#60a5fa", yMin: 0, yMax: 100, fmt: pctFmt },
    {
      metric: "mem.used_bytes",
      label: "Memory used",
      unit: "%",
      tone: "#34d399",
      yMin: 0,
      yMax: 100,
      transform: (v, ctx) =>
        ctx.totalRamBytes && ctx.totalRamBytes > 0 ? (v / ctx.totalRamBytes) * 100 : v,
      fmt: pctFmt,
    },
  ];
  for (const g of available.gpus) {
    out.push({
      metric: `gpu.${g.index}.util`,
      label: `GPU ${g.index} util`,
      unit: "%",
      tone: "#fbbf24",
      yMin: 0,
      yMax: 100,
      fmt: pctFmt,
    });
    out.push({
      metric: `gpu.${g.index}.temp`,
      label: `GPU ${g.index} temp`,
      unit: "°C",
      tone: "#f87171",
      fmt: celsiusFmt,
    });
    out.push({
      metric: `gpu.${g.index}.power_w`,
      label: `GPU ${g.index} power`,
      unit: "W",
      tone: "#a78bfa",
      fmt: wattsFmt,
    });
  }
  for (const iface of available.nics) {
    out.push({
      metric: `nic.${iface}.rx_bps`,
      label: `${iface} rx`,
      unit: "B/s",
      tone: "#22d3ee",
      fmt: bpsFmt,
    });
    out.push({
      metric: `nic.${iface}.tx_bps`,
      label: `${iface} tx`,
      unit: "B/s",
      tone: "#94a3b8",
      fmt: bpsFmt,
    });
  }
  if (available.temps.length > 0) {
    // Just chart the first temp sensor by default — the rest are
    // available via raw API for someone who wants them.
    out.push({
      metric: available.temps[0],
      label: available.temps[0].replace(/^temp\./, ""),
      unit: "°C",
      tone: "#fb923c",
      fmt: celsiusFmt,
    });
  }
  out.push({
    metric: "power.gpu_watts",
    label: "GPU power total",
    unit: "W",
    tone: "#c084fc",
    fmt: wattsFmt,
  });
  return out;
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
  available: { gpus: Array<{ index: number; name: string }>; nics: string[]; temps: string[] };
  context: HostDetailContext;
}) {
  const [rangeMs, setRangeMs] = useState<number>(RANGES[0].ms);
  const choices = useMemo(() => defaultMetrics(available), [available]);
  const [enabled, setEnabled] = useState<Set<string>>(
    () => new Set(choices.slice(0, 4).map((c) => c.metric)),
  );
  const [series, setSeries] = useState<Record<string, ApiResponse>>({});
  const [loading, setLoading] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  const fetchSeries = useCallback(async () => {
    if (!apiKey || !open) return;
    setLoading(true);
    setErrorMsg(null);
    try {
      const to = new Date();
      const from = new Date(to.getTime() - rangeMs);
      const want = choices.filter((c) => enabled.has(c.metric));
      const results = await Promise.all(
        want.map(async (c) => {
          const url =
            `${getApiBase()}/workbench/servers/${hostId}/metrics` +
            `?metric=${encodeURIComponent(c.metric)}` +
            `&from=${from.toISOString()}` +
            `&to=${to.toISOString()}` +
            `&bucket=auto`;
          const res = await fetch(url, {
            headers: { Authorization: `Bearer ${apiKey}` },
          });
          if (!res.ok) {
            throw new Error(`${c.metric}: ${res.status}`);
          }
          const body = (await res.json()) as ApiResponse;
          return [c.metric, body] as const;
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
  }, [apiKey, open, hostId, rangeMs, choices, enabled]);

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

  const toggleMetric = (m: string) => {
    setEnabled((prev) => {
      const next = new Set(prev);
      if (next.has(m)) next.delete(m);
      else next.add(m);
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
              disabled={loading}
              className="h-6 px-2 text-[11px]"
            >
              {loading ? "…" : "Refresh"}
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

        {/* Metric toggles */}
        <div
          className="flex flex-wrap gap-1 border-b border-zinc-800 px-4 py-2"
          aria-label="metrics"
        >
          {choices.map((c) => {
            const on = enabled.has(c.metric);
            return (
              <button
                key={c.metric}
                type="button"
                onClick={() => toggleMetric(c.metric)}
                className={`rounded border px-1.5 py-0.5 text-[10px] ${
                  on
                    ? "border-zinc-600 bg-zinc-800 text-zinc-200"
                    : "border-zinc-800 bg-zinc-950 text-zinc-600 hover:text-zinc-400"
                }`}
                style={on ? { borderColor: c.tone } : undefined}
                data-testid={`metric-toggle-${c.metric}`}
              >
                {c.label}
              </button>
            );
          })}
        </div>

        {/* Charts */}
        <div className="flex-1 space-y-4 p-4">
          {errorMsg && (
            <div className="rounded border border-red-900/60 bg-red-950/40 px-3 py-2 text-[11px] text-red-300">
              {errorMsg}
            </div>
          )}
          {choices
            .filter((c) => enabled.has(c.metric))
            .map((c) => {
              const body = series[c.metric];
              const data =
                body?.points.map((p) => ({
                  ts: new Date(p.ts).getTime(),
                  avg: c.transform ? c.transform(p.avg, context) : p.avg,
                  min: c.transform ? c.transform(p.min, context) : p.min,
                  max: c.transform ? c.transform(p.max, context) : p.max,
                })) ?? [];
              const fmt = c.fmt ?? ((v: number) => v.toFixed(2));
              return (
                <section
                  key={c.metric}
                  data-testid={`detail-chart-${c.metric}`}
                  className="rounded border border-zinc-800 bg-zinc-900 p-3"
                >
                  <div className="mb-2 flex items-center justify-between text-[11px]">
                    <span className="font-mono text-zinc-300">{c.label}</span>
                    <span className="text-zinc-600">
                      {body
                        ? `${data.length} pts · ${body.resolution}${
                            body.refresh_lag_seconds > 0
                              ? ` · lag ${body.refresh_lag_seconds}s`
                              : ""
                          }`
                        : "loading…"}
                    </span>
                  </div>
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
                          domain={[c.yMin ?? "auto", c.yMax ?? "auto"]}
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
                          formatter={(v) => [
                            typeof v === "number" ? fmt(v) : String(v),
                            c.label,
                          ]}
                        />
                        <Area
                          type="monotone"
                          dataKey="max"
                          stroke="none"
                          fill={c.tone}
                          fillOpacity={0.08}
                          isAnimationActive={false}
                        />
                        <Area
                          type="monotone"
                          dataKey="min"
                          stroke="none"
                          fill={c.tone}
                          fillOpacity={0.06}
                          isAnimationActive={false}
                        />
                        <Line
                          type="monotone"
                          dataKey="avg"
                          stroke={c.tone}
                          strokeWidth={1.5}
                          dot={false}
                          isAnimationActive={false}
                        />
                      </ComposedChart>
                    </ResponsiveContainer>
                  </div>
                </section>
              );
            })}
          {enabled.size === 0 && (
            <div className="text-center text-[11px] text-zinc-600">
              No metrics selected. Toggle one of the chips above to draw a chart.
            </div>
          )}
        </div>
      </aside>
    </div>
  );
}
