"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { seriesColor } from "@/lib/chart-palette";
import { cn } from "@/lib/utils";
import { useI18n, type Dictionary } from "../../lib/i18n";
import { invokeAction, unwrapPayload } from "../../lib/workbench-client";
import { Button } from "../ui/button";
import { Card, CardContent } from "../ui/card";
import { EmptyState } from "./empty-state";
import { InteractiveTimeseriesRenderer } from "./interactive-timeseries-renderer";

type JsonRecord = Record<string, unknown>;

interface TelemetryMetric {
  targetId: string;
  targetLabel: string;
  status: string;
  observedAt: string | null;
  metric: string;
  label: string;
  group: string;
  visual: "bar" | "gauge" | "number";
  value: number;
  unit: string;
  minimum: number;
  maximum: number | null;
  detail: string | null;
  labels: JsonRecord;
}

interface DeviceIdentity {
  familyKey: string;
  familyLabel: string;
  deviceKey: string;
  deviceLabel: string;
  measure: string;
  primaryMeasure: string;
  primaryLabel: string;
}

const MAX_ROWS = 500;
const MAX_TREND_POINTS = 60;
const LIVE_INTERVAL_MS = 3_000;

type TrendMap = Record<string, number[]>;
type LiveSampleMap = Record<string, LiveSample[]>;
type TelemetryCopy = Dictionary["telemetry"];
type HistoryRange = "live" | "5m" | "30m" | "24h" | "7d" | "30d";
type HistoryInterval = "auto" | "5s" | "1m" | "5m" | "15m" | "1h" | "6h" | "1d";

interface MetricChoice {
  key: string;
  metric: string;
  labels: JsonRecord;
  label: string;
  detail: string | null;
}

interface LiveSample {
  ts: string;
  value: number;
}

const HISTORY_RANGES: { value: HistoryRange; label: string }[] = [
  { value: "live", label: "Live" },
  { value: "5m", label: "5m" },
  { value: "30m", label: "30m" },
  { value: "24h", label: "24h" },
  { value: "7d", label: "7d" },
  { value: "30d", label: "30d" },
];

const RANGE_INTERVALS: Record<Exclude<HistoryRange, "live">, HistoryInterval[]> = {
  "5m": ["auto", "5s", "1m"],
  "30m": ["auto", "1m", "5m"],
  "24h": ["auto", "5m", "15m", "1h"],
  "7d": ["auto", "1h", "6h"],
  "30d": ["auto", "6h", "1d"],
};

function record(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonRecord
    : null;
}

function rows(payload: unknown): JsonRecord[] {
  const root = record(payload);
  const source = Array.isArray(payload)
    ? payload
    : Array.isArray(root?.rows)
      ? root.rows
      : [];
  return source.slice(0, MAX_ROWS).flatMap((value) => {
    const row = record(value);
    return row ? [row] : [];
  });
}

function finite(value: unknown): number | null {
  const number = typeof value === "number" ? value : Number(value);
  return Number.isFinite(number) ? number : null;
}

function parseMetric(row: JsonRecord): TelemetryMetric | null {
  const value = finite(row.latest ?? row.value);
  const presentation = record(row.presentation);
  if (value === null || !presentation) return null;
  const targetId = typeof row.target_id === "string" ? row.target_id : "unknown-target";
  const targetLabel = typeof row.target_label === "string" && row.target_label.trim()
    ? row.target_label
    : targetId;
  const metric = typeof row.metric === "string" ? row.metric : "unknown";
  const visual = presentation.visual;
  const maximum = finite(presentation.max);
  const minimum = finite(presentation.min) ?? 0;
  const labels = record(row.labels);
  const detail = typeof presentation.detail === "string"
    ? presentation.detail
    : labels
      ? Object.values(labels).filter((item) => typeof item === "string" || typeof item === "number").join(" · ")
      : null;
  return {
    targetId,
    targetLabel,
    status: typeof row.status === "string" ? row.status : "unknown",
    observedAt: typeof row.observed_at === "string" ? row.observed_at : null,
    metric,
    label: typeof presentation.label === "string" ? presentation.label : metric,
    group: typeof presentation.group === "string" ? presentation.group : "Other",
    visual: visual === "bar" || visual === "gauge" ? visual : "number",
    value,
    unit: typeof row.unit === "string" ? row.unit : "",
    minimum,
    maximum,
    detail: detail || null,
    labels: labels ?? {},
  };
}

function formatValue(value: number, unit: string): string {
  if (unit === "percent") return `${value.toFixed(1)}%`;
  if (unit === "celsius") return `${value.toFixed(1)} °C`;
  if (unit === "watts") return `${value.toFixed(1)} W`;
  if (unit === "bytes" || unit === "bytes_per_sec") {
    const units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let scaled = Math.abs(value);
    let index = 0;
    while (scaled >= 1024 && index < units.length - 1) {
      scaled /= 1024;
      index += 1;
    }
    const signed = value < 0 ? -scaled : scaled;
    return `${signed.toFixed(index === 0 ? 0 : 1)} ${units[index]}${unit === "bytes_per_sec" ? "/s" : ""}`;
  }
  if (unit === "mib") return `${value.toFixed(0)} MiB`;
  if (unit === "seconds") {
    const days = Math.floor(value / 86_400);
    const hours = Math.floor((value % 86_400) / 3_600);
    return days > 0 ? `${days}d ${hours}h` : `${hours}h`;
  }
  return `${Math.abs(value) >= 100 ? value.toFixed(0) : value.toFixed(2)}${unit ? ` ${unit}` : ""}`;
}

