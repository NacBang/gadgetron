"use client";

import { seriesColor } from "@/lib/chart-palette";
import { Card, CardContent } from "../ui/card";
import { EmptyState } from "./empty-state";

type JsonRecord = Record<string, unknown>;

interface Point {
  timestamp: string;
  milliseconds: number;
  value: number;
  minimum: number;
  maximum: number;
  samples: number;
  source: string | null;
}

const MAX_POINTS = 500;
const WIDTH = 1_000;
const HEIGHT = 300;
const LEFT = 76;
const RIGHT = 24;
const TOP = 24;
const BOTTOM = 52;

function record(value: unknown): JsonRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as JsonRecord
    : null;
}

function parsePoints(payload: JsonRecord): Point[] {
  const raw = Array.isArray(payload.points) ? payload.points.slice(0, MAX_POINTS) : [];
  return raw.flatMap((value) => {
    const point = record(value);
    const timestamp = typeof point?.ts === "string"
      ? point.ts
      : typeof point?.timestamp === "string"
        ? point.timestamp
        : null;
    const numeric = Number(point?.value);
    const minimum = Number(point?.min ?? numeric);
    const maximum = Number(point?.max ?? numeric);
    const samples = Number(point?.samples ?? 1);
    const milliseconds = timestamp === null ? Number.NaN : Date.parse(timestamp);
    return timestamp !== null && Number.isFinite(milliseconds) && Number.isFinite(numeric) && Number.isFinite(minimum) && Number.isFinite(maximum)
      ? [{
          timestamp,
          milliseconds,
          value: numeric,
          minimum,
          maximum,
          samples: Number.isFinite(samples) && samples > 0 ? samples : 1,
          source: typeof point?.source_tier === "string" ? point.source_tier : null,
        }]
      : [];
  }).sort((left, right) => left.milliseconds - right.milliseconds);
}

function formatValue(value: number): string {
  if (Math.abs(value) >= 1_000) return value.toLocaleString(undefined, { maximumFractionDigits: 1 });
  if (Math.abs(value) >= 10) return value.toFixed(1);
  return value.toFixed(2);
}

function timeLabel(timestamp: string): string {
  return new Date(timestamp).toISOString().replace("T", " ").replace(".000Z", "Z");
}

function intervalMilliseconds(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const match = /^(\d+)(s|m|h|d)$/.exec(value);
  if (!match) return null;
  const amount = Number(match[1]);
  const unit = match[2] === "s" ? 1_000 : match[2] === "m" ? 60_000 : match[2] === "h" ? 3_600_000 : 86_400_000;
  return amount * unit;
}

