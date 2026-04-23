"use client";

import { useMemo } from "react";

// Plain-SVG sparkline. No external charting dep — keeps the bundle
// lean and the bytes shipped per host card under 2 KB. The detail
// drawer (task #24) can opt in to a heavier lib like recharts.

export interface SparkPoint {
  ts: string;
  avg: number;
  min?: number;
  max?: number;
}

interface SparklineProps {
  points: SparkPoint[];
  /** Optional stacked series — each inner array is drawn as a
   * translucent line on the SAME chart so callers can overlay e.g.
   * all per-GPU utilizations on one graph. `points` remains the
   * primary series (drawn on top, fully opaque); `series` lines are
   * lower opacity for visual layering. All arrays must share the
   * same Y scale and timestamp spacing. */
  series?: SparkPoint[][];
  /** Height in pixels — width auto-stretches to the parent. */
  height?: number;
  /** Reserve a vertical range so a flat line doesn't fill the box.
   * `undefined` autoscales to data extent. */
  yMin?: number;
  yMax?: number;
  /** Tailwind color class for the line/area. */
  tone?: "blue" | "amber" | "emerald" | "red" | "zinc";
  /** Tooltip-friendly label (rendered above the SVG). */
  label?: string;
  /** Right-aligned current value text. Caller formats. */
  current?: string;
}

const TONE: Record<NonNullable<SparklineProps["tone"]>, string> = {
  blue: "stroke-blue-400 fill-blue-400/10",
  amber: "stroke-amber-400 fill-amber-400/10",
  emerald: "stroke-emerald-400 fill-emerald-400/10",
  red: "stroke-red-400 fill-red-400/10",
  zinc: "stroke-zinc-500 fill-zinc-500/10",
};

export function Sparkline({
  points,
  series,
  height = 24,
  yMin,
  yMax,
  tone = "blue",
  label,
  current,
}: SparklineProps) {
  const path = useMemo(() => {
    const allSeries = series ?? [];
    const primary = points.length >= 2 ? points : null;
    if (!primary && allSeries.every((s) => s.length < 2)) return null;
    // Compute shared Y scale across every series so overlapping lines
    // stay comparable. Pull from primary + secondary in one pass.
    const allVals: number[] = [];
    if (primary) for (const p of primary) allVals.push(p.avg);
    for (const s of allSeries) for (const p of s) allVals.push(p.avg);
    const lo = yMin ?? (allVals.length > 0 ? Math.min(...allVals) : 0);
    const hi = yMax ?? (allVals.length > 0 ? Math.max(...allVals) : 1);
    const range = hi - lo || 1;
    const w = 100;
    const h = height;
    const toPath = (pts: SparkPoint[]): string | null => {
      if (pts.length < 2) return null;
      const dx = w / (pts.length - 1);
      const xy = pts.map((p, i) => {
        const x = i * dx;
        const y = h - ((p.avg - lo) / range) * h;
        return [x, Number.isFinite(y) ? y : h] as const;
      });
      return (
        "M " +
        xy.map(([x, y]) => `${x.toFixed(2)},${y.toFixed(2)}`).join(" L ")
      );
    };
    const primaryLine = primary ? toPath(primary) : null;
    const primaryArea = primaryLine
      ? `${primaryLine} L ${w},${h} L 0,${h} Z`
      : null;
    const secondaryLines = allSeries
      .map((s) => toPath(s))
      .filter((p): p is string => p != null);
    return { primaryLine, primaryArea, secondaryLines, w, h };
  }, [points, series, height, yMin, yMax]);

  return (
    <div className="flex flex-col gap-0.5" data-testid="sparkline">
      {(label || current) && (
        <div className="flex items-center justify-between text-[10px]">
          {label && <span className="text-zinc-500">{label}</span>}
          {current && (
            <span className="font-mono text-zinc-300">{current}</span>
          )}
        </div>
      )}
      {path ? (
        <svg
          viewBox={`0 0 ${path.w} ${path.h}`}
          preserveAspectRatio="none"
          className="w-full"
          style={{ height: `${height}px` }}
        >
          {/* Secondary series — drawn first at lower opacity so the
              primary line stays dominant. Useful for "all GPUs util
              on one chart" type overlays. */}
          {path.secondaryLines.map((d, i) => (
            <path
              key={i}
              d={d}
              className={TONE[tone]}
              fill="none"
              strokeWidth={1}
              strokeOpacity={0.45}
              vectorEffect="non-scaling-stroke"
            />
          ))}
          {path.primaryArea && (
            <path
              d={path.primaryArea}
              className={`stroke-0 ${TONE[tone]}`}
            />
          )}
          {path.primaryLine && (
            <path
              d={path.primaryLine}
              className={TONE[tone]}
              fill="none"
              strokeWidth={1.4}
              vectorEffect="non-scaling-stroke"
            />
          )}
        </svg>
      ) : (
        <div
          className="w-full rounded bg-zinc-900/50"
          style={{ height: `${height}px` }}
          aria-label="not enough data yet"
        />
      )}
    </div>
  );
}