function ratio(metric: TelemetryMetric): number {
  if (metric.maximum === null || metric.maximum <= metric.minimum) return 0;
  return Math.min(1, Math.max(0, (metric.value - metric.minimum) / (metric.maximum - metric.minimum)));
}

function trendKey(metric: TelemetryMetric): string {
  return `${metric.targetId}:${metric.metric}:${metric.detail ?? ""}`;
}

function MiniTrend({ values, metric }: { values: number[]; metric: TelemetryMetric }) {
  if (values.length < 2) return <div className="mt-2 h-7 border-b border-zinc-800/80" aria-hidden="true" />;
  const fixedScale = metric.maximum !== null && metric.maximum > metric.minimum;
  const minimum = fixedScale ? metric.minimum : Math.min(...values);
  const maximum = fixedScale ? metric.maximum as number : Math.max(...values);
  const spread = maximum - minimum || 1;
  const points = values.map((value, index) => {
    const x = values.length === 1 ? 0 : (index / (values.length - 1)) * 100;
    const normalized = Math.min(1, Math.max(0, (value - minimum) / spread));
    const y = 26 - normalized * 22;
    return `${x.toFixed(2)},${y.toFixed(2)}`;
  }).join(" ");
  return (
    <svg
      viewBox="0 0 100 30"
      preserveAspectRatio="none"
      className="mt-2 h-7 w-full"
      role="img"
      aria-label={`${metric.label} recent live trend`}
      data-scale-mode={fixedScale ? "fixed" : "relative"}
      data-scale-min={minimum}
      data-scale-max={maximum}
    >
      <line x1="0" y1="28" x2="100" y2="28" stroke="#27272a" strokeWidth="1" />
      <polyline points={points} fill="none" stroke={seriesColor(0)} strokeWidth="1.8" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function MetricBar({ metric }: { metric: TelemetryMetric }) {
  const percent = ratio(metric) * 100;
  return (
    <div className="mt-3">
      <div
        className="relative h-2 overflow-hidden rounded-sm bg-zinc-800"
        role="progressbar"
        aria-label={metric.label}
        aria-valuemin={metric.minimum}
        aria-valuemax={metric.maximum ?? undefined}
        aria-valuenow={metric.value}
      >
        <div className="absolute inset-y-0 left-0 transition-[width] duration-200" style={{ width: `${percent}%`, backgroundColor: seriesColor(0) }} />
      </div>
    </div>
  );
}

function MetricGauge({ metric }: { metric: TelemetryMetric }) {
  const percent = ratio(metric) * 100;
  return (
    <svg viewBox="0 0 120 68" className="mt-1 h-16 w-full" role="img" aria-label={`${metric.label}: ${formatValue(metric.value, metric.unit)}`}>
      <path d="M 15 58 A 45 45 0 0 1 105 58" pathLength="100" fill="none" stroke="#27272a" strokeWidth="9" strokeLinecap="round" />
      <path d="M 15 58 A 45 45 0 0 1 105 58" pathLength="100" fill="none" stroke={seriesColor(0)} strokeWidth="9" strokeLinecap="round" strokeDasharray={`${percent} 100`} />
      <text x="60" y="52" textAnchor="middle" fill="#e4e4e7" fontSize="13" fontFamily="JetBrains Mono, monospace">{formatValue(metric.value, metric.unit)}</text>
    </svg>
  );
}

function MetricCard({ metric, trend }: { metric: TelemetryMetric; trend?: number[] }) {
  return (
    <Card className="border-zinc-800 bg-zinc-950/60">
      <CardContent className="p-3">
        <div className="text-xs font-semibold leading-4 text-zinc-400">{metric.label}</div>
        {metric.detail && <div className="mt-1 truncate text-[11px] text-zinc-500" title={metric.detail}>{metric.detail}</div>}
        {metric.visual !== "gauge" && <div className="mt-3 whitespace-nowrap font-mono text-lg text-zinc-100">{formatValue(metric.value, metric.unit)}</div>}
        {metric.visual === "bar" && <MetricBar metric={metric} />}
        {metric.visual === "gauge" && <MetricGauge metric={metric} />}
        {trend && <MiniTrend values={trend} metric={metric} />}
      </CardContent>
    </Card>
  );
}

function deviceIdentity(metric: TelemetryMetric): DeviceIdentity | null {
  const gpu = /^gpu\.(\d+)\.(.+)$/.exec(metric.metric);
  if (gpu) {
    return {
      familyKey: "gpu",
      familyLabel: "GPUs",
      deviceKey: gpu[1],
      deviceLabel: `GPU ${gpu[1]}`,
      measure: gpu[2],
      primaryMeasure: "util",
      primaryLabel: "GPU utilization",
    };
  }
  if (metric.metric.startsWith("disk.")) {
    const mount = typeof metric.labels.mount === "string" ? metric.labels.mount : metric.detail;
    if (mount) {
      return {
        familyKey: "disk",
        familyLabel: "Storage volumes",
        deviceKey: mount,
        deviceLabel: mount,
        measure: metric.metric.slice("disk.".length),
        primaryMeasure: "used_percent",
        primaryLabel: "Disk used",
      };
    }
  }
  if (metric.metric === "temp.celsius") {
    const chip = typeof metric.labels.chip === "string" ? metric.labels.chip : "sensor";
    const source = typeof metric.labels.source === "string" ? metric.labels.source : metric.detail;
    if (source) {
      return {
        familyKey: "temperature",
        familyLabel: "Temperature sensors",
        deviceKey: `${chip}:${source}`,
        deviceLabel: `${chip} · ${source}`,
        measure: "celsius",
        primaryMeasure: "celsius",
        primaryLabel: "Temperature",
      };
    }
  }
  const network = /^nic\.(.+)\.(rx_bps|tx_bps|rx_bytes_total|tx_bytes_total)$/.exec(metric.metric);
  if (network) {
    return {
      familyKey: "network-interface",
      familyLabel: "Network interfaces",
      deviceKey: network[1],
      deviceLabel: network[1],
      measure: network[2],
      primaryMeasure: "rx_bps",
      primaryLabel: "Receive throughput",
    };
  }
  return null;
}

function FamilyComparisonChart({
  familyLabel,
  primaryLabel,
  devices,
  copy,
}: {
  familyLabel: string;
  primaryLabel: string;
  devices: { label: string; metric: TelemetryMetric }[];
  copy: TelemetryCopy;
}) {
  const maximum = Math.max(1, ...devices.map(({ metric }) => metric.maximum ?? metric.value));
  const width = 1_000;
  const height = 240;
  const left = 58;
  const right = 20;
  const top = 30;
  const bottom = 38;
  const baseline = height - bottom;
  const plotHeight = baseline - top;
  const slot = (width - left - right) / devices.length;
  const barWidth = Math.min(72, slot * 0.58);
  return (
    <figure className="min-w-0 rounded border border-zinc-800 bg-[#101418] p-3">
      <figcaption className="mb-2">
        <div className="text-xs font-semibold text-zinc-300">{copy.currentComparison}</div>
        <div className="mt-1 text-[11px] text-zinc-500">{primaryLabel}</div>
      </figcaption>
      <div className="overflow-x-auto">
        <svg
          viewBox={`0 0 ${width} ${height}`}
          className="h-44 min-w-[560px] w-full"
          role="img"
          aria-label={copy.comparisonAria(primaryLabel, devices.length, familyLabel)}
          data-testid="family-comparison-chart"
        >
          {[maximum, maximum / 2, 0].map((tick) => {
            const y = baseline - (tick / maximum) * plotHeight;
            return (
              <g key={tick}>
                <line x1={left} x2={width - right} y1={y} y2={y} stroke="#273038" strokeWidth="1" />
                <text x={left - 9} y={y + 4} textAnchor="end" fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">
                  {formatValue(tick, devices[0].metric.unit)}
                </text>
              </g>
            );
          })}
          {devices.map(({ label, metric }, index) => {
            const scaledHeight = ratio(metric) * plotHeight;
            const barHeight = metric.value <= metric.minimum ? 0 : Math.max(2, scaledHeight);
            const x = left + slot * index + (slot - barWidth) / 2;
            const y = baseline - barHeight;
            const color = seriesColor(index);
            return (
              <g key={label}>
                <title>{`${label}: ${formatValue(metric.value, metric.unit)}`}</title>
                <rect
                  x={x}
                  y={y}
                  width={barWidth}
                  height={barHeight}
                  rx="2"
                  fill={color}
                  data-testid="gpu-comparison-bar"
                  data-series-label={label}
                  data-series-color={color}
                  data-series-value={metric.value}
                />
                <text x={x + barWidth / 2} y={baseline + 19} textAnchor="middle" fill="#89939A" fontSize="11" fontFamily="JetBrains Mono, monospace">
                  {label}
                </text>
              </g>
            );
          })}
          <line x1={left} x2={width - right} y1={baseline} y2={baseline} stroke="#6A757C" strokeWidth="1" />
        </svg>
      </div>
      <ul className="mt-2 flex flex-wrap gap-x-3 gap-y-1" aria-label={copy.seriesLegend}>
        {devices.map(({ label, metric }, index) => (
          <li key={label} className="flex items-center gap-1.5 text-[11px] text-zinc-400">
            <span className="h-2 w-2" style={{ backgroundColor: seriesColor(index) }} aria-hidden="true" />
            <span>{label}</span>
            <span className="font-mono text-zinc-500">{formatValue(metric.value, metric.unit)}</span>
          </li>
        ))}
      </ul>
    </figure>
  );
}

function FamilyTrendChart({
  primaryLabel,
  devices,
  samples,
  copy,
}: {
  primaryLabel: string;
  devices: { label: string; metric: TelemetryMetric }[];
  samples: LiveSampleMap;
  copy: TelemetryCopy;
}) {
  const width = 1_000;
  const height = 240;
  const left = 58;
  const right = 20;
  const top = 30;
  const bottom = 38;
  const baseline = height - bottom;
  const plotHeight = baseline - top;
  const series = devices.map(({ label, metric }, index) => ({
    label,
    metric,
    color: seriesColor(index),
    samples: samples[trendKey(metric)] ?? [],
  }));
  const timestamps = series.flatMap((item) => item.samples.map((sample) => Date.parse(sample.ts))).filter(Number.isFinite);
  const firstTimestamp = timestamps.length > 0 ? Math.min(...timestamps) : 0;
  const lastTimestamp = timestamps.length > 0 ? Math.max(...timestamps) : 0;
  const timeRange = lastTimestamp - firstTimestamp || 1;
  const yMinimum = Math.min(...devices.map(({ metric }) => metric.minimum));
  const observedMaximum = Math.max(yMinimum + 1, ...series.flatMap((item) => item.samples.map((sample) => sample.value)));
  const yMaximum = Math.max(observedMaximum, ...devices.map(({ metric }) => metric.maximum ?? observedMaximum));
  const valueRange = yMaximum - yMinimum || 1;
  const x = (timestamp: number) => left + ((timestamp - firstTimestamp) / timeRange) * (width - left - right);
  const y = (value: number) => baseline - Math.min(1, Math.max(0, (value - yMinimum) / valueRange)) * plotHeight;
  const hasTrend = series.some((item) => item.samples.length >= 2);

  return (
    <figure className="min-w-0 rounded border border-zinc-800 bg-[#101418] p-3" data-testid="gpu-live-trend">
      <figcaption className="mb-2">
        <div className="text-xs font-semibold text-zinc-300">{copy.recentLiveTrend}</div>
        <div className="mt-1 text-[11px] text-zinc-500">{copy.liveCadence}</div>
      </figcaption>
      {hasTrend ? (
        <div className="overflow-x-auto">
          <svg
            viewBox={`0 0 ${width} ${height}`}
            className="h-44 min-w-[560px] w-full"
            role="img"
            aria-label={copy.trendAria(primaryLabel, devices.length)}
            data-scale-min={yMinimum}
            data-scale-max={yMaximum}
          >
            {[yMaximum, (yMinimum + yMaximum) / 2, yMinimum].map((tick) => (
              <g key={tick}>
                <line x1={left} x2={width - right} y1={y(tick)} y2={y(tick)} stroke="#273038" strokeWidth="1" />
                <text x={left - 9} y={y(tick) + 4} textAnchor="end" fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">
                  {formatValue(tick, devices[0].metric.unit)}
                </text>
              </g>
            ))}
            {series.map((item) => {
              if (item.samples.length < 2) return null;
              const path = item.samples.map((sample, index) => {
                const timestamp = Date.parse(sample.ts);
                return `${index === 0 ? "M" : "L"}${x(timestamp).toFixed(2)},${y(sample.value).toFixed(2)}`;
              }).join(" ");
              return (
                <path
                  key={item.label}
                  d={path}
                  fill="none"
                  stroke={item.color}
                  strokeWidth="1.8"
                  vectorEffect="non-scaling-stroke"
                  data-series-label={item.label}
                  data-series-color={item.color}
                />
              );
            })}
            <line x1={left} x2={width - right} y1={baseline} y2={baseline} stroke="#6A757C" strokeWidth="1" />
            <text x={left} y={height - 12} fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">{timeLabel(new Date(firstTimestamp).toISOString())}</text>
            <text x={width - right} y={height - 12} textAnchor="end" fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">{timeLabel(new Date(lastTimestamp).toISOString())}</text>
          </svg>
        </div>
      ) : (
        <div role="status" className="flex h-44 items-center justify-center border-y border-zinc-800 text-xs text-zinc-500">
          {copy.waitingForTrend}
        </div>
      )}
      <ul className="mt-2 flex flex-wrap gap-x-3 gap-y-1" aria-label={copy.seriesLegend}>
        {series.map((item) => (
          <li key={item.label} className="flex items-center gap-1.5 text-[11px] text-zinc-400">
            <span className="h-2 w-2" style={{ backgroundColor: item.color }} aria-hidden="true" />
            <span>{item.label}</span>
          </li>
        ))}
      </ul>
    </figure>
  );
}

function DeviceFamilyCard({
  metrics,
  trends,
  samples,
  copy,
}: {
  metrics: TelemetryMetric[];
  trends: TrendMap;
  samples: LiveSampleMap;
  copy: TelemetryCopy;
}) {
  const identities = metrics.map((metric) => ({ metric, identity: deviceIdentity(metric)! }));
  const first = identities[0].identity;
  const devices = groupBy(identities, ({ identity }) => identity.deviceKey);
  const primary = [...devices.values()].flatMap((deviceMetrics) => {
    const selected = deviceMetrics.find(({ identity }) => identity.measure === identity.primaryMeasure);
    return selected ? [{ label: selected.identity.deviceLabel, metric: selected.metric }] : [];
  });
  const faultCount = identities.filter(({ metric, identity }) =>
    (identity.measure === "ecc_dbe" || identity.measure === "xid") && metric.value > 0,
  ).length;
  const temperatures = identities
    .filter(({ identity }) => identity.measure === "temp" || identity.measure === "mem_temp" || identity.measure === "celsius")
    .map(({ metric }) => metric.value);
  const average = primary.length > 0
    ? primary.reduce((sum, { metric }) => sum + metric.value, 0) / primary.length
    : null;
  const maximumTemperature = temperatures.length > 0 ? Math.max(...temperatures) : null;
  const deviceCount = devices.size;
  const isGpu = first.familyKey === "gpu";
  const familyLabel = isGpu ? copy.gpus : first.familyLabel;
  const primaryLabel = isGpu ? copy.gpuUtilization : first.primaryLabel;
  const countLabel = isGpu ? copy.gpuCount(deviceCount) : copy.deviceCount(deviceCount);
  const showLabel = isGpu ? copy.showGpus(deviceCount) : copy.showDevices(deviceCount);
  return (
    <Card className={faultCount > 0 ? "border-red-900/70 bg-red-950/10 sm:col-span-2 xl:col-span-4" : "border-zinc-800 bg-zinc-950/60 sm:col-span-2 xl:col-span-4"}>
      <CardContent className="p-0">
        <header className="flex flex-wrap items-start justify-between gap-3 border-b border-zinc-800 px-3 py-3">
          <div>
            <h4 className="text-sm font-semibold text-zinc-200">{familyLabel}</h4>
            <div className="mt-1 text-xs text-zinc-500">{countLabel}</div>
          </div>
          <div className="flex flex-wrap items-center gap-3 text-xs">
            {average !== null && <span className="text-zinc-500">{copy.average} <strong className="font-mono font-normal text-zinc-200">{formatValue(average, primary[0].metric.unit)}</strong></span>}
            {maximumTemperature !== null && <span className="text-zinc-500">{copy.maxTemperature} <strong className="font-mono font-normal text-zinc-200">{formatValue(maximumTemperature, "celsius")}</strong></span>}
            <span className={faultCount > 0 ? "border border-red-900 px-2 py-1 text-red-300" : "border border-zinc-700 px-2 py-1 text-zinc-400"}>{faultCount > 0 ? copy.faults(faultCount) : copy.noIssues}</span>
          </div>
        </header>
        <div className="px-3 py-3">
          {primary.length > 0 ? (
            <div className={cn("grid gap-3", isGpu && "lg:grid-cols-2")} data-testid={isGpu ? "gpu-current-and-trend" : undefined}>
              <FamilyComparisonChart familyLabel={familyLabel} primaryLabel={primaryLabel} devices={primary} copy={copy} />
              {isGpu && <FamilyTrendChart primaryLabel={primaryLabel} devices={primary} samples={samples} copy={copy} />}
            </div>
          ) : <div className="py-4 text-xs text-zinc-500">{copy.noComparableValues}</div>}
        </div>
        <details className="border-t border-zinc-800">
          <summary className="cursor-pointer px-3 py-2 text-xs font-medium text-zinc-400 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]">{showLabel}</summary>
          <div className="space-y-3 border-t border-zinc-900 p-3">
            {[...devices.entries()].map(([deviceKey, deviceMetrics]) => (
              <section key={deviceKey} aria-label={deviceMetrics[0].identity.deviceLabel}>
                <h5 className="mb-2 text-xs font-semibold text-zinc-400">{deviceMetrics[0].identity.deviceLabel}</h5>
                <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
                  {deviceMetrics.map(({ metric }) => <MetricCard key={`${metric.metric}:${metric.detail ?? ""}`} metric={metric} trend={trends[trendKey(metric)]} />)}
                </div>
              </section>
            ))}
          </div>
        </details>
      </CardContent>
    </Card>
  );
}

function MetricGroup({
  metrics,
  trends,
  samples,
  copy,
}: {
  metrics: TelemetryMetric[];
  trends: TrendMap;
  samples: LiveSampleMap;
  copy: TelemetryCopy;
}) {
  const families = new Map<string, TelemetryMetric[]>();
  const individual: TelemetryMetric[] = [];
  for (const metric of metrics) {
    const identity = deviceIdentity(metric);
    if (!identity) {
      individual.push(metric);
      continue;
    }
    families.set(identity.familyKey, [...(families.get(identity.familyKey) ?? []), metric]);
  }
  return (
    <div className="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
      {[...families.entries()].map(([family, familyMetrics]) => <DeviceFamilyCard key={family} metrics={familyMetrics} trends={trends} samples={samples} copy={copy} />)}
      {individual.map((metric) => <MetricCard key={`${metric.metric}:${metric.detail ?? ""}`} metric={metric} trend={trends[trendKey(metric)]} />)}
    </div>
  );
}

function statusTone(status: string): string {
  return /unreachable|failed|critical/i.test(status)
    ? "border-red-900/70 text-red-300"
    : /degraded|warning|stale/i.test(status)
      ? "border-amber-900/70 text-amber-300"
      : "border-zinc-700 text-zinc-400";
}

function timeLabel(value: string | null): string {
  if (!value) return "Not observed";
  const date = new Date(value);
  return Number.isNaN(date.valueOf()) ? "Not observed" : date.toISOString().replace("T", " ").replace(".000Z", "Z");
}

function readableStatus(status: string): string {
  return status.replaceAll("_", " ").replace(/^./, (first) => first.toUpperCase());
}

function groupBy<T>(values: T[], key: (value: T) => string): Map<string, T[]> {
  const groups = new Map<string, T[]>();
  for (const value of values) {
    const name = key(value);
    groups.set(name, [...(groups.get(name) ?? []), value]);
  }
  return groups;
}

export function TelemetryOverviewRenderer({
  payload,
  trends = {},
  samples = {},
}: {
  payload: unknown;
  trends?: TrendMap;
  samples?: LiveSampleMap;
}) {
  const { labels } = useI18n();
  const rawRows = rows(payload);
  const metrics = rawRows.flatMap((row) => {
    const metric = parseMetric(row);
    return metric ? [metric] : [];
  });
  if (metrics.length === 0) {
    return <EmptyState title="No readable telemetry" description="No signed human-presentation metadata was returned." />;
  }
  const targets = groupBy(metrics, (metric) => metric.targetId);

  return (
    <div className="space-y-4 p-3" data-testid="telemetry-overview">
      {[...targets.entries()].map(([targetId, targetMetrics]) => {
        const groups = groupBy(targetMetrics, (metric) => metric.group);
        const status = targetMetrics[0].status;
        const targetLabel = targetMetrics[0].targetLabel;
        const observedAt = targetMetrics.map((metric) => metric.observedAt).filter(Boolean).sort().at(-1) ?? null;
        return (
          <section key={targetId} aria-labelledby={`telemetry-${targetId}`} className="rounded border border-zinc-800 bg-[#101418]">
            <header className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-800 px-4 py-3">
              <div>
                <h2 id={`telemetry-${targetId}`} className="text-base font-semibold text-zinc-100">{targetLabel}</h2>
                <div className="mt-1 flex flex-wrap gap-x-3 font-mono text-[11px] text-zinc-500">
                  <span>Updated {timeLabel(observedAt)}</span>
                </div>
              </div>
              <span className={`rounded-sm border px-2.5 py-1.5 text-xs font-semibold ${statusTone(status)}`}>{readableStatus(status)}</span>
            </header>
            <div className="space-y-4 p-3">
              {[...groups.entries()].map(([group, groupMetrics]) => (
                <section key={group} aria-label={group}>
                  <h3 className="mb-2 text-xs font-semibold uppercase tracking-wider text-zinc-500">{group}</h3>
                  <MetricGroup metrics={groupMetrics} trends={trends} samples={samples} copy={labels.telemetry} />
                </section>
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

interface LiveTarget {
  id: string;
  label: string;
}

function targetsFrom(payload: unknown): LiveTarget[] {
  const targets = new Map<string, string>();
  for (const row of rows(payload)) {
    if (typeof row.target_id !== "string" || !row.target_id) continue;
    const label = typeof row.target_label === "string" && row.target_label.trim()
      ? row.target_label
      : row.target_id;
    targets.set(row.target_id, label);
  }
  return [...targets].map(([id, label]) => ({ id, label }));
}

function metricChoicesFrom(payload: unknown, targetId: string): MetricChoice[] {
  const choices = new Map<string, MetricChoice>();
  for (const row of rows(payload)) {
    const metric = parseMetric(row);
    if (!metric || metric.targetId !== targetId) continue;
    const key = trendKey(metric);
    choices.set(key, {
      key,
      metric: metric.metric,
      labels: metric.labels,
      label: metric.label,
      detail: metric.detail,
    });
  }
  return [...choices.values()].sort((left, right) =>
    left.label.localeCompare(right.label) || (left.detail ?? "").localeCompare(right.detail ?? ""),
  );
}

function mergeLiveSamples(payload: unknown, samples: LiveSample[]): unknown {
  const root = record(payload);
  if (!root || !Array.isArray(root.points) || samples.length === 0) return payload;
  const points = [...root.points];
  const seen = new Set(points.flatMap((value) => {
    const point = record(value);
    return typeof point?.ts === "string" ? [point.ts] : [];
  }));
  let added = 0;
  for (const sample of samples) {
    if (seen.has(sample.ts)) continue;
    points.push({
      ts: sample.ts,
      value: sample.value,
      avg: sample.value,
      min: sample.value,
      max: sample.value,
      samples: 1,
      source_tier: "live",
    });
    seen.add(sample.ts);
    added += 1;
  }
  points.sort((left, right) => {
    const leftAt = Date.parse(String(record(left)?.ts ?? ""));
    const rightAt = Date.parse(String(record(right)?.ts ?? ""));
    return leftAt - rightAt;
  });
  return { ...root, points: points.slice(-300), live_buffer_samples: added };
}

function mergeTargetRows(payload: unknown, liveRows: Record<string, JsonRecord[]>): unknown {
  const root = record(payload);
  const base = rows(payload).filter((row) => {
    const targetId = typeof row.target_id === "string" ? row.target_id : "";
    return !liveRows[targetId];
  });
  const merged = [...base, ...Object.values(liveRows).flat()].slice(0, MAX_ROWS);
  return { ...(root ?? {}), rows: merged, count: merged.length };
}

function usePageVisibility(): boolean {
  const [visible, setVisible] = useState(true);
  useEffect(() => {
    const update = () => setVisible(document.visibilityState === "visible");
    update();
    document.addEventListener("visibilitychange", update);
    return () => document.removeEventListener("visibilitychange", update);
  }, []);
  return visible;
}

export function LiveTelemetryWorkspaceRenderer({
  payload,
  apiKey,
  liveActionId,
  historyActionId,
  initialTargetId,
  initialRange,
  selectedTarget: controlledTarget,
  timeRange: controlledRange,
  onSelectedTargetChange,
  onTimeRangeChange,
}: {
  payload: unknown;
  apiKey: string | null;
  liveActionId: string;
  historyActionId?: string;
  initialTargetId?: string;
  initialRange?: string;
  selectedTarget?: string;
  timeRange?: string;
  onSelectedTargetChange?: (targetId: string) => void;
  onTimeRangeChange?: (range: HistoryRange) => void;
}) {
  const targets = useMemo(() => targetsFrom(payload), [payload]);
  const [localSelectedTarget, setLocalSelectedTarget] = useState(initialTargetId ?? "");
  const [localRange, setLocalRange] = useState<HistoryRange>(() =>
    HISTORY_RANGES.some((option) => option.value === initialRange)
      ? initialRange as HistoryRange
      : "live",
  );
  const selectedTarget = controlledTarget ?? localSelectedTarget;
  const range = HISTORY_RANGES.some((option) => option.value === controlledRange)
    ? controlledRange as HistoryRange
    : localRange;
  const changeTarget = useCallback((targetId: string) => {
    if (controlledTarget === undefined) setLocalSelectedTarget(targetId);
    onSelectedTargetChange?.(targetId);
  }, [controlledTarget, onSelectedTargetChange]);
  const changeRange = useCallback((nextRange: HistoryRange) => {
    if (controlledRange === undefined) setLocalRange(nextRange);
    onTimeRangeChange?.(nextRange);
  }, [controlledRange, onTimeRangeChange]);
  const [interval, setInterval] = useState<HistoryInterval>("auto");
  const [selectedMetricKey, setSelectedMetricKey] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [liveRows, setLiveRows] = useState<Record<string, JsonRecord[]>>({});
  const [trends, setTrends] = useState<TrendMap>({});
  const [liveSamples, setLiveSamples] = useState<LiveSampleMap>({});
  const [lastSuccess, setLastSuccess] = useState<string | null>(null);
  const [durationMs, setDurationMs] = useState<number | null>(null);
  const [collectorWarnings, setCollectorWarnings] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [history, setHistory] = useState<unknown>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const inFlight = useRef(false);
  const historyRequest = useRef(0);
  const pageVisible = usePageVisibility();
  const metricChoices = useMemo(
    () => metricChoicesFrom(payload, selectedTarget),
    [payload, selectedTarget],
  );
  const selectedMetric = useMemo(
    () => metricChoices.find((choice) => choice.key === selectedMetricKey) ?? metricChoices[0] ?? null,
    [metricChoices, selectedMetricKey],
  );

  useEffect(() => {
    if (targets.length === 0) return;
    if (!targets.some((target) => target.id === selectedTarget)) {
      changeTarget(targets[0]!.id);
    }
  }, [changeTarget, selectedTarget, targets]);

  useEffect(() => {
    if (!metricChoices.some((choice) => choice.key === selectedMetricKey)) {
      setSelectedMetricKey(metricChoices[0]?.key ?? "");
    }
  }, [metricChoices, selectedMetricKey]);

  const poll = useCallback(async () => {
    if (!selectedTarget || inFlight.current) return;
    inFlight.current = true;
    try {
      const response = await invokeAction(apiKey, liveActionId, { target_id: selectedTarget });
      const output = unwrapPayload(response);
      const outputRecord = record(output);
      const nextRows = rows(output).filter((row) => row.target_id === selectedTarget);
      if (nextRows.length === 0) throw new Error("Live telemetry returned no human-readable metrics.");
      setLiveRows((current) => ({ ...current, [selectedTarget]: nextRows }));
      setTrends((current) => {
        const next = { ...current };
        for (const row of nextRows) {
          const metric = parseMetric(row);
          if (!metric) continue;
          const key = trendKey(metric);
          next[key] = [...(next[key] ?? []), metric.value].slice(-MAX_TREND_POINTS);
        }
        return next;
      });
      const observedAt = typeof outputRecord?.observed_at === "string" ? outputRecord.observed_at : new Date().toISOString();
      setLiveSamples((current) => {
        const next = { ...current };
        for (const row of nextRows) {
          const metric = parseMetric(row);
          if (!metric) continue;
          const key = trendKey(metric);
          next[key] = [...(next[key] ?? []), { ts: observedAt, value: metric.value }].slice(-MAX_TREND_POINTS);
        }
        return next;
      });
      setLastSuccess(observedAt);
      setDurationMs(finite(outputRecord?.duration_ms));
      setCollectorWarnings(Array.isArray(outputRecord?.collector_warnings)
        ? outputRecord.collector_warnings.filter((warning): warning is string => typeof warning === "string").slice(0, 16)
        : []);
      setError(null);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "Live telemetry failed");
    } finally {
      inFlight.current = false;
    }
  }, [apiKey, liveActionId, selectedTarget]);

  useEffect(() => {
    if (range !== "live" || !enabled || !pageVisible || !selectedTarget) return;
    let cancelled = false;
    let timer: number | undefined;
    const tick = async () => {
      await poll();
      if (!cancelled) timer = window.setTimeout(() => void tick(), LIVE_INTERVAL_MS);
    };
    void tick();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [enabled, pageVisible, poll, range, selectedTarget]);

  const loadHistory = useCallback(async () => {
    if (range === "live" || !historyActionId || !selectedTarget || !selectedMetric) return;
    const request = ++historyRequest.current;
    setHistoryLoading(true);
    setHistoryError(null);
    try {
      const response = await invokeAction(apiKey, historyActionId, {
        target_id: selectedTarget,
        metric: selectedMetric.metric,
        labels: selectedMetric.labels,
        range,
        interval,
      });
      if (request !== historyRequest.current) return;
      setHistory(unwrapPayload(response));
    } catch (reason) {
      if (request !== historyRequest.current) return;
      setHistory(null);
      setHistoryError(reason instanceof Error ? reason.message : "Metric history failed");
    } finally {
      if (request === historyRequest.current) setHistoryLoading(false);
    }
  }, [apiKey, historyActionId, interval, range, selectedMetric, selectedTarget]);

  useEffect(() => {
    if (range === "live") {
      historyRequest.current += 1;
      setHistory(null);
      setHistoryError(null);
      setHistoryLoading(false);
      return;
    }
    void loadHistory();
  }, [loadHistory, range]);

  const merged = useMemo(() => mergeTargetRows(payload, liveRows), [liveRows, payload]);
  const selectedPayload = useMemo(() => {
    const selectedRows = rows(merged).filter((row) => row.target_id === selectedTarget);
    return { ...(record(merged) ?? {}), rows: selectedRows, count: selectedRows.length };
  }, [merged, selectedTarget]);
  const liveState = !pageVisible ? "Background paused" : enabled ? "Live · 3s" : "Paused";
  const intervalOptions = range === "live" ? [] : RANGE_INTERVALS[range];
  const historyPayload = useMemo(
    () => range === "5m" && selectedMetric
      ? mergeLiveSamples(history, liveSamples[selectedMetric.key] ?? [])
      : history,
    [history, liveSamples, range, selectedMetric],
  );
  const historyRecord = record(historyPayload);
  const coverage = record(historyRecord?.coverage);
  const availableRanges = historyActionId ? HISTORY_RANGES : HISTORY_RANGES.slice(0, 1);

  return (
    <div data-testid="live-telemetry-workspace">
      <div className="border-b border-zinc-800 bg-[#0d1115]">
        <div className="flex flex-wrap items-center gap-2 px-3 py-2.5">
          <select
            aria-label="Telemetry server"
            value={selectedTarget}
            onChange={(event) => {
              changeTarget(event.target.value);
              setLastSuccess(null);
              setDurationMs(null);
              setCollectorWarnings([]);
              setError(null);
            }}
            className="h-8 min-w-48 rounded border border-zinc-700 bg-zinc-950 px-2 text-xs text-zinc-200"
          >
            {targets.map((target) => <option key={target.id} value={target.id}>{target.label}</option>)}
          </select>
          <div className="penny-scroll flex max-w-full overflow-x-auto rounded border border-zinc-800 bg-zinc-950 p-0.5" role="group" aria-label="Telemetry range">
            {availableRanges.map((item) => (
              <button
                key={item.value}
                type="button"
                aria-pressed={range === item.value}
                onClick={() => {
                  changeRange(item.value);
                  setInterval("auto");
                }}
                className={cn(
                  "h-7 shrink-0 rounded px-2.5 text-xs font-medium focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[#B87333]",
                  range === item.value ? "bg-zinc-800 text-zinc-100" : "text-zinc-500 hover:text-zinc-200",
                )}
              >
                {item.label}
              </button>
            ))}
          </div>
        </div>
        {range === "live" ? (
          <div className="flex flex-wrap items-center gap-2 border-t border-zinc-900 px-3 py-2">
            <Button size="sm" variant={enabled ? "outline" : "secondary"} className="h-8 px-3 text-xs" onClick={() => setEnabled((value) => !value)} disabled={!selectedTarget}>
              {enabled ? "Pause" : "Resume"}
            </Button>
            <span className="rounded-sm border border-[#B87333]/40 bg-[#B87333]/10 px-2 py-1 font-mono text-xs text-[#d39a63]">{liveState}</span>
            <div className="ml-auto flex flex-wrap items-center gap-x-3 text-xs text-zinc-500">
              <span>{lastSuccess ? `Updated ${timeLabel(lastSuccess)}` : "Waiting for live sample"}</span>
              {durationMs !== null && <span className="font-mono">{Math.round(durationMs)} ms</span>}
              {collectorWarnings.length > 0 && (
                <details className="relative">
                  <summary className="cursor-pointer text-amber-300">{collectorWarnings.length} collector warning{collectorWarnings.length === 1 ? "" : "s"}</summary>
                  <div className="absolute right-0 z-20 mt-2 w-80 max-w-[calc(100vw-2rem)] rounded border border-amber-900/70 bg-[var(--surface)] p-3 text-xs leading-5 text-zinc-300">
                    {collectorWarnings.map((warning) => <div key={warning}>• {warning}</div>)}
                  </div>
                </details>
              )}
            </div>
          </div>
        ) : (
          <div className="flex flex-wrap items-center gap-2 border-t border-zinc-900 px-3 py-2">
            <select
              aria-label="Metric"
              value={selectedMetric?.key ?? ""}
              onChange={(event) => setSelectedMetricKey(event.target.value)}
              className="h-8 min-w-52 max-w-full rounded border border-zinc-700 bg-zinc-950 px-2 text-xs text-zinc-200"
            >
              {metricChoices.map((choice) => <option key={choice.key} value={choice.key}>{choice.label}{choice.detail ? ` · ${choice.detail}` : ""}</option>)}
            </select>
            <select
              aria-label="History interval"
              value={interval}
              onChange={(event) => setInterval(event.target.value as HistoryInterval)}
              className="h-8 rounded border border-zinc-700 bg-zinc-950 px-2 text-xs text-zinc-200"
            >
              {intervalOptions.map((item) => <option key={item} value={item}>{item === "auto" ? "Auto interval" : item}</option>)}
            </select>
            <Button size="sm" variant="outline" className="h-8 px-3 text-xs" onClick={() => void loadHistory()} disabled={historyLoading || !selectedMetric}>
              {historyLoading ? "Refreshing…" : "Refresh"}
            </Button>
            {historyRecord && (
              <div className="ml-auto flex flex-wrap items-center gap-x-3 text-xs text-zinc-500">
                <span>{String(historyRecord.effective_interval ?? interval)} interval</span>
                <span>{coverage?.start ? `${timeLabel(String(coverage.start))} → ${timeLabel(String(coverage.end))}` : "No coverage"}</span>
                {finite(historyRecord.live_buffer_samples) ? <span>{finite(historyRecord.live_buffer_samples)} live samples</span> : null}
              </div>
            )}
          </div>
        )}
      </div>
      {range === "live" && error && <div role="alert" className="border-b border-red-950 bg-red-950/20 px-3 py-2 text-xs text-red-300">Live read failed · showing the last available values · {error}</div>}
      <div data-testid="telemetry-current-and-history">
        <TelemetryOverviewRenderer payload={selectedPayload} trends={trends} samples={liveSamples} />
        {range !== "live" && <div className="space-y-3 border-t border-zinc-800 p-3">
          {historyError && <div role="alert" className="rounded border border-red-900/60 bg-red-950/20 px-3 py-2 text-xs text-red-300">Metric history unavailable · {historyError}</div>}
          {historyRecord?.partial === true && (
            <div role="status" className="rounded border border-amber-900/60 bg-amber-950/20 px-3 py-2 text-xs text-amber-200">
              Partial history · available samples and gaps are shown without interpolation.
            </div>
          )}
          {historyLoading && historyPayload === null ? <div className="p-6 text-center text-xs text-zinc-500">Loading metric history…</div> : historyPayload !== null ? <InteractiveTimeseriesRenderer payload={historyPayload} /> : !historyError ? <EmptyState title="No metric history" description="Choose a visible metric with persisted samples." /> : null}
        </div>}
      </div>
    </div>
  );
}