export function InteractiveTimeseriesRenderer({ payload }: { payload: unknown }) {
  const root = record(payload);
  if (!root) {
    return <EmptyState title="Incompatible metric series" description="Timeseries payload must be an object." />;
  }
  const points = parsePoints(root);
  if (points.length === 0) {
    return <EmptyState title="No metric samples" description="No finite timestamped values were returned for this series." />;
  }
  const presentation = record(root.presentation);
  const metric = typeof root.metric === "string" ? root.metric.slice(0, 128) : "Metric";
  const metricLabel = typeof presentation?.label === "string" ? presentation.label.slice(0, 128) : metric;
  const target = typeof root.target_id === "string" ? root.target_id.slice(0, 128) : "Unknown target";
  const unit = typeof root.unit === "string" && root.unit.length > 0 ? root.unit.slice(0, 32) : "unitless";
  const minimum = Math.min(...points.map((point) => point.value));
  const maximum = Math.max(...points.map((point) => point.value));
  const signedMinimum = Number(presentation?.min);
  const signedMaximum = Number(presentation?.max);
  const fixedScale = Number.isFinite(signedMinimum) && Number.isFinite(signedMaximum) && signedMaximum > signedMinimum;
  const padding = !fixedScale && minimum === maximum ? Math.max(Math.abs(minimum) * 0.05, 1) : 0;
  const yMinimum = fixedScale ? signedMinimum : minimum - padding;
  const yMaximum = fixedScale ? signedMaximum : maximum + padding;
  const yRange = yMaximum - yMinimum || 1;
  const xMinimum = points[0].milliseconds;
  const xMaximum = points.at(-1)!.milliseconds;
  const xRange = xMaximum - xMinimum || 1;
  const x = (milliseconds: number) => LEFT + ((milliseconds - xMinimum) / xRange) * (WIDTH - LEFT - RIGHT);
  const y = (value: number) => {
    const normalized = Math.min(1, Math.max(0, (yMaximum - value) / yRange));
    return TOP + normalized * (HEIGHT - TOP - BOTTOM);
  };
  const step = intervalMilliseconds(root.effective_interval);
  const paths: string[] = [];
  let path = "";
  points.forEach((point, index) => {
    const gap = index > 0 && step !== null && point.milliseconds - points[index - 1].milliseconds > step * 1.5;
    if (gap && path) {
      paths.push(path);
      path = "";
    }
    path += `${path ? " L" : "M"}${x(point.milliseconds).toFixed(2)},${y(point.value).toFixed(2)}`;
  });
  if (path) paths.push(path);
  const latest = points.at(-1)!;
  const midpoint = (yMinimum + yMaximum) / 2;
  const requestedRange = typeof root.requested_range === "string" ? root.requested_range : null;
  const effectiveInterval = typeof root.effective_interval === "string" ? root.effective_interval : null;
  const coverage = record(root.coverage);
  const gaps = Array.isArray(root.gaps) ? root.gaps.length : 0;
  const liveSamples = Number(root.live_buffer_samples ?? 0);

  return (
    <figure className="space-y-3 p-3" aria-labelledby="timeseries-title">
      <figcaption id="timeseries-title" className="flex flex-wrap items-end justify-between gap-2 border-b border-zinc-800 pb-2">
        <div>
          <div className="text-sm font-medium text-zinc-200">{metricLabel}</div>
          <div className="mt-1 flex flex-wrap gap-x-2 text-xs text-zinc-500">
            <span>{target}</span>
            {requestedRange && <span>{requestedRange} range</span>}
            {effectiveInterval && <span>{effectiveInterval} interval</span>}
            <span>{points.length} points</span>
            <span>{unit}</span>
          </div>
        </div>
        <div className="font-mono text-base text-zinc-100">{formatValue(latest.value)} <span className="text-xs text-zinc-500">{unit}</span></div>
      </figcaption>
      {(coverage || gaps > 0 || liveSamples > 0) && (
        <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-zinc-500">
          {typeof coverage?.start === "string" && typeof coverage?.end === "string" && <span>Coverage {timeLabel(coverage.start)} → {timeLabel(coverage.end)}</span>}
          <span>{gaps === 0 ? "No observed gaps" : `${gaps} observed gap${gaps === 1 ? "" : "s"}`}</span>
          {liveSamples > 0 && <span>{liveSamples} browser live sample{liveSamples === 1 ? "" : "s"}</span>}
        </div>
      )}
      <div className="overflow-x-auto rounded border border-zinc-800 bg-[#101418]">
        <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} className="h-[300px] min-w-[720px] w-full" role="img" aria-label={`${metricLabel} from ${timeLabel(points[0].timestamp)} to ${timeLabel(latest.timestamp)}, ${points.length} points in ${unit}${gaps > 0 ? ` with ${gaps} gaps` : ""}`} data-scale-mode={fixedScale ? "fixed" : "observed"} data-scale-min={yMinimum} data-scale-max={yMaximum}>
          {[yMaximum, midpoint, yMinimum].map((tick) => (
            <g key={tick}>
              <line x1={LEFT} x2={WIDTH - RIGHT} y1={y(tick)} y2={y(tick)} stroke="#262D33" strokeWidth="1" />
              <text x={LEFT - 10} y={y(tick) + 4} textAnchor="end" fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">{formatValue(tick)}</text>
            </g>
          ))}
          <line x1={LEFT} x2={LEFT} y1={TOP} y2={HEIGHT - BOTTOM} stroke="#6A757C" strokeWidth="1" />
          <line x1={LEFT} x2={WIDTH - RIGHT} y1={HEIGHT - BOTTOM} y2={HEIGHT - BOTTOM} stroke="#6A757C" strokeWidth="1" />
          {paths.map((segment, index) => <path key={index} d={segment} fill="none" stroke={seriesColor(0)} strokeWidth="1.8" vectorEffect="non-scaling-stroke" />)}
          <circle cx={x(latest.milliseconds)} cy={y(latest.value)} r="3" fill={seriesColor(0)} />
          <text x={LEFT} y={HEIGHT - 23} fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">{timeLabel(points[0].timestamp)}</text>
          <text x={WIDTH - RIGHT} y={HEIGHT - 23} textAnchor="end" fill="#6A757C" fontSize="11" fontFamily="JetBrains Mono, monospace">{timeLabel(latest.timestamp)}</text>
          <text x="18" y={TOP - 7} fill="#6A757C" fontSize="12" fontFamily="JetBrains Mono, monospace">{unit}</text>
        </svg>
      </div>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
        {[
          ["Latest", latest.value],
          ["Minimum", minimum],
          ["Maximum", maximum],
          ["Samples", points.length],
        ].map(([label, value]) => (
          <Card key={label} className="border-zinc-800 bg-zinc-950/60"><CardContent className="p-2"><div className="text-xs font-semibold uppercase tracking-wider text-zinc-500">{label}</div><div className="mt-1 font-mono text-sm text-zinc-200">{typeof value === "number" ? formatValue(value) : value}</div></CardContent></Card>
        ))}
      </div>
      <details className="rounded border border-zinc-800 bg-zinc-950/50">
        <summary className="cursor-pointer px-3 py-2 text-xs font-semibold uppercase tracking-wider text-zinc-500">Sample table</summary>
        <div className="max-h-80 overflow-auto border-t border-zinc-800">
          <table className="min-w-full text-left text-xs">
            <thead className="sticky top-0 bg-zinc-950"><tr><th className="border-b border-zinc-800 px-3 py-2 text-zinc-500">Timestamp (UTC)</th><th className="border-b border-zinc-800 px-3 py-2 text-right text-zinc-500">Average</th><th className="border-b border-zinc-800 px-3 py-2 text-right text-zinc-500">Observed range</th><th className="border-b border-zinc-800 px-3 py-2 text-right text-zinc-500">Samples</th><th className="border-b border-zinc-800 px-3 py-2 text-zinc-500">Source</th></tr></thead>
            <tbody>{points.map((point, index) => <tr key={`${point.timestamp}-${index}`} className="border-b border-zinc-900"><td className="px-3 py-2 font-mono text-zinc-400">{timeLabel(point.timestamp)}</td><td className="px-3 py-2 text-right font-mono text-zinc-300">{formatValue(point.value)} {unit}</td><td className="px-3 py-2 text-right font-mono text-zinc-400">{formatValue(point.minimum)}–{formatValue(point.maximum)}</td><td className="px-3 py-2 text-right font-mono text-zinc-400">{point.samples}</td><td className="px-3 py-2 text-xs text-zinc-500">{point.source ?? "persisted"}</td></tr>)}</tbody>
          </table>
        </div>
      </details>
    </figure>
  );
}
